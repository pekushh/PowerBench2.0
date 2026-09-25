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
use crate::config::MAXIMUM_JOBS;
use crate::prng::unit_bits;

/// Константа смешивания проб видимости (из спецификации).
const VISIBILITY_MIX_CONSTANT: u64 = 0x9E37_79B9;

/// Период опроса при ожидании пробуждения.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Грейс-период кооперативной отмены: ограничивает время выхода из фазы.
const CANCEL_GRACE: Duration = Duration::from_millis(100);

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
    /// Сколько воркеров уже завершили работу над текущим батчем.
    workers_done: usize,
    /// Отказ одного из воркеров (падение потока) — ошибка запуска.
    faulted: bool,
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
        let na = a + 0.000_1;
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
        let guard = shared.lock.lock().unwrap();
        debug_assert_eq!(guard.epoch, epoch, "воркер обработал не тот батч");
        let active = guard.active_jobs;
        let mut j = worker_index;
        while j < active {
            debug_assert!(n < MAXIMUM_JOBS);
            my_indices[n] = j;
            my_descs[n] = guard.descriptors[j];
            n += 1;
            j += worker_count;
        }
    }

    // Локальная копия слотов перед единичной записью в общий массив.
    let mut my_slots: [u64; MAXIMUM_JOBS] = [0; MAXIMUM_JOBS];
    for k in 0..n {
        if cancel.load(Ordering::Relaxed) {
            break; // кооперативный выход из текущего батча
        }
        my_slots[k] = unsafe { run_job(my_descs[k], entities) };
    }

    if n > 0 {
        let mut guard = shared.lock.lock().unwrap();
        for k in 0..n {
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
            let mut guard = shared.lock.lock().unwrap();
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
                    .unwrap();
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
        let mut guard = shared.lock.lock().unwrap();
        if caught.is_err() {
            guard.faulted = true;
        }
        guard.workers_done += 1;
        shared.work_done.notify_all();
        drop(guard);
    }
}

impl Pool {
    /// Создать пул из `worker_count` persistent-потоков.
    pub fn new(worker_count: usize, entities: Arc<RawShared<EntityBuffers>>) -> Self {
        assert!(worker_count >= 1, "пул воркеров не может быть пустым");
        let shared = Arc::new(PoolShared {
            lock: Mutex::new(PoolInner {
                epoch: 0,
                descriptors: [JobDescriptor::dummy(); MAXIMUM_JOBS],
                job_slots: [0u64; MAXIMUM_JOBS],
                active_jobs: 0,
                workers_done: 0,
                faulted: false,
            }),
            work_ready: Condvar::new(),
            work_done: Condvar::new(),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));

        let mut handles = Vec::with_capacity(worker_count);
        for w in 0..worker_count {
            let shared = shared.clone();
            let cancel = cancel.clone();
            let shutdown = shutdown.clone();
            let entities = entities.clone();
            let handle = std::thread::Builder::new()
                .name(format!("powerbench-worker-{w}"))
                .spawn(move || worker_loop(shared, cancel, shutdown, w, worker_count, entities))
                .expect("не удалось запустить воркер");
            handles.push(handle);
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

    /// Пул в idle перед фазой: батча нет, счётчик завершившихся воркеров нулевой.
    pub fn reset_to_idle(&self) {
        let mut guard = self.shared.lock.lock().unwrap();
        guard.active_jobs = 0;
        guard.workers_done = 0;
        guard.faulted = false;
    }

    /// Опубликовать батч из `active_jobs` задач и разбудить воркеров.
    pub fn dispatch(&self, active_jobs: usize, descriptors: &[JobDescriptor]) {
        assert!((1..=MAXIMUM_JOBS).contains(&active_jobs));
        // Короткий слайс молча оставил бы stale-дескрипторы прошлого батча.
        assert!(descriptors.len() >= active_jobs);
        let mut guard = self.shared.lock.lock().unwrap();
        guard.epoch = guard.epoch.wrapping_add(1);
        guard.workers_done = 0;
        guard.faulted = false;
        guard.active_jobs = active_jobs;
        for (i, d) in descriptors.iter().enumerate().take(active_jobs) {
            guard.descriptors[i] = *d;
        }
        drop(guard);
        self.shared.work_ready.notify_all();
    }

    /// Снимок слотов контрольных сумм задач по возрастанию номера задачи.
    pub fn job_slots_snapshot(&self, count: usize) -> [u64; MAXIMUM_JOBS] {
        let mut out = [0u64; MAXIMUM_JOBS];
        let guard = self.shared.lock.lock().unwrap();
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

            let mut guard = self.shared.lock.lock().unwrap();
            if guard.faulted && guard.workers_done == self.worker_count {
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
                .unwrap();
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
