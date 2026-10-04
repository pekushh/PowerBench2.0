//! Persistent-пул воркеров и совместная работа над задачами тика.
//!
//! Контракты спецификации:
//! - пул живёт весь запуск; задачи тика не создают потоков;
//! - каждый воркер получает ровно один сигнал пробуждения на пакет задач
//!   (быстрый воркер не «съедает» чужой сигнал);
//! - ожидание главного потока ограниченное, с кооперативной отменой;
//! - каждую задачу j исполняет ровно один воркер, записывая контрольную сумму
//!   только в свой слот j;
//! - остановка кооперативная и по времени ограниченная.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::buffers::{EntityBuffers, RawShared};
use crate::checksum::mix;
use crate::config::{ANIMATION_STEP, MAXIMUM_JOBS, VISIBILITY_MIX_CONSTANT};
use crate::prng::unit_bits;
use crate::topology::CpuTopology;

/// Константа смешивания проб видимости (из спецификации).
/// Период опроса при ожидании пробуждения.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Грейс-период кооперативной отмены: ограничивает время выхода из фазы.
const CANCEL_GRACE: Duration = Duration::from_millis(100);

/// Потолок ожидания «тишины» пула (все воркеры завершили батч). Защищает от
/// бесконечного ожидания, если воркер завис намертво.
const QUIESCE_TIMEOUT: Duration = Duration::from_secs(5);

/// Период опроса при ожидании тишины пула.
const QUIESCE_POLL: Duration = Duration::from_millis(1);

/// Дескриптор одной задачи воркерам.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobDescriptor {
    pub job_index: u32,
    pub visibility_start: u32,
    pub visibility_count: u32,
    pub animation_start: u32,
    pub animation_count: u32,
}

impl JobDescriptor {
    /// Пустой дескриптор (для предзаполненных массивов).
    pub const fn dummy() -> Self {
        Self {
            job_index: 0,
            visibility_start: 0,
            visibility_count: 0,
            animation_start: 0,
            animation_count: 0,
        }
    }
}

#[derive(Debug)]
struct PoolInner {
    /// Номер батча; инкрементируется при каждой публикации задач.
    epoch: u64,
    /// Размер пула: столько отчётов собирает батч, если никто не ушёл.
    worker_count: usize,
    descriptors: [JobDescriptor; MAXIMUM_JOBS],
    job_slots: [u64; MAXIMUM_JOBS],
    active_jobs: usize,
    /// Сколько воркеров реально берут задачи в текущем батче (1..=worker_count).
    ///
    /// Меньше `worker_count` — это фаза частичной нагрузки: вторая половина
    /// пула «припаркована». Припаркованный воркер не берёт задач, но обязан
    /// отчитаться о завершении батча, иначе ожидание в `wait_batch` не
    /// закончится. Поэтому счётчик `workers_done` считает **все** воркеры, и
    /// условие завершения батча не меняется.
    active_workers: usize,
    /// Сколько воркеров отчитались о завершении **текущей** эпохи.
    /// Отчёты по устаревшим эпохам игнорируются (см. `worker_loop`).
    workers_done: usize,
    /// Отказ одного из воркеров (падение потока) — ошибка запуска.
    faulted: bool,
    /// Номер эпохи, для которой воркеры уже сообщили о завершении.
    /// Позволяет отбрасывать опоздавшие отчёты и проверять «тишину» пула.
    completed_epoch: u64,
    /// Есть ли батч, который воркеры ещё не завершили. Именно этот признак
    /// отвечает на вопрос «никто не пишет в буферы», а не счётчик отчётов:
    /// после `reset_to_idle` счётчик обнуляется, но батч мог быть незавершённым.
    busy: bool,
    /// Поток воркера, который ушёл навсегда (упал вне `catch_unwind`).
    ///
    /// Требовать отчёт от потока, которого больше нет, — значит ждать батч до
    /// потолка и не снимать `busy` никогда. Такое случалось при панике вне
    /// участка, накрытого `catch_unwind`: воркер выходил, счётчик живых падал,
    /// а условие завершения батча оставалось прежним, и пул залипал навсегда —
    /// `wait_batch` всегда отдавал `TimedOut`, а `reset` не мог снять `busy`.
    lost_workers: usize,
}

/// Мьютекс пула: отравление игнорируем — потеря блокировки в одном воркере
/// не должна превращать панику в каскадную по всему пулу.
fn lock_pool(shared: &PoolShared) -> std::sync::MutexGuard<'_, PoolInner> {
    shared.lock.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug)]
struct PoolShared {
    lock: Mutex<PoolInner>,
    /// main -> воркеры: «есть новый батч».
    work_ready: Condvar,
    /// воркеры -> main: «мы завершили батч».
    work_done: Condvar,
}

/// Ошибка завершения батча.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchError {
    Cancelled,
    WorkerFailed,
    /// Батч не уложился в отведённое время: воркер завис (или ушёл в
    /// непрерываемый системный вызов) и `wait_batch` больше не ждёт вечно.
    TimedOut,
}

/// Persistent-пул воркеров.
#[derive(Debug)]
pub struct Pool {
    shared: Arc<PoolShared>,
    handles: Vec<JoinHandle<()>>,
    /// Флаг отмены текущей фазы (кооперативный).
    cancel: Arc<AtomicBool>,
    /// Флаг полной остановки пула.
    shutdown: Arc<AtomicBool>,
    /// Сколько воркеров ещё не вышло из цикла. `shutdown` ждёт по этому
    /// счётчику, а не по `join`, чтобы зависший поток не держал выключение.
    live_workers: Arc<AtomicUsize>,
    worker_count: usize,
    /// Воркеры, которым ОС отказала в маске привязки (пусто — привязка
    /// поставлена всем или выключена).
    affinity_failures: Vec<String>,
}

/// Снимает счётчик живых воркеров при любом выходе из цикла, включая панику.
struct LiveWorkerGuard {
    counter: Arc<AtomicUsize>,
    shared: Arc<PoolShared>,
    /// Отчёт о завершении батча перед выходом. Воркер может уйти и с паникой
    /// вне `catch_unwind`; без этого отчёта пул ждал бы его вечно.
    epoch: Option<u64>,
}

impl Drop for LiveWorkerGuard {
    fn drop(&mut self) {
        decrement_live(&self.counter);
        let Some(epoch) = self.epoch else {
            // Воркер ни разу не брал батч: зачитывать нечего, и пул он не блокирует.
            return;
        };
        let mut guard = lock_pool(&self.shared);
        // Воркер ушёл навсегда: требовать от него отчёт больше нельзя.
        guard.lost_workers = guard.lost_workers.saturating_add(1);
        if guard.epoch == epoch {
            report_done(&mut guard, epoch);
        }
    }
}

/// Засчитать отчёт воркера: счётчик, эпоха и снятие `busy` при полном сборе.
///
/// Общая точка для обычного пути в `worker_loop` и для аварийного выхода, иначе
/// два места разошлись бы при любой правке.
fn report_done(guard: &mut PoolInner, epoch: u64) {
    guard.workers_done = guard.workers_done.saturating_add(1);
    guard.completed_epoch = epoch;
    if guard.workers_done >= expected_reports(guard) {
        guard.busy = false;
    }
}

/// Сколько отчётов ДОЛЖЕН собрать батч: все воркеры, кроме ушедших навсегда.
///
/// Живой воркер отчитается всегда. Ушедший — никогда, и требование от него
/// отчёта оставляло `busy` поднятым навсегда: `reset` не мог опустить флаг,
/// `wait_batch` упирался в потолок, а движок отказывался мерить.
fn expected_reports(inner: &PoolInner) -> usize {
    inner
        .worker_count
        .saturating_sub(inner.lost_workers)
        .max(1)
}

/// Уменьшить счётчик живых воркеров, не уходя ниже нуля.
///
/// Регресс H12: раньше стоял голый `fetch_sub`. Счётчик наращивался ПОСЛЕ
/// `spawn`, поэтому воркер, успевший выйти (а он выходит сразу, если флаг
/// остановки уже поднят), вычитал из нуля и получал `usize::MAX`. Дальше
/// `shutdown` ждал бы этого «живого» воркера все `QUIESCE_TIMEOUT`, а
/// `JoinHandle` отпускались бы отсоединёнными — то есть потоки, которые
/// на самом деле мертвы, считались живыми.
///
/// Вычитание через `compare_exchange` вместо `saturating_sub`: у
/// `AtomicUsize` нет такой операции, а `fetch_update` с замыканием здесь
/// проще и выражает ту же гарантию — «вычесть, только если есть что вычесть».
fn decrement_live(counter: &AtomicUsize) {
    let mut current = counter.load(Ordering::Acquire);
    while current != 0 {
        match counter.compare_exchange_weak(
            current,
            current - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

/// Выполнить порцию работы задачи j и вернуть локальную контрольную сумму.
///
/// # Безопасность
///
/// Воркер читает порядок обхода и глубину (во время тика их никто не меняет)
/// и пишет только диапазон анимации, назначенный задаче j (диапазоны разных
/// задач не пересекаются по записи).
unsafe fn run_job(desc: JobDescriptor, entities: &RawShared<EntityBuffers>) -> u64 {
    let p = unsafe { &mut *entities.get() };
    let mut c: u64 = 0;

    // Пробы видимости: только чтение по предвычисленному порядку.
    let start = desc.visibility_start as usize;
    let count = desc.visibility_count as usize;
    for t in 0..count {
        let idx = p.order[start + t] as usize;
        let dval = p.depth[idx];
        let bits = unit_bits(dval.to_bits());
        c = mix(c, bits).wrapping_mul(VISIBILITY_MIX_CONSTANT);
        // Ветвистая фильтрация видимости.
        if dval > 0.5 {
            c = c.rotate_left(7) ^ VISIBILITY_MIX_CONSTANT ^ bits;
        } else {
            c = c.rotate_right(13);
        }
    }

    // Обновление анимации в выделенном диапазоне буфера.
    let astart = desc.animation_start as usize;
    let acount = desc.animation_count as usize;
    for t in 0..acount {
        let k = astart + t;
        let a = p.anim[k];
        let na = a + ANIMATION_STEP;
        p.anim[k] = if na > 1.0 { na - 1.0 } else { na };
        c = mix(c, p.anim[k].to_bits());
    }

    c
}

fn process_jobs(
    shared: &Arc<PoolShared>,
    cancel: &AtomicBool,
    worker_index: usize,
    worker_count: usize,
    epoch: u64,
    entities: &Arc<RawShared<EntityBuffers>>,
) {
    debug_assert!(worker_count >= 1);
    // Снимок дескрипторов своей части задач (стек, без выделений).
    let mut my_indices: [usize; MAXIMUM_JOBS] = [0; MAXIMUM_JOBS];
    let mut my_descs: [JobDescriptor; MAXIMUM_JOBS] = [JobDescriptor::dummy(); MAXIMUM_JOBS];
    let mut n = 0usize;
    {
        let guard = lock_pool(shared);
        // Фаза частичной нагрузки: припаркованный воркер не берёт задач, но
        // отчёт о завершении батча всё равно отправит — иначе main ждёт вечно.
        if worker_index >= guard.active_workers {
            return;
        }
        debug_assert_eq!(guard.epoch, epoch, "воркер обработал не тот батч");
        let active = guard.active_jobs;
        // Шаг раздачи — по ЧИСЛУ АКТИВНЫХ воркеров, а не по числу воркеров пула.
        // Иначе в фазе частичной нагрузки задачи с индексами ≥ active_workers
        // не достались бы никому: их слоты сохраняли бы значения прошлой фазы,
        // и контрольная сумма тика менялась бы от прогона к прогону. Агрегация
        // по раундам после этого отбрасывала все схемы целиком.
        let step = guard.active_workers;
        debug_assert!(step >= 1);
        let mut j = worker_index;
        while j < active {
            debug_assert!(n < MAXIMUM_JOBS);
            my_indices[n] = j;
            my_descs[n] = guard.descriptors[j];
            n += 1;
            j += step;
        }
    }

    // Локальная копия слотов перед единичной записью в общий массив.
    let mut my_slots: [u64; MAXIMUM_JOBS] = [0; MAXIMUM_JOBS];
    let mut done = 0usize;
    for k in 0..n {
        if cancel.load(Ordering::Relaxed) {
            break; // кооперативный выход из текущего батча
        }
        my_slots[k] = unsafe { run_job(my_descs[k], entities) };
        done += 1;
    }

    if done > 0 {
        let mut guard = lock_pool(shared);
        // Публикуем только реально посчитанные слоты. При кооперативном выходе
        // хвост остался бы нулевым, и нулевое значение попало бы в контрольную
        // сумму как результат работы, которой не было.
        for k in 0..done {
            guard.job_slots[my_indices[k]] = my_slots[k];
        }
    }
}

/// Что воркеру нужно для привязки к ядру: топология и канал отчёта.
type AffinityBind = (Arc<CpuTopology>, mpsc::Sender<(usize, Result<(), String>)>);

// Сигнатура — полный перечень того, что нужно потоку воркера; группировать
// ради счётчика аргументов значило бы спрятать зависимости в структуру.
#[allow(clippy::too_many_arguments)]
fn worker_loop(
    shared: Arc<PoolShared>,
    cancel: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    live_workers: Arc<AtomicUsize>,
    worker_index: usize,
    worker_count: usize,
    entities: Arc<RawShared<EntityBuffers>>,
    affinity: Option<AffinityBind>,
) {
    let mut live = LiveWorkerGuard {
        counter: live_workers,
        shared: shared.clone(),
        epoch: None,
    };
    // Привязка — первым делом в потоке и именно здесь, а не до `spawn`:
    // маска принадлежит потоку, и поставленная до создания наследуется ВСЕМИ
    // новыми потоками — то есть дала бы всем воркерам одно и то же ядро.
    if let Some((topology, tx)) = affinity {
        let outcome = topology.bind_current_thread(worker_index);
        let _ = tx.send((worker_index, outcome));
    }
    // Сентинел, отличный от любой эпохи: воркер ждёт первого настоящего батча.
    let mut last_epoch = u64::MAX;

    loop {
        // 1) Ожидание нового батча (ровно один сигнал на пакет задач).
        let epoch;
        {
            let mut guard = lock_pool(&shared);
            loop {
                if shutdown.load(Ordering::Acquire) {
                    return;
                }
                // `busy` обязателен: без него воркер при старте считал бы
                // эпоху 0 «новым батчем», отработал бы ноль задач и засчитал
                // себе «done». Тогда `wait_batch` возвращал бы `Ok` до того,
                // как `dispatch` вообще что-то раздал, — успех без единой
                // выполненной работы.
                if guard.busy && guard.epoch != last_epoch {
                    epoch = guard.epoch;
                    last_epoch = guard.epoch;
                    break;
                }
                let waited = shared
                    .work_ready
                    .wait_timeout(guard, POLL_INTERVAL)
                    .unwrap_or_else(|e| e.into_inner());
                guard = waited.0;
            }
        }
        // С этого момента воркер обязан отчитаться при любом выходе, включая
        // панику вне `catch_unwind`: иначе батч остался бы незавершённым, а
        // требование отчёта от ушедшего воркера заклинило бы пул навсегда.
        live.epoch = Some(epoch);

        // 2) Обработка своей части батча (падение воркера = ошибка запуска).
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_jobs(
                &shared,
                &cancel,
                worker_index,
                worker_count,
                epoch,
                &entities,
            )
        }));

        // 3) Сообщить о завершении порции.
        let mut guard = lock_pool(&shared);
        if caught.is_err() {
            guard.faulted = true;
        }
        // Отчёт принимается только для текущей эпохи. Иначе опоздавший воркер
        // (например, после истечения грейса отмены) зачёлся бы в следующем
        // батче, и `wait_batch` вернул бы `Ok` до реального завершения работы —
        // сломанные контрольные суммы на живых данных.
        if guard.epoch == epoch {
            report_done(&mut guard, epoch);
        }
        live.epoch = None;
        // Сигнал — ПОСЛЕ отпускания мьютекса. Notify под блокировкой будит
        // ожидающих, которые тут же упрутся в тот же мьютекс: лишнее
        // пробуждение и лишнее переключение контекста в самом горячем цикле.
        drop(guard);
        shared.work_done.notify_all();
    }
}

impl Pool {
    /// Создать пул из `worker_count` persistent-потоков без привязки к ядрам.
    ///
    /// Используется тестами и путями, где привязка не нужна. Боевой путь —
    /// [`Pool::new_with_affinity`].
    ///
    /// `worker_count == 0` — ошибка конфигурации, а не «пустой пул»: раньше
    /// это приводило к панике в горячем потоке. Если поток запустить не удалось,
    /// уже запущенные останавливаются, иначе они остались бы жить вечно.
    pub fn new(worker_count: usize, entities: Arc<RawShared<EntityBuffers>>) -> Self {
        Self::new_with_affinity(worker_count, entities, None)
    }

    /// Создать пул, привязав воркер `i` к ядру `topology.slot(i)`.
    ///
    /// Отказ ОС в маске не является ошибкой запуска: замер продолжается в
    /// обычном режиме, а неудачные привязки возвращаются в
    /// [`Pool::affinity_failures`] и попадают в журнал сессии. Молча пропустить
    /// их нельзя — тогда в отчёте появился бы замер, выполненный на
    /// произвольных ядрах, помеченный как привязанный.
    pub fn new_with_affinity(
        worker_count: usize,
        entities: Arc<RawShared<EntityBuffers>>,
        topology: Option<&CpuTopology>,
    ) -> Self {
        assert!(worker_count >= 1, "пул воркеров не может быть пустым");
        let shared = Arc::new(PoolShared {
            lock: Mutex::new(PoolInner {
                epoch: 0,
                worker_count,
                descriptors: [JobDescriptor::dummy(); MAXIMUM_JOBS],
                job_slots: [0u64; MAXIMUM_JOBS],
                active_jobs: 0,
                active_workers: 1,
                workers_done: 0,
                faulted: false,
                completed_epoch: 0,
                busy: false,
                lost_workers: 0,
            }),
            work_ready: Condvar::new(),
            work_done: Condvar::new(),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        // Счётчик растёт по мере успешного запуска, иначе при частичном
        // старте он навсегда остался бы больше числа живых потоков.
        let live_workers = Arc::new(AtomicUsize::new(0));
        // Канал только для отчёта о привязке: каждый воркер шлёт ровно одно
        // сообщение сразу при старте, до входа в цикл.
        let (bind_tx, bind_rx, bind_topology) = match topology {
            Some(t) => {
                let (tx, rx) = mpsc::channel();
                (Some(tx), Some(rx), Some(Arc::new(t.clone())))
            }
            None => (None, None, None),
        };

        let mut handles = Vec::with_capacity(worker_count);
        for w in 0..worker_count {
            // Копии для остановки пула на случай частичного запуска: originals
            // уйдут в замыкание потока, и после `spawn` их уже не достать.
            let stop_flag = shutdown.clone();
            let stop_cancel = cancel.clone();
            let stop_shared = shared.clone();
            let shared = shared.clone();
            let cancel = cancel.clone();
            let shutdown = shutdown.clone();
            let live_for_worker = live_workers.clone();
            let entities = entities.clone();
            let affinity = match (&bind_topology, &bind_tx) {
                (Some(t), Some(tx)) => Some((Arc::clone(t), tx.clone())),
                _ => None,
            };
            // Счётчик наращивается ДО `spawn`, а не после. Иначе воркер, который
            // сразу увидит поднятый флаг остановки и выйдет, вычтет из нуля
            // (регресс H12). Теперь каждый запущенный воркер заранее «держит»
            // единицу, и `LiveWorkerGuard` гарантированно вычитает свою.
            live_workers.fetch_add(1, Ordering::AcqRel);
            let spawned = std::thread::Builder::new()
                .name(format!("powerbench-worker-{w}"))
                .spawn(move || {
                    worker_loop(
                        shared,
                        cancel,
                        shutdown,
                        live_for_worker,
                        w,
                        worker_count,
                        entities,
                        affinity,
                    )
                });
            match spawned {
                Ok(handle) => {
                    handles.push(handle);
                }
                Err(e) => {
                    // Поток не создан — единицу счётчика возвращаем, иначе пул
                    // ждал бы воркера, которого не существует.
                    decrement_live(&live_workers);
                    // Частичный пул недопустим: будим и ждём уже созданные,
                    // иначе они остались бы жить вечно (дескрипторы `JoinHandle`
                    // в этом ветке не сохраняются и потоки отсоединяются).
                    stop_flag.store(true, Ordering::Release);
                    stop_cancel.store(true, Ordering::Release);
                    stop_shared.work_ready.notify_all();
                    for h in handles {
                        let _ = h.join();
                    }
                    panic!("не удалось запустить воркер {w}: {e}");
                }
            }
        }

        // Собираем отчёты о привязке. Воркер шлёт их до входа в цикл, поэтому
        // ожидание короткое; потолок нужен лишь на случай, если поток не
        // стартовал вовсе.
        let mut affinity_failures = Vec::new();
        if let Some(rx) = bind_rx {
            let deadline = Instant::now() + Duration::from_secs(5);
            for _ in 0..worker_count {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    affinity_failures.push("воркер не отчитался о привязке за 5 с".to_string());
                    break;
                }
                match rx.recv_timeout(left) {
                    Ok((idx, Ok(()))) => {
                        debug_assert!(idx < worker_count, "отчёт о привязке от чужого воркера");
                    }
                    Ok((idx, Err(e))) => {
                        affinity_failures.push(format!("воркер {idx}: {e}"));
                    }
                    Err(_) => {
                        affinity_failures.push("отчёт о привязке не получен".to_string());
                        break;
                    }
                }
            }
        }

        Self {
            shared,
            handles,
            cancel,
            shutdown,
            live_workers,
            worker_count,
            affinity_failures,
        }
    }

    /// Воркеры, которым ОС отказала в маске привязки. Пусто — привязка
    /// поставлена всем воркерам либо выключена.
    pub fn affinity_failures(&self) -> &[String] {
        &self.affinity_failures
    }

    /// Сколько воркеров ещё работает (для диагностики и тестов остановки).
    pub fn live_workers(&self) -> usize {
        self.live_workers.load(Ordering::Acquire)
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    /// Отменить текущую фазу (кооперативно).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// Разделяемый флаг отмены для внешнего потока.
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// Текущее состояние флага отмены.
    pub fn canceled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    /// Снять отмену (перед новым прогоном / при reset).
    pub fn clear_cancel(&self) {
        self.cancel.store(false, Ordering::Release);
    }

/// Пул в idle перед фазой: батча нет, счётчик завершившихся воркеров нулевой.
///
/// Сначала дожидается «тишины» пула: если грейс отмены истёк, воркеры могут
/// ещё писать в буферы сущностей. Сбрасывать буферы в этот момент — гонка
/// памяти и рассинхрон контрольных сумм, поэтому сначала quiesce.
///
/// Регресс C2: при незатихшем пуле обнулялся `workers_done`, а `busy` и
/// `epoch` оставались прежними. Дальше счётчик уже не мог достичь
/// `worker_count` — все отчёты-то пришли, — поэтому `busy` не снимался
/// никогда: `is_quiesced()` врал, каждая следующая `quiesce()` выжигала полные
/// `QUIESCE_TIMEOUT`, `Engine::reset()` возвращал `false`, и пул оказывался
/// заблокирован навсегда.
///
/// Поэтому состояние сбрасывается **только при подтверждённой тишине** и
/// согласованно: `epoch` сдвигается (опоздавший отчёт будет отброшен, но при
/// тихом пуле его и быть не может), `completed_epoch` догоняет `epoch`,
/// `workers_done` обнуляется, `busy` снимается. Если пул НЕ затих, ни `busy`,
/// ни `workers_done`, ни `epoch` не трогаем: оставшийся воркер обязан суметь
/// дочитать свой батч и снять `busy` своим отчётом, а сброс счётчика сделал
/// бы это невозможным и вернул вечную блокировку.
pub fn reset_to_idle(&self) -> bool {
    let quiet = self.quiesce();
    let mut guard = lock_pool(&self.shared);
    guard.active_jobs = 0;
    guard.active_workers = 1;
    guard.faulted = false;
    if quiet {
        guard.epoch = guard.epoch.wrapping_add(1);
        guard.completed_epoch = guard.epoch;
        guard.workers_done = 0;
        guard.busy = false;
    }
    quiet
}

    /// Дождаться, пока все воркеры завершат текущий батч.
    ///
    /// Вызывается перед `reset` (буферы сущностей перестают быть общими) и
    /// перед `shutdown`. Отмена кооперативная, поэтому воркер завершается
    /// быстро; ожидание ограничено сверху, чтобы зависший воркер не заблокировал
    /// поток навсегда.
    pub fn quiesce(&self) -> bool {
        let deadline = Instant::now() + QUIESCE_TIMEOUT;
        loop {
            if self.is_quiesced() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(QUIESCE_POLL);
        }
    }

    /// Пул гарантированно не работает: ни один воркер не пишет в буферы.
    pub fn is_quiesced(&self) -> bool {
        !lock_pool(&self.shared).busy
    }

    /// Опубликовать батч из `active_jobs` задач и разбудить воркеров.
    ///
    /// `active_workers` — сколько воркеров берут задачи (1..=worker_count).
    /// Значение меньше полного даёт фазу частичной нагрузки.
    pub fn dispatch(
        &self,
        active_jobs: usize,
        active_workers: usize,
        descriptors: &[JobDescriptor],
    ) {
        assert!((1..=MAXIMUM_JOBS).contains(&active_jobs));
        assert!((1..=self.worker_count).contains(&active_workers));
        // Короткий слайс молча оставил бы stale-дескрипторы прошлого батча.
        assert!(descriptors.len() >= active_jobs);
        let mut guard = lock_pool(&self.shared);
        guard.epoch = guard.epoch.wrapping_add(1);
        guard.workers_done = 0;
        guard.faulted = false;
        guard.active_jobs = active_jobs;
        guard.active_workers = active_workers;
        guard.busy = true;
        for (i, d) in descriptors.iter().enumerate().take(active_jobs) {
            guard.descriptors[i] = *d;
        }
        drop(guard);
        self.shared.work_ready.notify_all();
    }

    /// Снимок слотов контрольных сумм задач по возрастанию номера задачи.
    pub fn job_slots_snapshot(&self, count: usize) -> [u64; MAXIMUM_JOBS] {
        let mut out = [0u64; MAXIMUM_JOBS];
        let guard = lock_pool(&self.shared);
        out[..count].copy_from_slice(&guard.job_slots[..count]);
        out
    }

    /// Ограниченное ожидание завершения батча с кооперативной отменой.
    ///
    /// При отмене воркеры прекращают работу над текущим батчем, но main
    /// дожидается их явного «done» в пределах грейс-периода, после чего
    /// возвращается `Err(Cancelled)` (пул при этом вновь готов к работе).
    ///
    /// `timeout` — потолок на весь батч. Раньше его не было вовсе, и зависший
    /// воркер держал приложение намертво, хотя документация обещала
    /// ограниченное ожидание.
    pub fn wait_batch(&self, timeout: Duration) -> Result<(), BatchError> {
        let deadline = Instant::now() + timeout;
        let mut grace_start: Option<Instant> = None;
        loop {
            if self.cancel.load(Ordering::Acquire) && grace_start.is_none() {
                grace_start = Some(Instant::now());
            }

            let mut guard = lock_pool(&self.shared);
            // Ждём отчётов ЖИВЫХ воркеров. Ушедший не придёт никогда, и
            // требование отчёта от него означало бы, что батч не завершится
            // никогда: `wait_batch` всегда отдавал бы `TimedOut`, а `busy`
            // оставалось бы поднятым, заклинивая пул (регресс H13/H14).
            let expected = expected_reports(&guard);
            if guard.faulted && guard.workers_done >= expected {
                return Err(BatchError::WorkerFailed);
            }
            if guard.workers_done >= expected {
                return if grace_start.is_some() {
                    Err(BatchError::Cancelled)
                } else {
                    Ok(())
                };
            }
            if grace_start.is_some_and(|start| start.elapsed() > CANCEL_GRACE) {
                return Err(BatchError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(BatchError::TimedOut);
            }
            let waited = self
                .shared
                .work_done
                .wait_timeout(guard, POLL_INTERVAL)
                .unwrap_or_else(|e| e.into_inner());
            guard = waited.0;
        }
    }

    /// Пометить пул занятым батчем, который воркеры НЕ возьмут.
    ///
    /// Заставить поток воркера зависнуть в тесте нечем, а состояние «батч не
    /// завершён, один воркер ещё не отчитался» — это ровно то, из-за чего пул
    /// залипал (регресс C2). Состояние выставляется напрямую.
    ///
    /// Эпоха — `u64::MAX`, то есть значение, которое воркер уже видел как
    /// «свою последнюю»: батч он не возьмёт, и тест не соревнуется с живыми
    /// потоками. Отчёт о завершении добавляется вручную вызовом
    /// `report_done` с той же эпохой.
    #[cfg(test)]
    pub(crate) fn force_busy_for_test(&self, workers_done: usize) {
        let mut guard = lock_pool(&self.shared);
        guard.epoch = u64::MAX;
        guard.workers_done = workers_done;
        guard.busy = true;
    }

    /// Снимок внутреннего состояния для проверок инварианта.
    #[cfg(test)]
    pub(crate) fn inner_for_test(&self) -> (usize, usize, bool) {
        let guard = lock_pool(&self.shared);
        (
            guard.workers_done,
            expected_reports(&guard),
            guard.busy,
        )
    }

    /// Пометить воркера ушедшим навсегда (тестовая замена панике вне
    /// `catch_unwind`).
    #[cfg(test)]
    pub(crate) fn mark_worker_lost_for_test(&self, count: usize) {
        lock_pool(&self.shared).lost_workers = count;
    }

    /// Добавить отчёт воркера от имени ушедшего (см. `force_busy_for_test`).
    #[cfg(test)]
    pub(crate) fn report_done_for_test(&self) {
        let mut guard = lock_pool(&self.shared);
        report_done(&mut guard, u64::MAX);
    }

    /// Кооперативная, ограниченная по времени остановка пула.
    ///
    /// `join` ждал воркеры без потолка, поэтому зависший поток навсегда
    /// блокировал и `Drop`, и закрытие приложения. Теперь сначала ждём
    /// выхода всех воркеров (не дольше `QUIESCE_TIMEOUT`), и только
    /// успевшие дожидаются `join`. Оставшиеся отсоединяются: у них своя
    /// ссылка на `Arc<PoolShared>` и `Arc<RawShared<EntityBuffers>>`, так что
    /// освобождение памяти безопасно, а зависание приложения — нет.
    pub fn shutdown(&mut self) {
        if self.shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancel.store(true, Ordering::Release);
        self.shared.work_ready.notify_all();

        let deadline = Instant::now() + QUIESCE_TIMEOUT;
        while self.live_workers.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            std::thread::sleep(QUIESCE_POLL);
        }
        let all_exited = self.live_workers.load(Ordering::Acquire) == 0;

        let handles = std::mem::take(&mut self.handles);
        for h in handles {
            if all_exited {
                let _ = h.join();
            }
            // Иначе поток не завершился — `JoinHandle` отпускаем, не блокируя
            // выключение приложения.
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffers::{EntityBuffers, RawShared};
    use crate::config::SEED;

    fn pool(workers: usize) -> Pool {
        let entities = Arc::new(RawShared::new(EntityBuffers::from_seed(SEED)));
        Pool::new(workers, entities)
    }

    /// Батч, который никто не раздавал, обязан уложиться в потолок ожидания:
    /// раньше `wait_batch` ждал вечно и приложение висело намертво.
    #[test]
    fn wait_batch_times_out_instead_of_hanging_forever() {
        let _g = crate::tests::lock();
        let p = pool(2);
        let started = Instant::now();
        // База данных пула после создания: `workers_done == 0`, батча не было.
        let err = p
            .wait_batch(Duration::from_millis(50))
            .expect_err("батча не было — ждать нечего");
        assert_eq!(err, BatchError::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "ожидание должно упираться в потолок, а не в реальное время"
        );
    }

    /// Обычный батч завершается успешно и укладывается в отведённое время.
    #[test]
    fn wait_batch_reports_completion() {
        let _g = crate::tests::lock();
        let p = pool(2);
        p.clear_cancel();
        let descriptors = [JobDescriptor::dummy(); 4];
        p.dispatch(4, 2, &descriptors);
        assert_eq!(
            p.wait_batch(Duration::from_secs(10)),
            Ok(()),
            "воркеры обязаны отчитаться о батче"
        );
    }

    /// Остановка пула освобождает все потоки: счётчик живых доходит до нуля.
    #[test]
    fn shutdown_releases_all_workers() {
        let _g = crate::tests::lock();
        let mut p = pool(3);
        assert_eq!(p.live_workers(), 3);
        p.shutdown();
        assert_eq!(p.live_workers(), 0, "потоки остались жить после shutdown");
        // Повторный вызов обязан быть безопасным (вызывается из `Drop`).
        p.shutdown();
    }

    /// Пустой пул не считается «выполнившим батч»: воркеры не должны
    /// отрабатывать эпоху, которую никто не раздавал. Раньше каждый воркер
    /// при старте считал эпоху 0 своим батчем и засчитывал ноль задач как
    /// выполненные, из-за чего ожидание могло вернуть успех без работы.
    #[test]
    fn no_batch_means_no_completion() {
        let _g = crate::tests::lock();
        let p = pool(3);
        assert!(p.is_quiesced(), "свежий пул ничего не делает");
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            p.is_quiesced(),
            "без `dispatch` пул обязан оставаться тихим"
        );
        assert_eq!(
            p.wait_batch(Duration::from_millis(20)),
            Err(BatchError::TimedOut),
            "неразданный батч не может считаться выполненным"
        );
    }

    // ------------------------------------------------------------------
    // Регрессы C2 / H12 / H13 / H14
    // ------------------------------------------------------------------

    /// Счётчик живых воркеров не уходит под ноль (регресс H12).
    ///
    /// Раньше стоял голый `fetch_sub`, а счётчик наращивался уже после `spawn`.
    /// Воркер, успевавший выйти до `fetch_add`, вычитал из нуля и получал
    /// `usize::MAX` — после чего `shutdown` ждал бы этого «живого» воркера
    /// все `QUIESCE_TIMEOUT`, а потоки отпускались отсоединёнными.
    #[test]
    fn live_worker_counter_never_goes_below_zero() {
        let _g = crate::tests::lock();
        let counter = AtomicUsize::new(0);
        // Именно тот порядок, который и вызывал переполнение: сначала вычет.
        decrement_live(&counter);
        assert_eq!(counter.load(Ordering::Acquire), 0, "счётчик ушёл в минус");
        // Счётчик в ноль — тоже не повод вычитать.
        for _ in 0..5 {
            decrement_live(&counter);
        }
        assert_eq!(counter.load(Ordering::Acquire), 0);
        // Штатная работа счётчика.
        counter.fetch_add(3, Ordering::AcqRel);
        decrement_live(&counter);
        decrement_live(&counter);
        assert_eq!(counter.load(Ordering::Acquire), 1);
    }

    /// Многократные циклы «раздать батч → дождаться → сбросить» обязаны
    /// оставлять пул пригодным: `busy` обязана сниматься, а счётчик отчётов —
    /// обнуляться. Раньше незавершённый сброс обнулял `workers_done`, не
    /// трогая `busy`, и пул залипал (регресс C2).
    #[test]
    fn repeated_dispatch_and_reset_leaves_the_pool_usable() {
        let _g = crate::tests::lock();
        let p = pool(3);
        let descriptors = [JobDescriptor::dummy(); 6];
        for round in 0..25 {
            assert!(p.reset_to_idle(), "раунд {round}: пул должен затихнуть");
            assert!(p.is_quiesced(), "раунд {round}: пул не затих после reset");
            p.dispatch(6, 3, &descriptors);
            assert_eq!(
                p.wait_batch(Duration::from_secs(10)),
                Ok(()),
                "раунд {round}: батч не собран"
            );
            assert!(
                p.is_quiesced(),
                "раунд {round}: батч собран, а пул всё ещё считается занятым"
            );
        }
        // И пул по-прежнему в состоянии, пригодном для замера.
        p.dispatch(6, 3, &descriptors);
        assert_eq!(p.wait_batch(Duration::from_secs(10)), Ok(()));
    }

    /// Регресс C2: сброс НЕЗАТИХШЕГО пула обязан оставить возможность
    /// восстановления.
    ///
    /// Прежний код обнулял `workers_done`, но не трогал `busy`. Все отчёты за
    /// этот батч уже пришли, поэтому `workers_done` уже никогда не достиг бы
    /// `worker_count`, `busy` не снимался никогда, каждая `quiesce()` выжигала
    /// полный таймаут, а `Engine::reset()` возвращал `false` — пул заблокирован
    /// навсегда.
    ///
    /// Проверяем именно возможность восстановления: счётчик не обнулён и
    /// поздний отчёт воркера всё ещё может снять `busy`.
    #[test]
    fn reset_of_a_busy_pool_keeps_the_late_report_able_to_finish_the_batch() {
        let _g = crate::tests::lock();
        let p = pool(2);
        // Один воркер из двух ещё не отчитался.
        p.force_busy_for_test(1);
        assert!(!p.is_quiesced());
        let (done_before, expected, busy_before) = p.inner_for_test();
        assert_eq!((done_before, expected, busy_before), (1, 2, true));

        // `quiesce` не дождётся (воркера, который должен был отчитаться, нет):
        // пул остаётся занят — и это честно, а не «всё в порядке».
        assert!(!p.reset_to_idle(), "пул не может считаться тихим");
        let (done_after, _, busy_after) = p.inner_for_test();
        assert_eq!(
            done_after, done_before,
            "счётчик отчётов обнулён: поздний отчёт уже не закроет батч"
        );
        assert!(
            busy_after,
            "busy снят, хотя воркер ещё может писать в буферы сущностей"
        );

        // Поздний отчёт воркера обязан снять `busy` — иначе блокировка вечна.
        p.report_done_for_test();
        assert!(
            p.is_quiesced(),
            "поздний отчёт не закрыл батч: пул заблокирован навсегда"
        );
    }

    /// Регресс C2: сброс ТИХОГО пула обязан привести состояние в порядок —
    /// иначе следующий батч увидит старые счётчики.
    #[test]
    fn reset_of_a_quiet_pool_clears_the_completion_counters() {
        let _g = crate::tests::lock();
        let p = pool(2);
        let descriptors = [JobDescriptor::dummy(); 4];
        p.dispatch(4, 2, &descriptors);
        assert_eq!(p.wait_batch(Duration::from_secs(10)), Ok(()));
        assert_eq!(p.inner_for_test().0, 2, "батч собран не всеми воркерами");

        assert!(p.reset_to_idle(), "пул обязан затихнуть к моменту сброса");
        let (done, _, busy) = p.inner_for_test();
        assert_eq!(done, 0, "счётчик отчётов не обнулён после тихого сброса");
        assert!(!busy, "busy не снят после тихого сброса");

        // Пул обязан быть пригоден: следующий батч собирается заново, и его
        // завершение засчитывается с нуля.
        p.dispatch(4, 2, &descriptors);
        assert_eq!(
            p.wait_batch(Duration::from_secs(10)),
            Ok(()),
            "после сброса пул не собирает батчи"
        );
        assert!(p.is_quiesced(), "после сброса батч не закрылся");
    }

    /// Сколько отчётов ждёт батч: все воркеры, кроме ушедших навсегда.
    ///
    /// Требование отчёта от потока, которого больше нет, означало, что батч не
    /// завершится никогда: `wait_batch` всегда отдавал `TimedOut`, а `busy`
    /// оставалось поднятым — пул залипал (регресс H13/H14).
    #[test]
    fn expected_reports_shrink_with_lost_workers() {
        let _g = crate::tests::lock();
        let p = pool(3);
        assert_eq!(p.inner_for_test().1, 3, "целый пул ждёт все отчёты");
        p.mark_worker_lost_for_test(1);
        assert_eq!(p.inner_for_test().1, 2);
        p.mark_worker_lost_for_test(3);
        assert_eq!(
            p.inner_for_test().1,
            1,
            "даже когда не осталось ни одного воркера, батч обязан закрыться"
        );
    }

    /// Регресс H13/H14: `wait_batch` не должен требовать отчётов от ушедших
    /// воркеров — иначе он всегда упирался бы в потолок.
    #[test]
    fn wait_batch_does_not_demand_reports_from_lost_workers() {
        let _g = crate::tests::lock();
        let p = pool(3);
        // Двое из трёх ушли навсегда: батч вправе закрыться по отчёту третьего.
        p.mark_worker_lost_for_test(2);
        let descriptors = [JobDescriptor::dummy(); 4];
        p.dispatch(4, 3, &descriptors);
        assert_eq!(
            p.wait_batch(Duration::from_secs(10)),
            Ok(()),
            "батч не закрылся, хотя все живые воркеры отчитались"
        );
        assert!(p.is_quiesced(), "busy остался поднятым после сборки отчётов");
    }

    /// Уход воркера посреди батча не должен оставить батч незавершённым: его
    /// отчёт зачитывается при выходе, поэтому `busy` снимается.
    #[test]
    fn the_last_report_always_closes_the_batch() {
        let _g = crate::tests::lock();
        let p = pool(2);
        p.mark_worker_lost_for_test(2);
        p.force_busy_for_test(0);
        p.report_done_for_test();
        assert!(
            p.is_quiesced(),
            "после единственного отчёта пул обязан быть тихим"
        );
    }
}
