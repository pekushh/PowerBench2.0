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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::buffers::{EntityBuffers, RawShared};
use crate::checksum::mix;
use crate::config::{ANIMATION_STEP, MAXIMUM_JOBS, VISIBILITY_MIX_CONSTANT};
use crate::prng::unit_bits;

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
    worker_count: usize,
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

fn worker_loop(
    shared: Arc<PoolShared>,
    cancel: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    worker_index: usize,
    worker_count: usize,
    entities: Arc<RawShared<EntityBuffers>>,
) {
    // Сентинел, отличный от первого эпоха, чтобы обработать первый батч.
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
                if guard.epoch != last_epoch {
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
            guard.workers_done = guard.workers_done.saturating_add(1);
            guard.completed_epoch = epoch;
            if guard.workers_done >= worker_count {
                guard.busy = false;
            }
        }
        shared.work_done.notify_all();
        drop(guard);
    }
}

impl Pool {
    /// Создать пул из `worker_count` persistent-потоков.
    ///
    /// `worker_count == 0` — ошибка конфигурации, а не «пустой пул»: раньше
    /// это приводило к панике в горячем потоке. Если поток запустить не удалось,
    /// уже запущенные останавливаются, иначе они остались бы жить вечно.
    pub fn new(worker_count: usize, entities: Arc<RawShared<EntityBuffers>>) -> Self {
        assert!(worker_count >= 1, "пул воркеров не может быть пустым");
        let shared = Arc::new(PoolShared {
            lock: Mutex::new(PoolInner {
                epoch: 0,
                descriptors: [JobDescriptor::dummy(); MAXIMUM_JOBS],
                job_slots: [0u64; MAXIMUM_JOBS],
                active_jobs: 0,
                active_workers: 1,
                workers_done: 0,
                faulted: false,
                completed_epoch: 0,
                busy: false,
            }),
            work_ready: Condvar::new(),
            work_done: Condvar::new(),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));

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
            let entities = entities.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("powerbench-worker-{w}"))
                .spawn(move || worker_loop(shared, cancel, shutdown, w, worker_count, entities));
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(e) => {
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

        Self {
            shared,
            handles,
            cancel,
            shutdown,
            worker_count,
        }
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

    /// quiesce выполняется первым, и его результат возвращается наружу: раньше
    /// Пул в idle перед фазой: батча нет, счётчик завершившихся воркеров нулевой.
    /// он отбрасывался, и о незакрытом пуле узнавали только по следующим
    ///
    /// симптомам — расхождением контрольных сумм между прогонами.
    /// Сначала дожидается «тишины» пула: если грейс отмены истёк, воркеры могут
    /// ещё писать в буферы сущностей. Сбрасывать буферы в этот момент — гонка
    /// памяти и рассинхрон контрольных сумм, поэтому сначала quiesce.
    pub fn reset_to_idle(&self) -> bool {
        let quiet = self.quiesce();
        let mut guard = lock_pool(&self.shared);
        guard.active_jobs = 0;
        guard.active_workers = 1;
        guard.workers_done = 0;
        guard.faulted = false;
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
    pub fn dispatch(&self, active_jobs: usize, active_workers: usize, descriptors: &[JobDescriptor]) {
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
    pub fn wait_batch(&self) -> Result<(), BatchError> {
        let mut grace_start: Option<Instant> = None;
        loop {
            if self.cancel.load(Ordering::Acquire) && grace_start.is_none() {
                grace_start = Some(Instant::now());
            }

            let mut guard = lock_pool(&self.shared);
            if guard.faulted && guard.workers_done >= self.worker_count {
                return Err(BatchError::WorkerFailed);
            }
            if guard.workers_done == self.worker_count {
                return if grace_start.is_some() {
                    Err(BatchError::Cancelled)
                } else {
                    Ok(())
                };
            }
            if grace_start.is_some_and(|start| start.elapsed() > CANCEL_GRACE) {
                return Err(BatchError::Cancelled);
            }
            let waited = self
                .shared
                .work_done
                .wait_timeout(guard, POLL_INTERVAL)
                .unwrap_or_else(|e| e.into_inner());
            guard = waited.0;
        }
    }

    /// Кооперативная, ограниченная по времени остановка пула.
    pub fn shutdown(&mut self) {
        if self.shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        self.cancel.store(true, Ordering::Release);
        self.shared.work_ready.notify_all();
        let handles = std::mem::take(&mut self.handles);
        for h in handles {
            let _ = h.join();
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shutdown();
    }
}
