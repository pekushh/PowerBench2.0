//! Ядро нагрузки GamingCpuV1: тик-процессор, persistent main-поток и пул,
//! reset, цепочка контрольных сумм, буфер сэмплов, снапшоты прогресса.
//!
//! Главный поток бенчмарка — поток вызывающего кода: он живёт от вызова к
//! вызову и не пересоздаётся между фазами. Воркеры и их пул — persistent.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::buffers::{EntityBuffers, RawShared};
use crate::checksum::{finalize_tick, mix, start_run_checksum};
use crate::config::{
    DT_SECONDS, ENTITY_CAPACITY, MAXIMUM_JOBS, RESPONSE_SUPERCYCLE, SEED, VERSION, config_hash,
};
use crate::config::{
    Phase, Profile, ProfileParams, VELOCITY_DAMPING, VELOCITY_STEP, profile_params,
    response_profile,
};
use crate::pool::{JobDescriptor, Pool};
use crate::prng::wrap_position;
use crate::sample::{SampleBuffer, capacity_for_ticks};
use crate::topology::{AffinityMode, CpuTopology};

/// Число тиков в коротких прогонах самопроверки ядра.
const SELF_CHECK_TICKS: u64 = 32;

/// Запас сверх длительности фазы, прежде чем батч признаётся зависшим.
/// Тик должен уложиться в сотые доли секунды даже на самой слабой машине;
/// минута запаса означает уже не зависание, а потерю процесса.
const BATCH_TIMEOUT_MARGIN: Duration = Duration::from_secs(60);

/// Потолок батча для прогонов по числу тиков (самопроверка): 32 тика за
/// минуту — уже не замер, а зависание.
const TICK_BATCH_TIMEOUT: Duration = Duration::from_secs(60);

/// Цель прогона фазы.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunTarget {
    /// Ровно `N` тиков (для самопроверки и юнит-тестов).
    Ticks(u64),
    /// До истечения длительности по настенным часам.
    Duration(Duration),
}

/// Ошибка прогона.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunError {
    /// Фаза отменена пользователем (кооперативно).
    Cancelled,
    /// Ошибка воркера — запуск невалиден, fallback запрещён.
    WorkerFailed,
    /// Буфер сэмплов заполнился раньше конца фазы.
    SampleCapacityReached { capacity: usize },
    /// Самопроверка/сверка эталона не прошла — рассинхрон контрольных сумм.
    DeterminismRecheckFailed,
}

/// Отчёт о завершённом прогоне фазы.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunReport {
    pub phase: Phase,
    pub ticks: u64,
    /// Полностью завершённые 256-тиковые суперциклы (для фазы «Отклик»).
    pub supercycles_completed: u64,
    pub first_tick_checksum: u64,
    pub run_checksum: u64,
    pub samples_written: usize,
}

/// Снапшот прогресса для UI (атомарные счётчики, опрос отдельным потоком).
#[derive(Clone, Copy, Debug)]
pub struct ProgressSnapshot {
    pub running: bool,
    pub ticks_done: u64,
    pub elapsed_secs: u64,
    /// Время БАТЧА целиком: вычислительная часть плюс синхронизация пула.
    /// По нему считается темп тиков — он отражает реальную скорость цикла.
    pub last_tick_ns: u64,
    pub current_ticks_per_sec: u64,
    /// Время ВЫЧИСЛИТЕЛЬНОЙ части последнего тика — то, что пишется в замер.
    pub last_compute_ns: u64,
    /// Время батча целиком (дублирует `last_tick_ns` для пары счётчиков,
    /// обновляемых в одном месте).
    pub last_total_ns: u64,
}

/// Табло прогресса для UI (атомарные счётчики, опрос отдельным потоком).
#[derive(Debug)]
#[doc(hidden)]
pub struct Scoreboard {
    running: AtomicBool,
    ticks_done: AtomicU64,
    elapsed_secs: AtomicU64,
    last_tick_ns: AtomicU64,
    current_ticks_per_sec: AtomicU64,
    last_compute_ns: AtomicU64,
    last_total_ns: AtomicU64,
}

impl Scoreboard {
    /// Снапшот текущего состояния табло.
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            running: self.running.load(Ordering::Relaxed),
            ticks_done: self.ticks_done.load(Ordering::Relaxed),
            elapsed_secs: self.elapsed_secs.load(Ordering::Relaxed),
            last_tick_ns: self.last_tick_ns.load(Ordering::Relaxed),
            current_ticks_per_sec: self.current_ticks_per_sec.load(Ordering::Relaxed),
            last_compute_ns: self.last_compute_ns.load(Ordering::Relaxed),
            last_total_ns: self.last_total_ns.load(Ordering::Relaxed),
        }
    }
}

/// Число профилей нагрузки: столько эталонов нужно хранить движку.
///
/// Считается от последнего профиля, а не константой в коде: при добавлении
/// профиля массив должен вырасти сам, иначе эталон для новой фазы негде будет
/// положить, и её детерминизм молча перестанет проверяться.
const REFERENCE_SLOTS: usize = Profile::ResponseMajor.index() + 1;

/// Движок ядра нагрузки.
#[derive(Debug)]
pub struct Engine {
    entities: Arc<RawShared<EntityBuffers>>,
    pool: Pool,
    sample_buffer: Option<SampleBuffer>,
    /// Эталонные контрольные суммы первого тика (индексируются Profile::index).
    first_tick_references: [u64; REFERENCE_SLOTS],
    scoreboard: Arc<Scoreboard>,
    logical_cpus: usize,
    worker_count: usize,
    /// Какое число воркеров просил вызывающий (`None` — «по умолчанию»).
    /// Сохраняется, чтобы умещение можно было объяснить в журнале, а не
    /// делать вид, что запрос был удовлетворён.
    requested_worker_count: Option<usize>,
    config_hash: String,
    topology: CpuTopology,
}

/// Резерв одного ядра под главный поток и одного под Windows/UI.
fn default_worker_count(logical_cpus: usize) -> usize {
    logical_cpus.saturating_sub(2).max(1)
}

/// Число воркеров с учётом трёх потолков.
///
/// `MAXIMUM_JOBS` ограничивает не «разум», а полезность: пул шире, чем задач
/// на тик, не делает ничего — каждый лишний воркер всё равно просыпается на
/// `notify_all`, берёт мьютекс и рапортует о завершении, то есть добавляет
/// МЕРИМУЮ синхронизацию вместо вычислений. На сервере со 192 потоками без
/// этого ограничения в тик входило бы больше сотни холостых пробуждений.
///
/// Потолок по числу слотов привязки обязателен в режимах с маской: воркер без
/// ядра вернул бы в измерение ровно то, ради чего привязка и ставилась.
fn worker_count_for(
    logical_cpus: usize,
    requested: Option<usize>,
    topology: &CpuTopology,
) -> usize {
    let base = requested
        .unwrap_or_else(|| default_worker_count(logical_cpus))
        .max(1);
    let cap = match topology.mode() {
        // Привязка выключена или не разведана: ограничивать нечем.
        AffinityMode::AffinityOff => MAXIMUM_JOBS,
        _ => {
            let slots = topology.slots();
            if slots == 0 { MAXIMUM_JOBS } else { slots }
        }
    };
    base.min(cap)
}

impl Engine {
    /// Создать движок в режиме привязки по умолчанию (P-ядра, либо режим из
    /// `POWERBENCH_AFFINITY`). `worker_count = None` → `max(1, logical_cpus − 2)`,
    /// дополнительно ограниченное `MAXIMUM_JOBS` и числом доступных ядер.
    /// Число воркеров фиксируется на запуск и сохраняется во все результаты.
    pub fn new(worker_count: Option<usize>) -> Self {
        Self::new_with_affinity(worker_count, AffinityMode::resolve_default())
    }

    /// Создать движок с явно заданным режимом привязки потоков.
    pub fn new_with_affinity(worker_count: Option<usize>, mode: AffinityMode) -> Self {
        let logical_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let topology = CpuTopology::detect(mode);
        let requested = worker_count;
        let worker_count = worker_count_for(logical_cpus, requested, &topology);
        let entities = Arc::new(RawShared::new(EntityBuffers::from_seed(SEED)));
        let pool = Pool::new_with_affinity(worker_count, entities.clone(), Some(&topology));
        Self {
            entities,
            pool,
            sample_buffer: None,
            first_tick_references: [0u64; REFERENCE_SLOTS],
            scoreboard: Arc::new(Scoreboard {
                running: AtomicBool::new(false),
                ticks_done: AtomicU64::new(0),
                elapsed_secs: AtomicU64::new(0),
                last_tick_ns: AtomicU64::new(0),
                current_ticks_per_sec: AtomicU64::new(0),
                last_compute_ns: AtomicU64::new(0),
                last_total_ns: AtomicU64::new(0),
            }),
            logical_cpus,
            worker_count,
            requested_worker_count: requested,
            config_hash: config_hash().to_string(),
            topology,
        }
    }

    /// Топология и режим привязки, под которыми работает движок.
    pub fn topology(&self) -> &CpuTopology {
        &self.topology
    }

    /// Режим привязки потоков.
    pub fn affinity_mode(&self) -> AffinityMode {
        self.topology.mode()
    }

    /// Подпись размещения для `CompatibilitySignature`.
    pub fn affinity_signature(&self) -> String {
        self.topology.signature()
    }

    /// Какое число воркеров просил вызывающий (`None` — «по умолчанию»).
    pub fn requested_worker_count(&self) -> Option<usize> {
        self.requested_worker_count
    }

    /// Число воркеров уменьшилось относительно запроса.
    ///
    /// Умещение — не ошибка, а требование к измеримости: лишний воркер не
    /// получил бы своего ядра и добавил бы в измерение холостые пробуждения.
    /// Но сказать об этом пользователю обязательно, иначе «7 воркеров» в
    /// настройках молча превращаются в 6.
    pub fn worker_count_clamped(&self) -> bool {
        self.requested_worker_count
            .is_some_and(|r| r != self.worker_count)
    }

    /// Пояснение к числу воркеров для журнала сессии (пусто — умещения не было).
    pub fn worker_count_note(&self) -> String {
        if !self.worker_count_clamped() {
            return String::new();
        }
        let requested = self.requested_worker_count.unwrap_or(0);
        format!(
            "число воркеров уменьшено с {requested} до {}: {}",
            self.worker_count,
            match self.topology.mode() {
                AffinityMode::AffinityOff =>
                    format!("потолок MAXIMUM_JOBS={MAXIMUM_JOBS} (привязка выключена)"),
                _ if self.topology.slots() == 0 => {
                    "топология ядер не разведана, действует потолок по числу воркеров".to_string()
                }
                _ => format!(
                    "в режиме «{}» доступно {} ядер",
                    self.topology.mode().as_str(),
                    self.topology.slots()
                ),
            }
        )
    }

    /// Воркеры, которым ОС отказала в маске привязки (пусто — привязка
    /// поставлена всем воркерам либо выключена).
    pub fn affinity_failures(&self) -> &[String] {
        self.pool.affinity_failures()
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    pub fn logical_cpus(&self) -> usize {
        self.logical_cpus
    }

    pub fn seed(&self) -> u64 {
        SEED
    }

    pub fn version(&self) -> &'static str {
        VERSION
    }

    pub fn config_hash(&self) -> &str {
        &self.config_hash
    }

    /// Разделяемый флаг отмены текущей фазы.
    pub fn canceller(&self) -> Arc<AtomicBool> {
        self.pool.cancel_handle()
    }

    /// Пул воркеров гарантированно не работает (никто не пишет в буферы).
    /// Используется после отменённой фазы как проверка «нагрузка остановлена».
    pub fn pool_quiesced(&self) -> bool {
        self.pool.is_quiesced()
    }

    /// Дождаться завершения воркеров (в пределах разумного времени).
    pub fn quiesce_pool(&self) -> bool {
        self.pool.quiesce()
    }

    /// Кооперативная отмена текущего прогона.
    pub fn cancel(&self) {
        self.pool.cancel();
    }

    /// Снапшот прогресса (атомарный опрос, без побочных эффектов в цикле тика).
    pub fn progress_snapshot(&self) -> ProgressSnapshot {
        self.scoreboard.snapshot()
    }

    /// Разделяемое табло прогресса — для сторожевого таймера отдельным потоком,
    /// пока главный поток удерживает `&mut self` в `run_phase`.
    pub fn scoreboard_arc(&self) -> Arc<Scoreboard> {
        self.scoreboard.clone()
    }

    /// Подготовить буфер сэмплов под длительность фазы (выделение здесь ок).
    pub fn prepare_sample_buffer(&mut self, phase: Phase, duration_secs: u64) {
        self.sample_buffer = Some(SampleBuffer::new(phase, duration_secs));
    }

    /// Reset ядра перед прогоном/фазой: буферы к состоянию из Seed, индексы
    /// тиков профилей обнуляются, слоты и runChecksum восстанавливаются,
    /// пул переводится в idle, отмена снимается.
    ///
    /// Порядок обязателен: **сначала** пул останавливается, **потом** пишутся
    /// буферы. Если грейс отмены истёк, опоздавший воркер ещё исполняет
    /// `run_job` и пишет в буферы сущностей; сбросить их в этот момент — гонка
    /// памяти, а снятие флага отмены до остановки пула заставит воркер
    /// досчитать все задачи поверх только что записанных значений. Следующая
    /// фаза стартовала бы с недетерминированного состояния, и контрольная сумма
    /// расходилась бы между прогонами.
    ///
    /// Возвращает `false`, если пул не затих: состояние после такого сброса
    /// недостоверно, и мерить дальше нельзя. Молча продолжать нельзя — вызывающий
    /// обязан либо остановиться, либо знать, что данные негодны.
    pub fn reset(&mut self) -> bool {
        let quiet = self.pool.reset_to_idle();
        // Отмена снимается только после тишины пула: иначе воркер успеет
        // доработать прерванный батч поверх новых буферов.
        self.pool.clear_cancel();
        unsafe { &mut *self.entities.get() }.reset();
        quiet
    }

    /// Запустить фазу (вызывающий выполняет reset перед каждой фазой).
    pub fn run_phase(&mut self, phase: Phase, target: RunTarget) -> Result<RunReport, RunError> {
        if self.sample_buffer.is_none() {
            match target {
                RunTarget::Ticks(n) => {
                    let capacity = capacity_for_ticks(phase, n);
                    self.sample_buffer = Some(SampleBuffer::with_capacity(capacity));
                }
                RunTarget::Duration(d) => {
                    self.sample_buffer = Some(SampleBuffer::new(phase, d.as_secs().max(1)));
                }
            }
        }

        self.scoreboard.running.store(true, Ordering::Release);
        self.scoreboard.ticks_done.store(0, Ordering::Relaxed);
        self.scoreboard.elapsed_secs.store(0, Ordering::Relaxed);
        if let Some(buffer) = self.sample_buffer.as_mut() {
            buffer.clear();
        }
        let result = self.run_inner(phase, target);
        self.scoreboard.running.store(false, Ordering::Release);
        result
    }

    fn run_inner(&mut self, phase: Phase, target: RunTarget) -> Result<RunReport, RunError> {
        let started = Instant::now();
        let buffer = self.sample_buffer.as_mut().ok_or(RunError::WorkerFailed)?;
        let mut run_checksum = start_run_checksum();
        let mut first_tick_checksum: Option<u64> = None;
        let mut phase_index = 0u64;
        let mut global_tick = 0u64;

        loop {
            if self.pool.canceled() {
                return Err(RunError::Cancelled);
            }
            match target {
                RunTarget::Ticks(n) if global_tick >= n => break,
                RunTarget::Duration(d) if started.elapsed() >= d => break,
                _ => {}
            }
            if buffer.is_full() {
                return Err(RunError::SampleCapacityReached {
                    capacity: buffer.capacity(),
                });
            }

            let params = match phase {
                Phase::Light => profile_params(Profile::Light),
                Phase::Partial => profile_params(Profile::Partial),
                Phase::Heavy => profile_params(Profile::Heavy),
                Phase::Response => profile_params(response_profile(phase_index)),
            };
            // Сколько воркеров реально берут задачи: в фазе частичной нагрузки
            // вторая половина пула простаивает, и это и есть измеряемый режим.
            let active_workers = active_workers_for(phase, self.pool.worker_count());

            // Измеряется ТОЛЬКО вычислительная часть тика.
            //
            // Вокруг `main_stage` не должно быть ни захватов мьютексов пула, ни
            // построения дескрипторов, ни ожидания батча, ни финальной цепочки
            // контрольных сумм: всё это — стоимость измерительного устройства,
            // а не измеряемой работы. Такая константа одинакова для всех схем,
            // поэтому она не портит сравнение схем между собой, но СЖИМАЕТ
            // относительную разницу (реальные 5 % превращаются в меньшее), и
            // главное — делает шум синхронизации сравнимым с шумом вычислений.
            //
            // Общее время батча (`total_ns`) считается отдельно и идёт в
            // темп тиков и телеметрию: темп тиков — это скорость цикла, и он
            // обязан включать синхронизацию, иначе детектор провала и «стоп»
            // получили бы неверную величину.
            let tick_started = Instant::now();

            // 1. Main-стадия.
            let main_checksum = main_stage(&self.entities, &params);

            // Замер вычислительной части закрыт ДО любой синхронизации.
            let compute_elapsed = tick_started.elapsed();

            // 2. Диспетчер задач.
            let descriptors = build_descriptors(&params);
            self.pool
                .dispatch(params.worker_jobs, active_workers, &descriptors);

            // 3. Синхронизация (bounded, кооперативная отмена).
            // Потолок батча — сама фаза плюс запас на один зависший тик. Без
            // него зависший воркер держал сессию намертво.
            let batch_timeout = match target {
                RunTarget::Duration(d) => d + BATCH_TIMEOUT_MARGIN,
                RunTarget::Ticks(_) => TICK_BATCH_TIMEOUT,
            };
            self.pool.wait_batch(batch_timeout).map_err(|e| match e {
                crate::pool::BatchError::Cancelled => RunError::Cancelled,
                crate::pool::BatchError::WorkerFailed => RunError::WorkerFailed,
                crate::pool::BatchError::TimedOut => RunError::WorkerFailed,
            })?;

            // Общее время батча: вычисление + построение дескрипторов +
            // диспетчеризация + ожидание + снимок слотов + контрольные суммы.
            let total_elapsed = tick_started.elapsed();

            // 4. Финализация: объединение слотов по возрастанию номера задачи.
            let slots = self.pool.job_slots_snapshot(params.worker_jobs);
            let mut tick_checksum = mix(main_checksum, phase_index);
            for slot in slots.iter().take(params.worker_jobs) {
                tick_checksum = mix(tick_checksum, *slot);
            }
            tick_checksum = finalize_tick(tick_checksum, global_tick);
            run_checksum = mix(run_checksum, tick_checksum);
            if first_tick_checksum.is_none() {
                first_tick_checksum = Some(tick_checksum);
            }

            // Сэмпл — время вычислительной части, в миллисекундах.
            buffer.push(compute_elapsed.as_secs_f64() * 1000.0);

            let compute_ns = compute_elapsed.as_nanos() as u64;
            let total_ns = total_elapsed.as_nanos() as u64;
            self.scoreboard
                .ticks_done
                .store(global_tick + 1, Ordering::Relaxed);
            self.scoreboard
                .last_compute_ns
                .store(compute_ns, Ordering::Relaxed);
            self.scoreboard
                .last_total_ns
                .store(total_ns, Ordering::Relaxed);
            self.scoreboard
                .last_tick_ns
                .store(total_ns, Ordering::Relaxed);
            self.scoreboard
                .elapsed_secs
                .store(started.elapsed().as_secs(), Ordering::Relaxed);
            // Темп тиков — по ОБЩЕМУ времени батча, а не по вычислению: темп
            // характеризует скорость цикла, и подмена сделала бы детектор
            // провала нечувствительным к зависанию пула.
            let tps = 1_000_000_000u64.checked_div(total_ns).unwrap_or(0);
            self.scoreboard
                .current_ticks_per_sec
                .store(tps, Ordering::Relaxed);

            global_tick += 1;
            phase_index += 1;
        }

        Ok(RunReport {
            phase,
            ticks: global_tick,
            supercycles_completed: if phase == Phase::Response {
                phase_index / RESPONSE_SUPERCYCLE
            } else {
                0
            },
            first_tick_checksum: first_tick_checksum.unwrap_or(0),
            run_checksum,
            samples_written: buffer.len(),
        })
    }

    /// Обязательная самопроверка ядра при старте (результаты выбрасываются):
    /// двойной reset + короткий прогон для Light/Heavy/Response с равенством
    /// контрольных сумм; запоминаются эталонные суммы первого тика.
    pub fn self_check(&mut self) -> Result<(), RunError> {
        let mut references = [0u64; REFERENCE_SLOTS];
        // Все измеряемые фазы, а не «сложные» три: у фазы, для которой эталон не
        // заведён, детерминизм не проверяется нигде — и поломка в ней обнаружится
        // не на старте, а спустя минуты замера, когда агрегация отбросит все схемы
        // с «контрольная сумма различается между повторами».
        for phase in [Phase::Light, Phase::Partial, Phase::Heavy] {
            self.reset();
            let first = self.run_phase(phase, RunTarget::Ticks(SELF_CHECK_TICKS))?;
            self.reset();
            let second = self.run_phase(phase, RunTarget::Ticks(SELF_CHECK_TICKS))?;
            if first.first_tick_checksum != second.first_tick_checksum
                || first.run_checksum != second.run_checksum
            {
                return Err(RunError::DeterminismRecheckFailed);
            }
            references[phase.first_profile().index()] = first.first_tick_checksum;
        }

        // «Отклик»: один суперцикл (все классы тиков), двойной прогон.
        self.reset();
        let first = self.run_phase(Phase::Response, RunTarget::Ticks(RESPONSE_SUPERCYCLE))?;
        self.reset();
        let second = self.run_phase(Phase::Response, RunTarget::Ticks(RESPONSE_SUPERCYCLE))?;
        if first.first_tick_checksum != second.first_tick_checksum
            || first.run_checksum != second.run_checksum
        {
            return Err(RunError::DeterminismRecheckFailed);
        }
        references[Profile::ResponseBase.index()] = first.first_tick_checksum;

        self.first_tick_references = references;
        Ok(())
    }

    /// Эталон контрольной суммы первого тика профиля (после self_check).
    pub fn first_tick_reference(&self, profile: Profile) -> Option<u64> {
        let v = self.first_tick_references[profile.index()];
        if v == 0 { None } else { Some(v) }
    }

    /// Сверка фактической контрольной суммы первого тика с эталоном.
    pub fn verify_first_tick(&self, profile: Profile, actual: u64) -> bool {
        self.first_tick_reference(profile) == Some(actual)
    }

    /// Времена завершённых тиков последнего прогона в миллисекундах
    /// (`[0, len)` от записанных сэмплов; читается после `run_phase`).
    pub fn samples(&self) -> &[f64] {
        match &self.sample_buffer {
            Some(buffer) => buffer.as_slice(),
            None => &[],
        }
    }
}

/// Main-стадия: последовательная обработка сущностей в предвычисленном
/// порядке с ветвистыми переходами и накоплением контрольной суммы.
/// Сколько воркеров занято в фазе (1..=worker_count).
///
/// Для пула из одного воркера результат всё равно 1: парковать последний
/// некому, и «частичная нагрузка» выродилась бы в фазу без нагрузки вовсе.
fn active_workers_for(phase: Phase, worker_count: usize) -> usize {
    let pct = phase.active_worker_percent();
    let active = worker_count.saturating_mul(pct as usize) / 100;
    active.clamp(1, worker_count)
}

fn main_stage(entities: &RawShared<EntityBuffers>, params: &ProfileParams) -> u64 {
    let p = unsafe { &mut *entities.get() };
    let n = params.main_entity_updates.min(ENTITY_CAPACITY);
    let mut main_checksum = 0u64;
    for i in 0..n {
        let e = p.order[i] as usize;
        let f = p.flags[e];

        // Ветвистые переходы состояния (branch-heavy).
        let dir = if f & 1 != 0 { 1.0f64 } else { -1.0f64 };
        let vx_new = p.vx[e] * VELOCITY_DAMPING + dir * VELOCITY_STEP;
        let vy_new = match (f >> 1) & 3 {
            0 => p.vy[e] * VELOCITY_DAMPING,
            1 => p.vy[e] * VELOCITY_DAMPING - VELOCITY_STEP,
            2 => p.vy[e] * VELOCITY_DAMPING + VELOCITY_STEP,
            _ => {
                if f & 4 != 0 {
                    p.vy[e] * 1.000_5
                } else {
                    p.vy[e] * 0.998_5
                }
            }
        };
        let vz_new = {
            let base = p.vz[e] * VELOCITY_DAMPING;
            if f & 8 != 0 {
                base + 0.000_5
            } else {
                base - 0.000_5
            }
        };

        p.vx[e] = vx_new;
        p.vy[e] = vy_new;
        p.vz[e] = vz_new;
        // pos = WrapPosition(pos + vel * dt).
        p.x[e] = wrap_position(p.x[e] + vx_new * DT_SECONDS);
        p.y[e] = wrap_position(p.y[e] + vy_new * DT_SECONDS);
        p.z[e] = wrap_position(p.z[e] + vz_new * DT_SECONDS);

        // Ограниченная арифметика над флагами (ротация + соль).
        let f_new = f.rotate_left(1);
        let f_new = f_new ^ (e as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        p.flags[e] = f_new;

        main_checksum = mix(main_checksum, p.x[e].to_bits());
        main_checksum = mix(main_checksum, f_new ^ p.anim[e].to_bits());
        main_checksum = mix(main_checksum, i as u64);
    }
    main_checksum
}

/// Построить дескрипторы задач на стеке (без выделений в цикле тика).
///
/// Разбиение обязано оставаться непересекающимся: от него зависит
/// `unsafe impl Sync` в `buffers::RawShared`. Диагностические проверки собраны
/// в `debug_assert!` — в release их нет, поэтому инвариант проверяется тестами
/// `animation_ranges_are_disjoint` и `ranges_stay_in_bounds`, а не в рантайме.
fn build_descriptors(params: &ProfileParams) -> [JobDescriptor; MAXIMUM_JOBS] {
    let active = params.worker_jobs;
    debug_assert!((1..=MAXIMUM_JOBS).contains(&active));
    // Разбиение должно быть точным: неполный остаток молча потерял бы работу.
    debug_assert_eq!(params.visibility_probes % active, 0);
    debug_assert_eq!(params.animation_items % active, 0);
    let probes_per_job = params.visibility_probes / active;
    let anim_per_job = params.animation_items / active;
    // Смещения растут монотонно и равны шагу, поэтому диапазоны соседних задач
    // не пересекаются. В release эти проверки исчезают, а инвариант проверяется
    // тестами `animation_ranges_are_disjoint` / `ranges_stay_in_bounds`.
    let mut out = [JobDescriptor::dummy(); MAXIMUM_JOBS];
    for (j, slot) in out.iter_mut().enumerate().take(active) {
        *slot = JobDescriptor {
            job_index: j as u32,
            visibility_start: (j * probes_per_job) as u32,
            visibility_count: probes_per_job as u32,
            animation_start: (j * anim_per_job) as u32,
            animation_count: anim_per_job as u32,
        };
    }
    out
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;
    use crate::config::profile_params;

    /// Все профили нагрузки. Число воркеров варьируется, но только среди
    /// делителей объёмов работы: разбиение обязано быть точным, иначе часть
    /// работы просто потеряется (это же проверяет `build_descriptors`).
    fn all_profiles() -> Vec<ProfileParams> {
        crate::config::ALL_PROFILES
            .into_iter()
            .flat_map(|p| {
                let base = profile_params(p);
                // Все делители числа воркеров по умолчанию, плюс сам делитель:
                // так проверяются и «родные», и уменьшенные пулы.
                let div = base.worker_jobs;
                (1..=MAXIMUM_JOBS.min(div))
                    .filter(move |j| div.is_multiple_of(*j))
                    .map(move |jobs| ProfileParams {
                        main_entity_updates: base.main_entity_updates,
                        visibility_probes: base.visibility_probes,
                        animation_items: base.animation_items,
                        worker_jobs: jobs,
                    })
            })
            .collect()
    }

    /// Инвариант `RawShared`: ни один индекс анимации не принадлежит двум
    /// задачам. Нарушение здесь означает гонку записи в горячем тике ядра.
    #[test]
    fn animation_ranges_are_disjoint() {
        let _g = crate::tests::lock();
        for params in all_profiles() {
            let descs = build_descriptors(&params);
            let mut seen = vec![0u8; ENTITY_CAPACITY];
            for d in descs.iter().take(params.worker_jobs) {
                let start = d.animation_start as usize;
                let end = start + d.animation_count as usize;
                for slot in &mut seen[start..end.min(ENTITY_CAPACITY)] {
                    *slot = slot.saturating_add(1);
                }
            }
            let total: usize = descs
                .iter()
                .take(params.worker_jobs)
                .map(|d| d.animation_count as usize)
                .sum();
            assert_eq!(
                total, params.animation_items,
                "{params:?}: не все элементы анимации покрыты"
            );
            assert!(
                seen.iter().all(|&c| c <= 1),
                "{params:?}: диапазоны анимации пересеклись"
            );
        }
    }

    /// Границы диапазонов внутри буфера: выход за них в `run_job` — это
    /// обращение за пределы выделенной памяти.
    #[test]
    fn ranges_stay_in_bounds() {
        let _g = crate::tests::lock();
        for params in all_profiles() {
            for d in build_descriptors(&params).iter().take(params.worker_jobs) {
                let a_end = d.animation_start as usize + d.animation_count as usize;
                assert!(
                    a_end <= ENTITY_CAPACITY,
                    "{params:?}: диапазон анимации выходит за {ENTITY_CAPACITY}"
                );
                let v_end = d.visibility_start as usize + d.visibility_count as usize;
                assert!(
                    v_end <= ENTITY_CAPACITY,
                    "{params:?}: диапазон проб видимости выходит за {ENTITY_CAPACITY}"
                );
            }
        }
    }
}
