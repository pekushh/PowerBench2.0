//! Планировщик сессии: применение схемы, преамбула (пауза после схемы,
//! проверка фона, разогрев профилем «Отклик»), измеряемые фазы со
//! стабилизационной паузой и сверкой контрольных сумм, сторожевой таймер,
//! фоновый мониторинг и корреляция скачков, раунды с ротацией схем,
//! контрольные точки, охлаждение и восстановление исходной схемы при любом
//! завершении.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use powerbench_core::config::Phase;
use powerbench_core::engine::{
    Engine, ProgressSnapshot, RunError, RunReport, RunTarget, Scoreboard,
};
use powerbench_metrics::run::{
    RunStats, burst_retention_percent, consistency_percent, median, run_stats,
};
use powerbench_metrics::{
    AggregateResult, CompatibilitySignature, DeterminismSignature, aggregate_runs,
};
use powerbench_recommend::{Recommendation, RunCompact, SchemeAggregate, recommend};
use powerbench_windows::monitor::{
    CorrelatedProcess, ProcessSample, ProcessSampler, SpikeWindow, correlate,
};
use powerbench_windows::powercfg::PowerScheme;

use crate::checkpoint::{Checkpoint, PhaseStats, PowerSnapshot, StoredRun};
use crate::config::{
    PHASES_PER_RUN, SessionConfig, canonical_scheme_order, phase_durations, round_order, run_key,
    validate_config,
};
use crate::quarantine::{
    CATASTROPHIC_SHARE_LIMIT, DEGRADED_MIN_RUNS, DEGRADED_SHARE_LIMIT, MACHINE_COLLAPSE_SHARE,
    MIN_RUNS_FOR_JUDGMENT, QuarantineKind, TestingMarker, UNSTABLE_MAD_LIMIT,
    below_machine_baseline, catastrophic_share, clear_testing_marker, degraded_share,
    load_quarantine, phase_floor_collapse, preflight_filter, quarantine_add, unstable_spread,
    write_testing_marker,
};

/// Период опроса сторожевого таймера.
pub const WATCHDOG_POLL_MS: u64 = 250;
/// Сколько секунд сторож ждёт признака старта фазы, прежде чем сдаться.
pub const WATCHDOG_START_GRACE_SECS: u64 = 30;
/// Длительность без прогресса, после которой прогон бракуется (спецификация).
pub const WATCHDOG_NO_PROGRESS_SECS: u64 = 30;
/// Грейс ожидания остановки нагрузки после отмены (спецификация).
pub const WATCHDOG_CANCEL_GRACE_SECS: u64 = 5;
/// Пауза после применения схемы питания: живёт в `config`, оттуда же её
/// берёт оценка времени сессии (единый источник правды).
pub use crate::config::PAUSE_AFTER_SCHEME_SECS;
/// Длительность одного замера фоновой нагрузки (спецификация).
pub const BACKGROUND_MEASURE_MS: u64 = 700;
/// Пауза между повторными замерами фона (спецификация).
pub const BACKGROUND_RETRY_PAUSE_MS: u64 = 1200;
/// Число попыток проверки фона до предупреждения (спецификация).
pub const BACKGROUND_ATTEMPTS: u32 = 3;
/// Стабилизационная пауза после окончания каждой фазы: живёт в `config`,
/// оттуда же её берёт оценка времени сессии (единый источник правды).
pub use crate::config::STABILIZATION_SECS;

/// Ошибка сессии.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionError {
    NotAdmin,
    NoAcPower,
    Config(String),
    CheckpointPlanMismatch {
        expected: String,
        found: String,
    },
    ApplyScheme {
        scheme_id: String,
        cause: String,
    },
    ChecksumMismatch {
        scheme_id: String,
        phase: String,
        expected: u64,
        actual: u64,
    },
    Engine(RunError),
    LoadDidNotStop,
    Aggregate(String),
    Persist(String),
    RestoreScheme(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NotAdmin => write!(f, "требуются права администратора"),
            SessionError::NoAcPower => write!(f, "запуск запрещён без питания от сети"),
            SessionError::Config(msg) => write!(f, "неверный план: {msg}"),
            SessionError::CheckpointPlanMismatch { expected, found } => write!(
                f,
                "контрольная точка относится к другому плану: ожидался {expected}, найден {found}"
            ),
            SessionError::ApplyScheme { scheme_id, cause } => {
                write!(f, "не удалось применить схему {scheme_id}: {cause}")
            }
            SessionError::ChecksumMismatch {
                scheme_id,
                phase,
                expected,
                actual,
            } => write!(
                f,
                "рассинхрон контрольных сумм: схема {scheme_id}, фаза {phase}: ожидалось {expected:#018x}, получено {actual:#018x}"
            ),
            SessionError::Engine(err) => write!(f, "ошибка движка: {err:?}"),
            SessionError::LoadDidNotStop => write!(
                f,
                "нагрузка не остановилась за не более {} с после отмены",
                WATCHDOG_CANCEL_GRACE_SECS
            ),
            SessionError::Aggregate(msg) => write!(f, "ошибка агрегации: {msg}"),
            SessionError::Persist(msg) => write!(f, "ошибка сохранения контрольной точки: {msg}"),
            SessionError::RestoreScheme(cause) => {
                write!(f, "не удалось восстановить исходную схему: {cause}")
            }
        }
    }
}

/// Событие сессии (для консольного протоколирования).
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Started {
        plan_guid: String,
        rounds: u32,
        schemes: usize,
    },
    SchemeApplied {
        scheme_id: String,
    },
    BackgroundNoisy {
        measured_total_percent: f64,
        threshold_percent: f64,
    },
    BackgroundClean {
        measured_total_percent: f64,
        threshold_percent: f64,
    },
    PhaseStarted {
        label: String,
        seconds: u64,
    },
    PhaseFinished {
        label: String,
        ticks: u64,
        samples: usize,
    },
    SpikeWindows {
        label: String,
        count: usize,
    },
    RunCompleted {
        key: String,
        ticks: u64,
        duration_ms: u64,
    },
    RunSkippedCompleted {
        key: String,
    },
    RunSkippedRejected {
        scheme_id: String,
    },
    SchemeRejected {
        scheme_id: String,
        reason: String,
    },
    Cooling {
        seconds: u64,
    },
    Restored {
        scheme_id: String,
        label: String,
    },
    Warn(String),
    Finished,
}

impl std::fmt::Display for SessionEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionEvent::Started {
                plan_guid,
                rounds,
                schemes,
            } => write!(f, "Сессия {plan_guid}: раундов {rounds}, схем {schemes}"),
            SessionEvent::SchemeApplied { scheme_id } => {
                write!(f, "Применена схема {scheme_id}")
            }
            SessionEvent::BackgroundNoisy {
                measured_total_percent,
                threshold_percent,
            } => write!(
                f,
                "Предупреждение: фон загружен ({measured_total_percent:.1}% при пороге {threshold_percent:.1}% суммарно), замеры продолжаются"
            ),
            SessionEvent::BackgroundClean {
                measured_total_percent,
                threshold_percent,
            } => write!(
                f,
                "Фон чистый: {measured_total_percent:.1}% при пороге {threshold_percent:.1}%"
            ),
            SessionEvent::PhaseStarted { label, seconds } => {
                write!(f, "Фаза «{label}» на {seconds} с")
            }
            SessionEvent::PhaseFinished {
                label,
                ticks,
                samples,
            } => {
                write!(
                    f,
                    "Фаза «{label}» завершена: {ticks} тиков, {samples} измерений"
                )
            }
            SessionEvent::SpikeWindows { label, count } => {
                write!(f, "Фаза «{label}»: окон скачков {count}")
            }
            SessionEvent::RunCompleted {
                key,
                ticks,
                duration_ms,
            } => {
                write!(
                    f,
                    "Прогон {key} завершён: {ticks} тиков за {duration_ms} мс"
                )
            }
            SessionEvent::RunSkippedCompleted { key } => {
                write!(f, "Пропуск {key}: уже выполнен ранее")
            }
            SessionEvent::RunSkippedRejected { scheme_id } => {
                write!(f, "Пропуск схемы {scheme_id}: забракована ранее")
            }
            SessionEvent::SchemeRejected { scheme_id, reason } => {
                write!(f, "Схема {scheme_id} забракована: {reason}")
            }
            SessionEvent::Cooling { seconds } => write!(f, "Охлаждение {seconds} с"),
            SessionEvent::Restored { label, scheme_id } => {
                write!(f, "Восстановлена схема {scheme_id} ({label})")
            }
            SessionEvent::Warn(msg) => write!(f, "Предупреждение: {msg}"),
            SessionEvent::Finished => write!(f, "Сессия завершена"),
        }
    }
}

/// Наблюдатель за ходом сессии для интерфейса: live-телеметрия, журнал,
/// панель прогресса. Не влияет на измерения и результаты. Методы вызываются
/// из фоновых потоков сессии (поток фазы и сторожевой таймер).
pub trait TelemetryObserver: Send + Sync {
    /// Любое событие сессии (журнал): фазы, прогоны, предупреждения.
    fn event(&self, _e: &SessionEvent) {}
    /// Перед началом измеряемой фазы: контекст текущего прогона.
    #[allow(clippy::too_many_arguments)]
    fn phase(
        &self,
        _run_index: u32,
        _run_total: u32,
        _round: u32,
        _scheme_id: &str,
        _scheme_name: &str,
        _label: &str,
        _seconds: u64,
    ) {
    }
    /// Живой снапшот ядра (~10 Гц внутри измеряемой фазы).
    fn tick(&self, _snap: &ProgressSnapshot, _phase_label: &str, _phase_started: &Instant) {}
}

/// Разослать событие наблюдателям и сложить в журнал сессии.
fn note(
    observer: &Option<Arc<dyn TelemetryObserver>>,
    events: &mut Vec<SessionEvent>,
    e: SessionEvent,
) {
    if let Some(obs) = observer {
        obs.event(&e);
    }
    events.push(e);
}

/// Драйвер окружения (powercfg/питание/права) — абстрагирован для тестов.
pub trait SchemeDriver {
    fn list_schemes(&self) -> Result<Vec<PowerScheme>, String>;
    fn set_active(&self, guid: &str) -> Result<(), String>;
    fn ac_power_online(&self) -> Result<bool, String>;
    fn is_admin(&self) -> bool;
}

/// Реальный драйвер поверх powercfg и Windows API.
pub struct RealSchemeDriver;

impl SchemeDriver for RealSchemeDriver {
    fn list_schemes(&self) -> Result<Vec<PowerScheme>, String> {
        powerbench_windows::powercfg::list_schemes().map_err(|e| e.message)
    }

    fn set_active(&self, guid: &str) -> Result<(), String> {
        powerbench_windows::powercfg::activate(guid).map_err(|e| e.message)
    }

    fn ac_power_online(&self) -> Result<bool, String> {
        powerbench_windows::power::ac_power_online().map_err(|e| format!("{e:?}"))
    }

    fn is_admin(&self) -> bool {
        powerbench_windows::power::is_admin()
    }
}

/// Хранилище контрольной точки.
pub trait CheckpointStore {
    /// Загрузить контрольную точку. Err — файл есть, но прочитать его
    /// нельзя: это не то же самое, что «чекпоинта нет» (см. [DiskCheckpointStore]).
    fn load(&self) -> Result<Option<Checkpoint>, String>;
    fn save(&mut self, checkpoint: &Checkpoint) -> Result<(), String>;
}

/// Хранилище на диске (%LOCALAPPDATA%\PowerBench\).
pub struct DiskCheckpointStore;

impl CheckpointStore for DiskCheckpointStore {
    fn load(&self) -> Result<Option<Checkpoint>, String> {
        // Битый чекпоинт раньше молча превращался в «чекпоинта нет», и первая
        // же запись затирала его: все выполненные прогоны исчезали без
        // единого сообщения. Теперь это ошибка, а не пустота.
        crate::storage::read_json_checked(&crate::checkpoint::checkpoint_path())
    }

    fn save(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
        crate::checkpoint::save_checkpoint(checkpoint).map_err(|e| e.to_string())
    }
}

/// Итог сессии.
#[derive(Debug, Clone)]
pub struct SessionOutcome {
    pub checkpoint: Checkpoint,
    /// (scheme_id, агрегат) по допущенным схемам.
    pub aggregates: Vec<(String, AggregateResult)>,
    /// guid схемы → причина браковки.
    pub rejection_reasons: BTreeMap<String, String>,
    pub recommendation: Option<Recommendation>,
    pub events: Vec<SessionEvent>,
    pub cancelled: bool,
}

/// Подпись прогонов сессии (общая для всех: движок и диагностика фиксированы).
pub fn session_signature(engine: &Engine) -> CompatibilitySignature {
    CompatibilitySignature {
        workload_version: engine.version().to_string(),
        config_hash: engine.config_hash().to_string(),
        seed: engine.seed(),
        worker_count: engine.worker_count(),
        logical_cpus: engine.logical_cpus(),
        timer_hz: powerbench_windows::power::qpc_frequency(),
        cpu_identifier: powerbench_windows::power::cpu_identifier(),
        diagnostics_version: powerbench_windows::power::diagnostics_version().to_string(),
        // Привязка потоков к ядрам — часть сигнатуры: замер на физических
        // P-ядрах и замер «на всех логических» — разные измерения, и их
        // средние складывать нельзя.
        affinity_mode: engine.affinity_mode().as_str().to_string(),
        affinity_signature: engine.affinity_signature(),
    }
}

fn now_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn now_unix_secs() -> u64 {
    now_unix_ns() / 1_000_000_000
}

/// Человекочитаемое имя фазы. Подписи живут в `core::config::Phase::label()`,
/// иначе отчёт, JSON и тесты рано или поздно разойдутся между собой.
pub fn phase_label(phase: Phase) -> &'static str {
    phase.label()
}

/// Порог скачка латентности.
///
/// Раньше использовалось `медиана + 3σ`. σ неустойчив: несколько крупных
/// выбросов (а они и есть предмет детекции) сами раздувают σ, порог
/// поднимается выше настоящих скачков, и они перестают находиться. Используем
/// медианное абсолютное отклонение (MAD) — стандартная робастная оценка
/// разброса. Коэффициент 3 подобран так, чтобы для нормального распределения
/// порог соответствовал ~3σ: `σ ≈ 1.4826 · MAD`.
pub const SPIKE_MAD_FACTOR: f64 = 3.0;

pub fn spike_threshold(times: &[f64]) -> f64 {
    if times.is_empty() {
        return 0.0;
    }
    let med = median(times);
    let deviations: Vec<f64> = times.iter().map(|t| (t - med).abs()).collect();
    let mad = median(&deviations);
    if mad > 0.0 {
        med + SPIKE_MAD_FACTOR * 1.4826 * mad
    } else {
        // Все времена почти равны: отступаем от среднего хотя бы на 20%,
        // иначе порог равен значению и скачки не находятся вовсе.
        med * 1.2
    }
}

/// Окна скачков по временам тиков. Индексы переводятся в настенные секунды
/// линейно: `секунда ≈ start + индекс / (сэмплов в секунду)`.
pub fn spike_windows_for(
    times: &[f64],
    threshold: f64,
    start_second: u64,
    sec_per_index: f64,
    label: &str,
) -> Vec<SpikeWindow> {
    let sec_per_index = if sec_per_index > 0.0 {
        sec_per_index
    } else {
        1.0 / times.len().max(1) as f64
    };
    let mut out: Vec<SpikeWindow> = Vec::new();
    let mut i = 0usize;
    while i < times.len() {
        if times[i] > threshold {
            let begin = i;
            while i + 1 < times.len() && times[i + 1] > threshold {
                i += 1;
            }
            out.push(SpikeWindow {
                start_second: start_second + (begin as f64 * sec_per_index) as u64,
                end_second: start_second + ((i + 1) as f64 * sec_per_index) as u64,
                phase_label: label.to_string(),
            });
        }
        i += 1;
    }
    out
}

/// Решение «фон чистый»: суммарная загрузка процессов (кроме системы и себя)
/// меньше порога `threshold_percent * логических_CPU`.
///
/// Нормировка `cpu_percent` у sysinfo — процент **одного** ядра, поэтому
/// сумма по процессам сравнивается с порогом, умноженным на число логических
/// CPU. Первый вызов `sample()` прогревает накопители sysinfo (без него
/// `cpu_usage()` отдаёт 0), дальше идут настоящие измерения.
pub fn background_check(logical_cpus: usize, threshold_percent: f64) -> (bool, f64) {
    let mut sampler = ProcessSampler::new();
    // Прогрев накопителей: без него первый замер всегда нулевой.
    let _ = sampler.sample();
    let mut last = 0.0;
    for attempt in 0..BACKGROUND_ATTEMPTS {
        std::thread::sleep(Duration::from_millis(BACKGROUND_MEASURE_MS));
        let samples = sampler.sample();
        last = samples.iter().map(|s| s.cpu_percent).sum();
        let threshold = threshold_percent * logical_cpus as f64;
        if last < threshold {
            return (true, last);
        }
        // Между попытками ждём только если ещё остались попытки.
        if attempt + 1 < BACKGROUND_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(BACKGROUND_RETRY_PAUSE_MS));
        }
    }
    (false, last)
}

/// Вердикт сторожевого таймера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogVerdict {
    Hung,
    UserCancelled,
    /// Машина не пашет, а ползёт: замер продолжать бессмысленно.
    Collapsed {
        ticks_per_sec: u64,
    },
}

/// Доля мгновенного темпа, ниже которой фаза считается проваленной.
///
/// Порогов в тиках в секунду нет намеренно: абсолютное число было бы свойством
/// машины, а не схемы, и на ноутбуке обрывало бы здоровые замеры. Ориентиром
/// служит то, что эта же машина показывала в этой же фазе раньше; если такой
/// истории нет, фаза не обрывается вовсе (сторож зависания по нулевому
/// прогрессу остаётся — он от машины не зависит).
pub const COLLAPSE_RATE_SHARE: f64 = 0.20;
/// Первые секунды фазы не смотрим: разгон частот и холодный кэш дают низкий
/// темп даже у совершенно здоровой машины.
pub const COLLAPSE_ARM_SECS: u64 = 4;
/// Сколько секунд темп должен оставаться ниже порога, прежде чем остановиться.
///
/// Несколько секунд, а не один замер: мгновенный темп считается по времени
/// одного батча и сам по себе скачет.
pub const COLLAPSE_HOLD_SECS: u64 = 3;

/// Детектор «машина не тянет» по мгновенному темпу тиков.
///
/// Вынесен отдельным типом с явным временем на входе, чтобы правило можно было
/// проверить без реального прогона: иначе любой тест на «обрыв замера» стоил бы
/// минуты работы сторожевого таймера.
#[derive(Debug, Default)]
pub struct CollapseDetector {
    below_since: Option<Duration>,
    /// Порог в тиках в секунду; без ориентира по истории машины его нет.
    floor: Option<f64>,
}

impl CollapseDetector {
    /// Создать детектор с порогом из истории машины для этой фазы.
    pub fn with_reference(reference_p1: Option<f64>) -> Self {
        Self {
            below_since: None,
            floor: reference_p1
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| v * COLLAPSE_RATE_SHARE),
        }
    }

    /// Отметить очередной замер темпа; вернуть, сколько секунд темп держится
    /// ниже порога, если пора прекращать замер.
    pub fn observe(&mut self, ticks_per_sec: u64, elapsed: Duration) -> Option<Duration> {
        // Без ориентира по истории машины решение не принимаем: неизвестно, что
        // для неё считать нормой.
        let floor = self.floor?;
        // До разгона не смотрим вовсе.
        if elapsed.as_secs() < COLLAPSE_ARM_SECS {
            return None;
        }
        let below = (ticks_per_sec as f64) < floor;
        if !below {
            // Темп восстановился — отсчёт начинается заново.
            self.below_since = None;
            return None;
        }
        let since = *self.below_since.get_or_insert(elapsed);
        let held = elapsed.saturating_sub(since);
        (held.as_secs() >= COLLAPSE_HOLD_SECS).then_some(held)
    }
}

/// Сторожевой таймер фазы.
///
/// Ключевые требования:
/// 1. **Не уйти до старта фазы.** Раньше первый же опрос видел `running == false`
///    (флаг взводится внутри `run_phase`, то есть позже старта потока) и сторож
///    завершался, не отслеживая зависание вообще. Теперь сторож сначала ждёт
///    признака старта фазы (или отмены), и только затем начинает следить.
/// 2. **Не ждать завершения самому.** Он только выставляет флаг отмены;
///    освобождение батча делает пул (см. `Pool::quiesce`).
fn spawn_watchdog(
    scoreboard: Arc<Scoreboard>,
    cancel: Arc<AtomicBool>,
    user_cancel: Arc<AtomicBool>,
    collapse: CollapseDetector,
    phase_finished: Arc<AtomicBool>,
) -> (JoinHandle<()>, Receiver<WatchdogVerdict>) {
    let (tx, rx) = mpsc::channel();
    let handle: JoinHandle<()> = std::thread::spawn(move || {
        let mut last_ticks: u64 = 0;
        let mut last_progress = Instant::now();
        let mut collapse = collapse;
        // Фаза 1: ждём старта фазы. Если пользователь отменил раньше — выходим.
        //
        // Выход есть и по сигналу «фаза завершилась»: если фаза закончилась,
        // не успев начаться (например, воркер упал на первом тике), `running`
        // так и не станет `true`, и без этого сигнала сторож крутился бы вечно,
        // а `join()` на стороне сессии hangs бы насмерть.
        let wait_start = Instant::now();
        loop {
            if user_cancel.load(Ordering::Relaxed) {
                let _ = tx.send(WatchdogVerdict::UserCancelled);
                cancel.store(true, Ordering::Relaxed);
                return;
            }
            if scoreboard.snapshot().running {
                break;
            }
            if cancel.load(Ordering::Relaxed) {
                // Фаза отменена до старта (например, гонка отмены).
                return;
            }
            if phase_finished.load(Ordering::Acquire) {
                return;
            }
            if wait_start.elapsed().as_secs() >= WATCHDOG_START_GRACE_SECS {
                // Страховка на случай, если сигнал о завершении потерялся.
                return;
            }
            std::thread::sleep(Duration::from_millis(WATCHDOG_POLL_MS));
        }
        // Фаза 2: следим за прогрессом тиков.
        loop {
            std::thread::sleep(Duration::from_millis(WATCHDOG_POLL_MS));
            let snap: ProgressSnapshot = scoreboard.snapshot();
            if !snap.running {
                return;
            }
            if user_cancel.load(Ordering::Relaxed) {
                let _ = tx.send(WatchdogVerdict::UserCancelled);
                cancel.store(true, Ordering::Relaxed);
                return;
            }
            if snap.ticks_done != last_ticks {
                last_ticks = snap.ticks_done;
                last_progress = Instant::now();
            } else if last_progress.elapsed().as_secs() >= WATCHDOG_NO_PROGRESS_SECS {
                let _ = tx.send(WatchdogVerdict::Hung);
                cancel.store(true, Ordering::Relaxed);
                return;
            }
            // Замер ползущего темпа: тики идут, поэтому «зависание» не
            // срабатывает, а ждать конца сессии на такой машине нельзя —
            // пользователь успевает пожалеть о запуске.
            if collapse
                .observe(
                    snap.current_ticks_per_sec,
                    Duration::from_secs(snap.elapsed_secs),
                )
                .is_some()
            {
                let _ = tx.send(WatchdogVerdict::Collapsed {
                    ticks_per_sec: snap.current_ticks_per_sec,
                });
                cancel.store(true, Ordering::Relaxed);
                return;
            }
        }
    });
    (handle, rx)
}

/// Фоновый мониторинг процессов: раз в секунду пишет выборку в общую карту.
fn spawn_monitor(
    map: Arc<Mutex<BTreeMap<u64, Vec<ProcessSample>>>>,
    run: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut sampler = ProcessSampler::new();
        while run.load(Ordering::Relaxed) {
            let sec = now_unix_secs();
            let samples = sampler.sample();
            if !samples.is_empty() {
                map.lock().unwrap().insert(sec, samples);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    })
}

/// Проверка пользовательской отмены между фазами/прогонами.
fn user_wants_stop(user_cancel: &AtomicBool) -> bool {
    user_cancel.load(Ordering::Relaxed)
}

/// Сон с быстрой реакцией на отмену пользователя.
/// Возвращает `true`, если сон был прерван до истечения.
fn interruptible_sleep(user_cancel: &AtomicBool, seconds: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if user_cancel.load(Ordering::Relaxed) {
            return true;
        }
        // `saturating_sub` вместо вычитания: между проверкой и вычитанием поток
        // может быть вытеснен, и `deadline - now()` на отрицательном остатке
        // паниковал бы прямо в потоке сессии.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return user_cancel.load(Ordering::Relaxed);
        }
        std::thread::sleep(Duration::from_millis(100).min(left));
    }
}

/// Выполнить одну измеряемую фазу с watchdog и монитором, собрать времена
/// тиков.
#[allow(clippy::too_many_arguments)]
fn run_measured_phase(
    engine: &mut Engine,
    user_cancel: Arc<AtomicBool>,
    phase: Phase,
    seconds: u64,
    label: &str,
    monitor_map: &Arc<Mutex<BTreeMap<u64, Vec<ProcessSample>>>>,
    observer: Option<Arc<dyn TelemetryObserver>>,
    baseline: Option<&crate::history::MachineBaseline>,
) -> Result<(RunReport, Vec<f64>), PhaseFailure> {
    // Приоритет процесса поднимается ровно на время измеряемой фазы и
    // снимается на Drop — до любой точки выхода, включая ошибку и отмену.
    // Между фазами процесс остаётся в обычном классе, чтобы не мешать ни
    // интерфейсу, ни фоновой службе.
    let priority = powerbench_windows::priority::Guard::raise();
    if !engine.reset() {
        // Пул не затих: в буферы мог писать опоздавший воркер, и любые числа
        // отсюда недостоверны. Молчать об этом нельзя — иначе поломка всплывёт
        // позже как «контрольная сумма различается между повторами».
        return Err(PhaseFailure::LoadDidNotStop);
    }
    engine.prepare_sample_buffer(phase, seconds.max(1));
    let cancel = engine.canceller();
    let scoreboard = engine.scoreboard_arc();
    // Монитор процесса в фоне.
    let monitor_run = Arc::new(AtomicBool::new(true));
    let monitor_handle = spawn_monitor(Arc::clone(monitor_map), Arc::clone(&monitor_run));
    // Сигнал «фаза завершилась»: без него сторож ждал бы старта вечно, если
    // фаза закончилась, не успев начаться.
    let phase_finished = Arc::new(AtomicBool::new(false));
    // Сторожевой таймер: отменяет при бездействии или пользовательском Ctrl+C.
    // Детектор провала получает ориентир из истории машины: без него фаза не
    // обрывается, потому что неизвестно, что для этой машины считать нормой.
    let (watchdog_handle, rx) = spawn_watchdog(
        Arc::clone(&scoreboard),
        cancel,
        Arc::clone(&user_cancel),
        CollapseDetector::with_reference(baseline.as_ref().and_then(|b| b.p1_of(label))),
        Arc::clone(&phase_finished),
    );
    // Тел­еметрия ~10 Гц для интерфейса: читает снапшот ядра и зовёт наблюдателя.
    let telemetry_handle = observer.as_ref().map(|obs| {
        let obs = Arc::clone(obs);
        let scoreboard = Arc::clone(&scoreboard);
        let label = label.to_string();
        let phase_finished = Arc::clone(&phase_finished);
        let phase_started = Instant::now();
        std::thread::spawn(move || {
            loop {
                let snap = scoreboard.snapshot();
                if !snap.running {
                    // Поток поднимается ДО `run_phase`, и на первом опросе
                    // `running` ещё false — это гонка, а не конец фазы. Раньше
                    // телеметрия молчала всю фазу, если опрос проигрывал.
                    if phase_finished.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(WATCHDOG_POLL_MS));
                    continue;
                }
                obs.tick(&snap, &label, &phase_started);
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    });
    let result = engine.run_phase(
        phase,
        RunTarget::Duration(Duration::from_secs(seconds.max(1))),
    );
    // Сигнализируем всем наблюдателям, что фаза закончилась: без этого поток
    // телеметрии и сторож ждали бы следующего старта вечно.
    phase_finished.store(true, Ordering::Release);
    monitor_run.store(false, Ordering::Relaxed);
    let _ = monitor_handle.join();
    // Классификация отмены: вердикт запрашиваем только при отмене. Раньше
    // `recv_timeout` выполнялся при любой ошибке (включая ошибки движка, которых
    // сторож не присылает) — это гарантированная лишняя пауза в 5 секунд.
    let verdict = if matches!(result, Err(RunError::Cancelled)) {
        rx.recv_timeout(Duration::from_millis(WATCHDOG_CANCEL_GRACE_SECS * 1000))
            .ok()
    } else {
        None
    };
    let _ = watchdog_handle.join();
    if let Some(h) = telemetry_handle {
        let _ = h.join();
    }
    // Гарантия «нагрузка остановлена»: движок уже вернул управление, значит
    // батч снят, но воркеры могли не уложиться в грейс отмены. Ждём реальной
    // тишины пула — иначе они продолжали бы писать в буферы сущностей, а
    // следующий `reset` пересоздал бы данные у них под ногами.
    if !engine.quiesce_pool() {
        return Err(PhaseFailure::LoadDidNotStop);
    }
    // Приоритет возвращается исходному классу до выхода из функции: замер
    // окончен, а процесс пользователя не должен остаться в HIGH.
    drop(priority);
    match (result, verdict) {
        (Err(RunError::Cancelled), Some(WatchdogVerdict::Hung)) => Err(PhaseFailure::Hung {
            phase: label.to_string(),
        }),
        (Err(RunError::Cancelled), Some(WatchdogVerdict::Collapsed { ticks_per_sec })) => {
            Err(PhaseFailure::Collapsed {
                phase: label.to_string(),
                ticks_per_sec,
            })
        }
        (Err(RunError::Cancelled), _) => Err(PhaseFailure::UserCancelled),
        (Err(err), _) => Err(PhaseFailure::Engine(err)),
        (Ok(report), _) => Ok((report, engine.samples().to_vec())),
    }
}

/// Ошибка отдельной фазы.
#[derive(Debug, Clone, PartialEq)]
enum PhaseFailure {
    Engine(RunError),
    Hung {
        phase: String,
    },
    /// Машина ползёт: замер остановлен досрочно, схема бракуется.
    Collapsed {
        phase: String,
        ticks_per_sec: u64,
    },
    UserCancelled,
    LoadDidNotStop,
}

/// Медиана и 95-й перцентиль фоновой нагрузки за интервал прогона.
///
/// Суммируется `cpu_percent` всех процессов в снимке (норма sysinfo — процент
/// **одного** ядра). Возвращается `(p50, p95, сколько секунд набралось)`.
/// Снимки вне интервала прогона не берутся: между прогонами сэмплер всё равно
/// не ходит, но при переходе между схемами в карту могли попасть чужие секунды.
fn background_cpu_stats(
    map: &BTreeMap<u64, Vec<ProcessSample>>,
    from_secs: u64,
    to_secs: u64,
) -> (f64, f64, u32) {
    let mut series: Vec<f64> = map
        .iter()
        .filter(|(sec, _)| **sec >= from_secs && **sec <= to_secs)
        .map(|(_, samples)| samples.iter().map(|s| s.cpu_percent).sum())
        .collect();
    if series.is_empty() {
        return (0.0, 0.0, 0);
    }
    let n = series.len();
    series.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |q: f64| -> f64 {
        let idx = ((q * n as f64).ceil() as usize).clamp(1, n) - 1;
        series[idx]
    };
    (at(0.50), at(0.95), n as u32)
}

/// Результат одного прогона схемы.
#[derive(Debug)]
struct RunData {
    key: String,
    round: u32,
    scheme_id: String,
    started_at_ns: u64,
    duration_ms: u64,
    ticks: u64,
    supercycles: u64,
    first_tick_checksums: [u64; PHASES_PER_RUN as usize],
    run_checksums: [u64; PHASES_PER_RUN as usize],
    phase_times: Vec<(u8, Vec<f64>)>,
    combined: RunStats,
    cross_phase_consistency: f64,
    burst_retention_percent: f64,
    background: Vec<CorrelatedProcess>,
    spike_windows_total: usize,
    /// Срез питания в конце каждой измеряемой фазы (индексы фаз в том же
    /// порядке, что `phase_times`).
    phase_power: Vec<PowerSnapshot>,
    /// Снимок питания в конце прогона.
    power: PowerSnapshot,
    /// Фоновая нагрузка (сумма по процессам, % одного ядра) по секундам
    /// прогона: p50 и p95.
    background_cpu: (f64, f64, u32),
}

const fn zero_run_stats() -> RunStats {
    RunStats {
        samples: 0,
        work_units: 0,
        active_time_ms_total: 0.0,
        average_throughput: 0.0,
        average_execution_time_ms: 0.0,
        median_throughput: 0.0,
        p1_throughput: 0.0,
        p01_throughput: 0.0,
        p95_execution_time_ms: 0.0,
        p99_execution_time_ms: 0.0,
        consistency_percent: 0.0,
        jitter_p99_ms: 0.0,
        worst_second_throughput: 0.0,
    }
}

/// Запуск полной сессии.
#[allow(clippy::too_many_arguments)]
pub fn run_session(
    engine: &mut Engine,
    driver: &dyn SchemeDriver,
    plan: SessionConfig,
    user_cancel: Arc<AtomicBool>,
    store: &mut dyn CheckpointStore,
    observer: Option<Arc<dyn TelemetryObserver>>,
) -> Result<SessionOutcome, SessionError> {
    let mut events: Vec<SessionEvent> = Vec::new();

    if let Some(reason) = validate_config(&plan) {
        return Err(SessionError::Config(reason));
    }
    // Карантин (префлайт): забракованные схемы не допускаются к прогонам.
    // Фильтруем здесь, а не в UI, чтобы работало и для CLI.
    let plan = {
        let (admitted, skipped) = preflight_filter(&plan.scheme_ids, &load_quarantine());
        for (id, why) in &skipped {
            note(
                &observer,
                &mut events,
                SessionEvent::Warn(format!("Схема «{id}» пропущена: {why}")),
            );
        }
        if admitted.is_empty() {
            return Err(SessionError::Config(
                "все выбранные схемы находятся в карантине — верните их в бенчмарк вручную"
                    .to_string(),
            ));
        }
        SessionConfig {
            scheme_ids: admitted,
            ..plan
        }
    };
    if !driver.is_admin() {
        return Err(SessionError::NotAdmin);
    }
    match driver.ac_power_online() {
        Ok(true) => {}
        _ => return Err(SessionError::NoAcPower),
    }

    let signature = session_signature(engine);

    // Ориентир «что эта машина умеет» из её же истории. Абсолютных порогов в
    // коде нет намеренно: 100 тик/с — это много для ноутбука и мало для
    // станции. Пока сопоставимой истории нет, ориентира нет — и тогда решения
    // о браке по абсолютной величине не принимаются вовсе.
    let baseline =
        crate::history::machine_baseline(&crate::result::IdentityJson::from_signature(&signature));

    // Список схем и карта «guid → имя»: имя нужно только для отображения
    // (PlanName), в идентификации/матчинге никогда не участвует.
    let schemes_list = driver.list_schemes().map_err(SessionError::Config)?;
    let name_map: BTreeMap<String, String> = schemes_list
        .iter()
        .map(|s| (s.guid.to_ascii_lowercase(), s.name.clone()))
        .collect();

    // Канонический порядок плана: активная схема первая, остальные как заданы.
    // Делается здесь, а не только в интерфейсе, чтобы правило действовало и
    // для CLI, и для продолжения из чекпоинта.
    let plan = {
        let active = schemes_list
            .iter()
            .find(|s| s.active)
            .map(|s| s.guid.as_str());
        let ordered = canonical_scheme_order(&plan.scheme_ids, active);
        if ordered != plan.scheme_ids {
            note(
                &observer,
                &mut events,
                SessionEvent::Warn(format!(
                    "порядок схем: активная «{}» перенесена в начало",
                    name_map
                        .get(&active.unwrap_or_default().to_ascii_lowercase())
                        .cloned()
                        .unwrap_or_else(|| active.unwrap_or_default().to_string())
                )),
            );
        }
        SessionConfig {
            scheme_ids: ordered,
            ..plan
        }
    };

    // Контрольная точка: либо текущий план, либо ошибка несовпадения.
    let loaded = store.load().map_err(SessionError::Config)?;
    let mut checkpoint = match loaded {
        Some(existing) => {
            if existing.plan.plan_guid != plan.plan_guid {
                return Err(SessionError::CheckpointPlanMismatch {
                    expected: plan.plan_guid.clone(),
                    found: existing.plan.plan_guid,
                });
            }
            existing
        }
        None => {
            let mut cp = Checkpoint::new(plan.clone());
            cp.original_scheme_guid = schemes_list
                .iter()
                .find(|s| s.active)
                .map(|s| s.guid.clone());
            cp
        }
    };

    // Восстановление после прерывания: если активна не исходная схема —
    // восстанавливаем до продолжения.
    // MSRV 1.85: схлопывание через let-цепочки требует Rust 1.88+.
    #[allow(clippy::collapsible_if)]
    if !checkpoint.original_restored {
        if let Some(original) = checkpoint.original_scheme_guid.clone() {
            let active_guid = driver
                .list_schemes()
                .map_err(SessionError::Config)?
                .into_iter()
                .find(|s| s.active)
                .map(|s| s.guid);
            if active_guid.as_deref() != Some(original.as_str()) {
                driver
                    .set_active(&original)
                    .map_err(SessionError::RestoreScheme)?;
                note(
                    &observer,
                    &mut events,
                    SessionEvent::Restored {
                        scheme_id: original.clone(),
                        label: "после прерывания".to_string(),
                    },
                );
            }
            checkpoint.original_restored = true;
            store.save(&checkpoint).map_err(SessionError::Persist)?;
        }
    }

    let n_schemes = plan.scheme_ids.len();
    note(
        &observer,
        &mut events,
        SessionEvent::Started {
            plan_guid: plan.plan_guid.clone(),
            rounds: plan.repetitions,
            schemes: n_schemes,
        },
    );

    // --- Условия измерения, которые пользователь обязан знать ---
    //
    // Всё это либо напрямую меняет величину в отчёте, либо ослабляет саму
    // защиту замера. Молчать о таком нельзя: тогда на вопрос «почему две
    // сессии так разошлись» пришлось бы отвечать догадками. Сообщается
    // один раз за сессию и только о проблемах — штатное состояние описано
    // строкой старта у обоих фронтендов.

    // 1. Привязка потоков к ядрам.
    for failure in engine.affinity_failures() {
        note(
            &observer,
            &mut events,
            SessionEvent::Warn(format!(
                "привязать поток к ядру не удалось ({failure}) — замер идёт без привязки, \
                 разброс между прогонами будет выше обычного"
            )),
        );
    }
    if let Some(why) = engine.topology().error() {
        note(
            &observer,
            &mut events,
            SessionEvent::Warn(format!(
                "топологию ядер разведать не удалось ({why}) — замер идёт без привязки"
            )),
        );
    }
    let clamp_note = engine.worker_count_note();
    if !clamp_note.is_empty() {
        note(
            &observer,
            &mut events,
            SessionEvent::Warn(format!(
                "{clamp_note}; фактически воркеров {}",
                engine.worker_count()
            )),
        );
    }

    // 2. Приоритет процесса на время фаз: поднимается и сразу возвращается,
    // поэтому служит здесь только проверкой «можно ли поднять».
    {
        let probe = powerbench_windows::priority::Guard::raise();
        if let Some(why) = probe.not_raised_reason() {
            note(
                &observer,
                &mut events,
                SessionEvent::Warn(format!(
                    "приоритет процесса на время замера не поднят ({why}) — \
                     фоновые процессы смогут вытеснять потоки бенчмарка"
                )),
            );
        }
        drop(probe);
    }

    // Гарантия ОС, которая не зависит от пути выхода.
    //
    // Маркер и исходная схема снимались только в ветках, которые явно до этого
    // доходили. Ошибка сохранения (`?`), ошибка движка или паника в потоке
    // сессии оставляли маркер на диске, и следующий запуск честно, но неверно
    // отправлял нормальную схему в карантин с причиной «процесс убили».
    // Теперь оба восстановления висят на Drop и срабатывают всегда.
    //
    // Важно, что guard создаётся ДО `run_session_loop`: сам цикл применяет
    // тестовую схему и пишет маркер прогона. Созданные после вызова они
    // существовали бы только к моменту возврата, и паника внутри цикла
    // разворачивала бы стек мимо них — тестовая схема осталась бы активной,
    // а маркер на диске — нетронутым. Ровно то состояние, из-за которого
    // следующий запуск карантинит исправную схему как HardFreeze.
    let _os_guard = OsRestoreGuard {
        original: checkpoint.original_scheme_guid.clone(),
        driver,
        restored: Arc::new(AtomicBool::new(checkpoint.original_restored)),
    };
    let _marker_guard = TestingMarkerGuard;

    // Основной цикл раундов (признак возврата — «остановлено пользователем»).
    let loop_result = run_session_loop(
        engine,
        driver,
        &plan,
        &user_cancel,
        store,
        &mut checkpoint,
        &mut events,
        &name_map,
        &observer,
        baseline.as_ref(),
    );

    // и при успехе, и при ошибке/отмене.
    let restore_result =
        restore_original(driver, &mut checkpoint, &mut events, &mut *store, &observer);
    // Помечаем, что штатное восстановление выполнено: guard на Drop иначе
    // переключил бы схему повторно при выходе из функции.
    if restore_result.is_ok() {
        _os_guard.restored.store(true, Ordering::Release);
    }

    let cancelled = match loop_result {
        Err(e) => {
            // Восстановление уже предпринято; возвращаем первичную ошибку.
            return Err(e);
        }
        Ok(cancelled) => cancelled,
    };
    restore_result?;

    // --- Агрегация и рекомендация ---
    let expected_runs = plan.repetitions as usize;
    let (aggregates, mut rejection_reasons, mut recommendation) =
        build_aggregation(&checkpoint, &signature, expected_runs);

    // Пост-сессия: карантин нестабильных и деградировавших схем. Идёт ДО
    // агрегации по списку: иначе схема могла уйти в карантин и тут же быть
    // рекомендована как победитель, а следующая сессия её уже не тестировала.
    let post = quarantine_post_session(
        &checkpoint,
        &plan.plan_guid,
        &name_map,
        &observer,
        &mut events,
        baseline.as_ref(),
    );
    for (id, reason) in post {
        // Потребитель (отчёт и интерфейс) помечает схему бракованной именно по
        // этой карте, поэтому пост-карантин обязан попасть в неё.
        rejection_reasons.insert(id.clone(), reason.clone());
        // Схема в карантине не может оставаться рекомендацией: вердикт по ней
        // бессмыслен, а следующая сессия её всё равно пропустит.
        if let Some(rec) = recommendation.as_mut().filter(|rec| {
            rec.recommended_scheme
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(&id))
        }) {
            rec.recommended_scheme = None;
            rec.runner_up_scheme = None;
            rec.reason = format!("схема-победитель отправлена в карантин: {reason}");
            rec.level = powerbench_recommend::EvidenceLevel::None;
        }
    }

    note(&observer, &mut events, SessionEvent::Finished);
    Ok(SessionOutcome {
        checkpoint,
        aggregates,
        rejection_reasons,
        recommendation,
        events,
        cancelled,
    })
}

/// Цикл раундов: применение схем, преамбула, измеряемые фазы, контрольные
/// точки и охлаждение. Возвращает признак «остановлено пользователем».
#[allow(clippy::too_many_arguments)]
fn run_session_loop(
    engine: &mut Engine,
    driver: &dyn SchemeDriver,
    plan: &SessionConfig,
    user_cancel: &Arc<AtomicBool>,
    store: &mut dyn CheckpointStore,
    checkpoint: &mut Checkpoint,
    events: &mut Vec<SessionEvent>,
    name_map: &BTreeMap<String, String>,
    observer: &Option<Arc<dyn TelemetryObserver>>,
    baseline: Option<&crate::history::MachineBaseline>,
) -> Result<bool, SessionError> {
    let durs = phase_durations(plan.duration_seconds);
    let mut cancelled = false;
    let run_total = (plan.scheme_ids.len() as u32) * plan.repetitions;
    let mut run_index: u32 = 0;

    'outer: for round in 0..plan.repetitions {
        let order = round_order(&plan.scheme_ids, round);
        // Ключ строго `"{round}:{plan-guid}"` без имени/guid схемы: отметка
        // «раунд выполнен целиком». Внутри раунда выполненные схемы
        // распознаются по записям прогонов (см. `Checkpoint::has_run`).
        let key = run_key(round, &plan.plan_guid);
        let round_done = checkpoint.is_completed(&key);
        if round_done {
            note(
                observer,
                events,
                SessionEvent::RunSkippedCompleted { key: key.clone() },
            );
            continue;
        }
        'scheme: for scheme_id in order {
            run_index += 1;
            if user_wants_stop(user_cancel) {
                cancelled = true;
                break 'outer;
            }
            if checkpoint.has_run(round, &scheme_id) {
                note(
                    observer,
                    events,
                    SessionEvent::RunSkippedCompleted { key: key.clone() },
                );
                continue;
            }
            if checkpoint.is_rejected(&scheme_id) {
                note(
                    observer,
                    events,
                    SessionEvent::RunSkippedRejected {
                        scheme_id: scheme_id.clone(),
                    },
                );
                continue;
            }

            // Сначала сбрасываем флаг (и сохраняем): если kill случится до
            // или во время set_active, чекпоинт честно говорит «не
            // восстановлено», и следующий запуск всё починит. Обратный
            // порядок оставлял бы чужую схему активной при restored=true.
            if checkpoint.original_restored {
                checkpoint.original_restored = false;
                store.save(checkpoint).map_err(SessionError::Persist)?;
            }
            // --- Применение схемы ---
            driver
                .set_active(&scheme_id)
                .map_err(|cause| SessionError::ApplyScheme {
                    scheme_id: scheme_id.clone(),
                    cause,
                })?;
            // Маркер активного прогона: переживает убийство процесса.
            // Если следующий запуск найдёт его — схема уйдёт в карантин
            // как подозрение на жёсткое зависание ПК.
            if let Err(e) = write_testing_marker(&TestingMarker {
                scheme_id: scheme_id.clone(),
                scheme_name: name_map.get(&scheme_id.to_ascii_lowercase()).cloned(),
                plan_guid: plan.plan_guid.clone(),
                started_at_ns: now_unix_ns(),
            }) {
                note(
                    observer,
                    events,
                    SessionEvent::Warn(format!(
                        "не удалось записать маркер прогона «{scheme_id}»: {e}"
                    )),
                );
            }
            note(
                observer,
                events,
                SessionEvent::SchemeApplied {
                    scheme_id: scheme_id.clone(),
                },
            );

            // --- Преамбула: пауза после схемы ---
            std::thread::sleep(Duration::from_secs(PAUSE_AFTER_SCHEME_SECS));

            // --- Проверка фона ---
            let (clean, measured) =
                background_check(engine.logical_cpus(), plan.background_threshold_percent);
            let threshold = plan.background_threshold_percent * engine.logical_cpus() as f64;
            if clean {
                note(
                    observer,
                    events,
                    SessionEvent::BackgroundClean {
                        measured_total_percent: measured,
                        threshold_percent: threshold,
                    },
                );
            } else {
                note(
                    observer,
                    events,
                    SessionEvent::BackgroundNoisy {
                        measured_total_percent: measured,
                        threshold_percent: threshold,
                    },
                );
            }

            // --- Разогрев профилем «Отклик» (результаты отбрасываются) ---
            if !engine.reset() {
                clear_testing_marker();
                return Err(SessionError::LoadDidNotStop);
            }
            engine.prepare_sample_buffer(Phase::Response, plan.warmup_seconds.max(1));
            let warmup_cancel = engine.canceller();
            let warmup_scoreboard = engine.scoreboard_arc();
            let warmup_finished = Arc::new(AtomicBool::new(false));
            let (warmup_handle, warmup_rx) = spawn_watchdog(
                warmup_scoreboard,
                warmup_cancel,
                Arc::clone(user_cancel),
                // Разогрев — не измерение: обрывать его по темпу нельзя, иначе
                // плохая схема «успела бы» не начать замер вовсе.
                CollapseDetector::default(),
                Arc::clone(&warmup_finished),
            );
            let warmup_result = engine.run_phase(
                Phase::Response,
                RunTarget::Duration(Duration::from_secs(plan.warmup_seconds.max(1))),
            );
            // Сторож разогрева обязан узнать, что фаза закончилась: иначе он
            // остаётся жить и опрашивает счётчик вхолостую до конца процесса.
            warmup_finished.store(true, Ordering::Release);
            match warmup_result {
                Err(RunError::Cancelled) => {
                    let verdict = warmup_rx
                        .recv_timeout(Duration::from_secs(WATCHDOG_CANCEL_GRACE_SECS))
                        .ok();
                    let _ = warmup_handle.join();
                    // Пул мог остаться горящим: `wait_batch` выходит по грейсу
                    // отмены, не дождавшись отчётов. Гасим его здесь же — иначе
                    // следующая схема начнёт смену плана поверх работающих
                    // воркеров, и замер пойдёт с недетерминированного состояния.
                    if !engine.quiesce_pool() {
                        clear_testing_marker();
                        return Err(SessionError::LoadDidNotStop);
                    }
                    if user_wants_stop(user_cancel)
                        || verdict == Some(WatchdogVerdict::UserCancelled)
                    {
                        cancelled = true;
                        clear_testing_marker();
                        break 'outer;
                    }
                    note(
                        observer,
                        events,
                        SessionEvent::SchemeRejected {
                            scheme_id: scheme_id.clone(),
                            reason: format!(
                                "нет прогресса более {} секунд",
                                WATCHDOG_NO_PROGRESS_SECS
                            ),
                        },
                    );
                    let reason = format!(
                        "нет прогресса более {} секунд (зависание на разогреве)",
                        WATCHDOG_NO_PROGRESS_SECS
                    );
                    checkpoint
                        .rejections
                        .insert(scheme_id.clone(), reason.clone());
                    store.save(checkpoint).map_err(SessionError::Persist)?;
                    // Зависание на разогреве — сразу в карантин.
                    quarantine_scheme(
                        &scheme_id,
                        name_map,
                        QuarantineKind::NoProgress,
                        &reason,
                        &plan.plan_guid,
                        observer,
                        events,
                    );
                    clear_testing_marker();
                    continue;
                }
                Err(err) => {
                    clear_testing_marker();
                    return Err(SessionError::Engine(err));
                }
                Ok(_) => {
                    let _ = warmup_handle.join();
                }
            }

            // --- Измеряемые фазы (с подключённым «пользовательским» флагом) ---
            let phase_secs = [
                durs.light_seconds,
                durs.partial_seconds,
                durs.heavy_seconds,
                durs.response_seconds,
            ];
            let monitor_map: Arc<Mutex<BTreeMap<u64, Vec<ProcessSample>>>> =
                Arc::new(Mutex::new(BTreeMap::new()));

            let started_at_ns = now_unix_ns();
            let run_started_secs = now_unix_secs();
            let phase_started = Instant::now();
            let mut times_by_phase: Vec<(u8, Vec<f64>)> = Vec::new();
            let mut spike_count_total = 0usize;
            let mut all_spike_windows: Vec<SpikeWindow> = Vec::new();
            let mut phase_power: Vec<PowerSnapshot> = Vec::new();
            let mut first_tick_checksums = [0u64; PHASES_PER_RUN as usize];
            let mut run_checksums = [0u64; PHASES_PER_RUN as usize];
            let mut ticks = 0u64;
            let mut supercycles = 0u64;

            let phases = powerbench_core::config::PHASE_ORDER;
            for (idx, (&phase, &secs)) in phases.iter().zip(phase_secs.iter()).enumerate() {
                if user_wants_stop(user_cancel) {
                    cancelled = true;
                    // Маркер прогона обязательно снимаем: иначе при следующем
                    // старте схема попадёт в карантин как «зависшая по вине
                    // пользователя».
                    clear_testing_marker();
                    break 'outer;
                }
                let label = phase_label(phase);
                // Момент старта ИМЕННО ЭТОЙ фазы: `phase_started` выше отсчитывает
                // весь прогон и годится только для `duration_ms`. По шкале фаз
                // строится привязка окон скачков к секундам, и для второй и
                // последующих фаз общая шкала растягивала окна в разы и
                // привязывала их к секундам, где скачков не было, из-за чего
                // `correlate` подставлял не тот процесс.
                let phase_started_at = Instant::now();
                note(
                    observer,
                    events,
                    SessionEvent::PhaseStarted {
                        label: label.to_string(),
                        seconds: secs,
                    },
                );
                if let Some(obs) = observer {
                    obs.phase(
                        run_index,
                        run_total,
                        round,
                        &scheme_id,
                        name_map
                            .get(&scheme_id.to_ascii_lowercase())
                            .map(String::as_str)
                            .unwrap_or("—"),
                        label,
                        secs,
                    );
                }
                let phase_result = run_measured_phase(
                    engine,
                    Arc::clone(user_cancel),
                    phase,
                    secs,
                    label,
                    &monitor_map,
                    observer.clone(),
                    baseline,
                );
                match phase_result {
                    Ok((report, times)) => {
                        let reference = engine.first_tick_reference(phase.first_profile());
                        // MSRV 1.85: схлопывание через let-цепочки требует Rust 1.88+.
                        #[allow(clippy::collapsible_if)]
                        if let Some(expected) = reference {
                            if report.first_tick_checksum != expected {
                                clear_testing_marker();
                                return Err(SessionError::ChecksumMismatch {
                                    scheme_id: scheme_id.clone(),
                                    phase: label.to_string(),
                                    expected,
                                    actual: report.first_tick_checksum,
                                });
                            }
                        }
                        note(
                            observer,
                            events,
                            SessionEvent::PhaseFinished {
                                label: label.to_string(),
                                ticks: report.ticks,
                                samples: times.len(),
                            },
                        );
                        ticks += report.ticks;
                        if phase == Phase::Response {
                            supercycles = report.supercycles_completed;
                        }
                        first_tick_checksums[idx] = report.first_tick_checksum;
                        run_checksums[idx] = report.run_checksum;
                        // Срез питания в конце фазы: троттлинг может начаться
                        // и кончиться внутри прогона, и по одному срезу в
                        // конце всего прогона это не поймать.
                        phase_power.push(PowerSnapshot::capture());
                        // Шаг перевода индекса сэмпла в секунды и начало фазы
                        // считаем по ФАКТИЧЕСКОМУ времени, а не по номинальному
                        // `secs`: фаза включает запуск нагрузки и разгон, из-за
                        // чего окна скачков систематически смещались на
                        // несколько секунд и не совпадали с выборками процессов.
                        let actual = phase_started_at.elapsed().as_secs_f64().max(0.001);
                        let sec_per_index = actual / times.len().max(1) as f64;
                        let windows = spike_windows_for(
                            &times,
                            spike_threshold(&times),
                            now_unix_secs().saturating_sub(actual.ceil() as u64),
                            sec_per_index,
                            label,
                        );
                        spike_count_total += windows.len();
                        all_spike_windows.extend(windows.clone());
                        times_by_phase.push((phase.index(), times));
                        // Стабилизационная пауза после каждой фазы.
                        std::thread::sleep(Duration::from_secs(STABILIZATION_SECS));
                        if spike_count_total > 0 {
                            note(
                                observer,
                                events,
                                SessionEvent::SpikeWindows {
                                    label: label.to_string(),
                                    count: windows.len(),
                                },
                            );
                        }
                    }
                    Err(PhaseFailure::Hung { phase }) => {
                        note(
                            observer,
                            events,
                            SessionEvent::SchemeRejected {
                                scheme_id: scheme_id.clone(),
                                reason: format!(
                                    "нет прогресса более {} секунд (фаза «{phase}»)",
                                    WATCHDOG_NO_PROGRESS_SECS
                                ),
                            },
                        );
                        let reason = format!(
                            "нет прогресса более {} секунд (фаза «{phase}»)",
                            WATCHDOG_NO_PROGRESS_SECS
                        );
                        checkpoint
                            .rejections
                            .insert(scheme_id.clone(), reason.clone());
                        store.save(checkpoint).map_err(SessionError::Persist)?;
                        // Зависание фазы — сразу в карантин.
                        quarantine_scheme(
                            &scheme_id,
                            name_map,
                            QuarantineKind::NoProgress,
                            &reason,
                            &plan.plan_guid,
                            observer,
                            events,
                        );
                        clear_testing_marker();
                        // Зависла одна схема — остальные схемы раунда идут дальше.
                        continue 'scheme;
                    }
                    Err(PhaseFailure::Collapsed {
                        phase,
                        ticks_per_sec,
                    }) => {
                        // Машина не пашет, а ползёт. Ждать конца прогона на
                        // таком темпе незачем: пользователь уже несколько секунд
                        // работает в замедленной машине. Останавливаем сразу и
                        // объясняем причину — иначе брак выглядит как зависание.
                        let reason = format!(
                            "машина держит всего {ticks_per_sec} тик/с (фаза «{phase}») — \
                             замер остановлен, схема не тянет"
                        );
                        note(
                            observer,
                            events,
                            SessionEvent::SchemeRejected {
                                scheme_id: scheme_id.clone(),
                                reason: reason.clone(),
                            },
                        );
                        checkpoint
                            .rejections
                            .insert(scheme_id.clone(), reason.clone());
                        store.save(checkpoint).map_err(SessionError::Persist)?;
                        quarantine_scheme(
                            &scheme_id,
                            name_map,
                            QuarantineKind::Degraded,
                            &reason,
                            &plan.plan_guid,
                            observer,
                            events,
                        );
                        clear_testing_marker();
                        continue 'scheme;
                    }
                    Err(PhaseFailure::UserCancelled) => {
                        cancelled = true;
                        clear_testing_marker();
                        break 'outer;
                    }
                    Err(PhaseFailure::LoadDidNotStop) => {
                        clear_testing_marker();
                        return Err(SessionError::LoadDidNotStop);
                    }
                    Err(PhaseFailure::Engine(err)) => {
                        clear_testing_marker();
                        return Err(SessionError::Engine(err));
                    }
                }
            }
            let duration_ms = phase_started.elapsed().as_millis() as u64;
            let run_ended_secs = now_unix_secs();

            let light_avg = times_by_phase
                .iter()
                .find(|(i, _)| *i == Phase::Light.index())
                .and_then(|(_, t)| run_stats(t))
                .map(|s| s.average_throughput)
                .unwrap_or(0.0);
            // Индекс берём у самой фазы, а не литералом: после появления фазы
            // «Частичная» литерал 1 перестал быть «Тяжёлой», и метрика
            // удержания темпа молча считалась по чужой фазе.
            let heavy_avg = times_by_phase
                .iter()
                .find(|(i, _)| *i == Phase::Heavy.index())
                .and_then(|(_, t)| run_stats(t))
                .map(|s| s.average_throughput)
                .unwrap_or(0.0);
            let burst = burst_retention_percent(heavy_avg, light_avg);

            // Объединённая статистика прогона.
            let mut all_times: Vec<f64> = Vec::new();
            let mut group_refs: Vec<(u32, Vec<f64>)> = Vec::new();
            for (_, times) in &times_by_phase {
                all_times.extend_from_slice(times);
                group_refs.push((0u32, times.clone()));
            }
            let combined = run_stats(&all_times).unwrap_or_else(zero_run_stats);
            let combined_groups: Vec<(u32, &[f64])> =
                group_refs.iter().map(|(g, t)| (*g, t.as_slice())).collect();
            let cross = consistency_percent(&combined_groups);

            // Фоновые корреляции по окнам скачков этого прогона.
            let map_snapshot = monitor_map.lock().unwrap().clone();
            let background = correlate(&all_spike_windows, &map_snapshot);
            // Непрерывная оценка фона: сэмплер уже ходит раз в секунду на
            // протяжении всех фаз, но вердикт до этого строился на одной
            // пробе 700 мс *перед* прогоном. Пятисекундная проверка Defender
            // попадала в неё случайно, а постоянная внешняя нагрузка (VPN,
            // синхронизация) либо не попадала вовсе, либо обрушивала «чистоту»
            // в ноль. Теперь у прогона есть собственные p50/p95.
            let background_cpu =
                background_cpu_stats(&map_snapshot, run_started_secs, run_ended_secs);

            let run_outcome = Some(RunData {
                key: key.clone(),
                round,
                scheme_id: scheme_id.clone(),
                started_at_ns,
                duration_ms,
                ticks,
                supercycles,
                first_tick_checksums,
                run_checksums,
                phase_times: times_by_phase.clone(),
                combined,
                cross_phase_consistency: cross,
                burst_retention_percent: burst,
                background,
                spike_windows_total: spike_count_total,
                phase_power,
                power: PowerSnapshot::capture(),
                background_cpu,
            });

            if let Some(run) = run_outcome {
                let stored = build_stored_run(&run, plan, name_map);
                checkpoint.runs.push(stored);
                store.save(checkpoint).map_err(SessionError::Persist)?;
                note(
                    observer,
                    events,
                    SessionEvent::RunCompleted {
                        key: key.clone(),
                        ticks: run.ticks,
                        duration_ms: run.duration_ms,
                    },
                );
            }

            // Прогон записан — маркер больше не нужен: зависания уже не будет.
            clear_testing_marker();

            // Охлаждение после прогона, кроме самого последнего в плане.
            if !is_last_planned_run(plan, round, &scheme_id) {
                note(
                    observer,
                    events,
                    SessionEvent::Cooling {
                        seconds: plan.cooling_seconds,
                    },
                );
                if !user_wants_stop(user_cancel) && plan.cooling_seconds > 0 {
                    // Охлаждение прерываемо: пауза до 60 с иначе игнорировала бы
                    // кнопку «Стоп» почти на минуту после каждого прогона.
                    interruptible_sleep(user_cancel, plan.cooling_seconds);
                }
            }
        }
        // Раунд отработан целиком (все схемы прогона записаны): отмечаем ключ
        // раунда, чтобы повторный запуск пропустил ротацию одним махом.
        checkpoint.completed_keys.push(key.clone());
        store.save(checkpoint).map_err(SessionError::Persist)?;
    }

    Ok(cancelled)
}

/// Восстановление исходной схемы (может быть вызвано в любой точке выхода).
#[allow(clippy::too_many_arguments)]
/// Снимает тестовый маркер при любом выходе из сессии — включая ошибку
/// сохранения и панику в потоке.
///
/// Раньше маркер снимался только в ветках, которые до этого доходили явно.
/// Ошибка записи чекпоинта (например, кончилось место на диске) возвращалась
/// раньше, и маркер оставался: следующий запуск видел «тест был прерван» и
/// отправлял нормальную схему в постоянный карантин с ложной причиной.
struct TestingMarkerGuard;

impl Drop for TestingMarkerGuard {
    fn drop(&mut self) {
        clear_testing_marker();
    }
}

/// Возвращает исходную схему питания, если сессия вышла нештатно (паника).
///
/// Штатное восстановление делает [`restore_original`], и оно же взводит флаг
/// `restored` — тогда guard ничего не делает. Без guard паника в потоке сессии
/// оставляла активной тестовую схему до перезапуска приложения.
struct OsRestoreGuard<'a> {
    original: Option<String>,
    driver: &'a dyn SchemeDriver,
    restored: Arc<AtomicBool>,
}

impl Drop for OsRestoreGuard<'_> {
    fn drop(&mut self) {
        if self.restored.load(Ordering::Acquire) {
            return;
        }
        let Some(original) = self.original.clone() else {
            return;
        };
        if let Err(e) = self.driver.set_active(&original) {
            // Паника при разворачивании паники = аварийный останов процесса,
            // поэтому ограничиваемся сообщением: восстановить схему не удалось,
            // и пользователю нужно сказать об этом прямо.
            eprintln!("PowerBench: не удалось вернуть исходную схему питания: {e}");
        }
    }
}

fn restore_original(
    driver: &dyn SchemeDriver,
    checkpoint: &mut Checkpoint,
    events: &mut Vec<SessionEvent>,
    store: &mut dyn CheckpointStore,
    observer: &Option<Arc<dyn TelemetryObserver>>,
) -> Result<(), SessionError> {
    if checkpoint.original_restored {
        return Ok(());
    }
    if let Some(original) = checkpoint.original_scheme_guid.clone() {
        // Проверяем, активна ли уже (например, браковка могла не менять схему).
        let active_guid = driver
            .list_schemes()
            .map_err(SessionError::Config)?
            .into_iter()
            .find(|s| s.active)
            .map(|s| s.guid);
        if active_guid.as_deref() != Some(original.as_str()) {
            driver
                .set_active(&original)
                .map_err(SessionError::RestoreScheme)?;
        }
        note(
            observer,
            events,
            SessionEvent::Restored {
                scheme_id: original.clone(),
                label: "после теста".to_string(),
            },
        );
    } else {
        // Исходную схему так и не запомнили (powercfg не отдал активную).
        // Помечать «восстановлено» нельзя: следующий запуск и восстановление
        // после сбоя увидят «всё в порядке» и ничего не починят.
        note(
            observer,
            events,
            SessionEvent::Warn(
                "исходная схема питания неизвестна — восстанавливать нечего".to_string(),
            ),
        );
        return Ok(());
    }
    checkpoint.original_restored = true;
    store.save(checkpoint).map_err(SessionError::Persist)?;
    Ok(())
}

/// Последний ли это плановый прогон (после него охлаждение не нужно).
fn is_last_planned_run(plan: &SessionConfig, round: u32, scheme_id: &str) -> bool {
    let last_round = plan.repetitions.saturating_sub(1);
    if round != last_round {
        return false;
    }
    let order = round_order(&plan.scheme_ids, last_round);
    order.last().map(|s| s == scheme_id).unwrap_or(false)
    // Совпадение «последнего ключа» и этого прогона.
}

/// Забраковать схему в карантин (best-effort: ошибка записи — предупреждение
/// в журнал, сессия продолжается). При повторной браковке той же схемы
/// журнал не засоряется (quarantine_add идемпотентен).
#[allow(clippy::too_many_arguments)]
/// Записать схему в карантин; вернуть пару (id, причина), если запись состоялась.
///
/// Возврат нужен пост-сессионным правилам: они должны отразить браковку в
/// агрегатах и рекомендации, иначе схема окажется одновременно в карантине и
/// в списке рекомендованных.
fn quarantine_scheme(
    scheme_id: &str,
    name_map: &BTreeMap<String, String>,
    kind: QuarantineKind,
    reason: &str,
    plan_guid: &str,
    observer: &Option<Arc<dyn TelemetryObserver>>,
    events: &mut Vec<SessionEvent>,
) -> Option<(String, String)> {
    let name = name_map
        .get(&scheme_id.to_ascii_lowercase())
        .map(String::as_str);
    match quarantine_add(scheme_id, name, kind, reason, plan_guid) {
        Ok(true) => {
            note(
                observer,
                events,
                SessionEvent::Warn(format!(
                    "Схема «{scheme_id}» отправлена в карантин ({}): {reason}",
                    kind.label()
                )),
            );
            Some((scheme_id.to_string(), reason.to_string()))
        }
        Ok(false) => None,
        Err(e) => {
            note(
                observer,
                events,
                SessionEvent::Warn(format!(
                    "не удалось записать карантин схемы «{scheme_id}»: {e}"
                )),
            );
            None
        }
    }
}

/// Медиана массива (копия сортируется). Пустой массив — 0.0.
fn median_of(values: &[f64]) -> f64 {
    let mut valid: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if valid.is_empty() {
        return 0.0;
    }
    valid.sort_by(|a, b| a.total_cmp(b));
    let n = valid.len();
    if n % 2 == 1 {
        valid[n / 2]
    } else {
        (valid[n / 2 - 1] + valid[n / 2]) / 2.0
    }
}

/// Пост-сессия: карантин нестабильных и деградировавших схем по статистике
/// всех записанных прогонов. Вызывается один раз после агрегации.
/// Отказ относительно собственной истории машины при единственной схеме.
///
/// Сравнивать не с чем — в сессии одна схема. Единственный честный ориентир —
/// лучший результат этой же машины при той же конфигурации; без него решение не
/// принимается. Раньше здесь стояла константа «100 тик/с», и это была ошибка:
/// сто тиков — много для ноутбука и мало для станции, то есть на медленной
/// машине она браковала бы здоровые схемы, а на быстрой — пропускала бы
/// поломку.
#[allow(clippy::too_many_arguments)]
fn quarantine_absolute_floor(
    checkpoint: &Checkpoint,
    medians: &BTreeMap<String, Vec<f64>>,
    plan_guid: &str,
    name_map: &BTreeMap<String, String>,
    observer: &Option<Arc<dyn TelemetryObserver>>,
    events: &mut Vec<SessionEvent>,
    baseline: Option<&crate::history::MachineBaseline>,
    written: &mut Vec<(String, String)>,
) {
    let canon = |lower: &str| {
        checkpoint
            .runs
            .iter()
            .find(|r| r.scheme_id.eq_ignore_ascii_case(lower))
            .map(|r| r.scheme_id.clone())
            .unwrap_or_else(|| lower.to_string())
    };
    let reference = baseline.map(|b| b.best_median);
    for (lower, values) in medians {
        let own = median_of(values);
        if !below_machine_baseline(own, reference) {
            continue;
        }
        let Some(best) = reference else {
            continue;
        };
        if let Some(entry) = quarantine_scheme(
            &canon(lower),
            name_map,
            QuarantineKind::Degraded,
            &format!(
                "медиана {own:.1} тик/с — менее {:.0}% от лучшего результата \
                 этой же машины ({best:.1})",
                MACHINE_COLLAPSE_SHARE * 100.0
            ),
            plan_guid,
            observer,
            events,
        ) {
            written.push(entry);
        }
    }
}

/// Типичный P1 каждой фазы среди всех схем сессии.
///
/// Берётся медиана по прогонам, а не максимум: один аномально удачный
/// отрезок у любой схемы поднимал планку, после чего здоровая схема
/// «проваливала такты» и попадала в постоянный карантин. Медиана к одному
/// выбросу устойчива, а при двух и более прогонах отражает устойчивый уровень.
fn typical_p1_per_phase(checkpoint: &Checkpoint) -> BTreeMap<u8, f64> {
    let mut per_phase: BTreeMap<u8, Vec<f64>> = BTreeMap::new();
    for r in &checkpoint.runs {
        for ph in &r.phases {
            let p1 = ph.stats.p1_throughput;
            if p1.is_finite() && p1 > 0.0 {
                per_phase.entry(ph.phase_index).or_default().push(p1);
            }
        }
    }
    per_phase
        .into_iter()
        .filter_map(|(idx, values)| {
            let m = median_of(&values);
            if m.is_finite() && m > 0.0 {
                Some((idx, m))
            } else {
                None
            }
        })
        .collect()
}

/// Причина провала фазы по худшему окну, если она есть.
///
/// Схема, чей P1 в какой-либо фазе — малая доля от P1 лидера в этой же фазе.
/// Возвращаем `None`, если такого провала нет.
fn phase_collapse_reason(
    checkpoint: &Checkpoint,
    scheme_id: &str,
    typical_p1_by_phase: &BTreeMap<u8, f64>,
) -> Option<String> {
    // Самая глубокая фаза побеждает: сначала ищем худший относительный провал.
    let mut worst: Option<(f64, String)> = None;
    for r in &checkpoint.runs {
        if !r.scheme_id.eq_ignore_ascii_case(scheme_id) {
            continue;
        }
        for ph in &r.phases {
            let own = ph.stats.p1_throughput;
            let Some(&leader) = typical_p1_by_phase.get(&ph.phase_index) else {
                continue;
            };
            if !phase_floor_collapse(own, leader) {
                continue;
            }
            let label = Phase::from_index(ph.phase_index)
                .map(|p| p.label())
                .unwrap_or("неизвестная");
            if worst
                .as_ref()
                .is_none_or(|(share, _)| own / leader < *share)
            {
                worst = Some((
                    own / leader,
                    format!(
                        "в фазе «{label}» худшее окно всего {own:.0} тик/с против \
                         {leader:.0} у лучшей схемы — схема проваливает такты"
                    ),
                ));
            }
        }
    }
    worst.map(|(_, reason)| reason)
}

/// Какая именно фаза дала разные контрольные суммы между прогонами.
///
/// Сообщение агрегатора называет симптом, а не причину: без фазы приходилось
/// перебирать все четыре вручную. Возвращает пустую строку, если все суммы
/// совпали (тогда причина другая и подсказка только сбила бы).
fn checksum_phase_hint(runs: &[&StoredRun]) -> String {
    let first = match runs.first() {
        Some(r) => *r,
        None => return String::new(),
    };
    for idx in 0..first.first_tick_checksums.len() {
        let head = first.first_tick_checksums[idx];
        if runs
            .iter()
            .skip(1)
            .any(|r| r.first_tick_checksums[idx] != head)
        {
            let label = Phase::from_index(idx as u8)
                .map(|p| p.label())
                .unwrap_or("неизвестная");
            return format!(" (расходится фаза «{label}»)");
        }
    }
    String::new()
}

fn quarantine_post_session(
    checkpoint: &Checkpoint,
    plan_guid: &str,
    name_map: &BTreeMap<String, String>,
    observer: &Option<Arc<dyn TelemetryObserver>>,
    events: &mut Vec<SessionEvent>,
    baseline: Option<&crate::history::MachineBaseline>,
) -> Vec<(String, String)> {
    let mut written: Vec<(String, String)> = Vec::new();
    let mut medians: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for r in &checkpoint.runs {
        // Нестабильность оцениваем по МЕДИАНЕ прогона, а не по среднему:
        // среднее внутри прогона зашумлено всплесками фоновой нагрузки, и
        // сравнивать такие значения между прогонами бессмысленно.
        let med = r.combined.median_throughput;
        if med.is_finite() && med > 0.0 {
            medians
                .entry(r.scheme_id.to_ascii_lowercase())
                .or_default()
                .push(med);
        }
    }
    if medians.len() < 2 {
        // Одна схема: сравнивать не с чем. Единственный ориентир в этом случае —
        // собственная история машины, и без неё решение не принимается.
        quarantine_absolute_floor(
            checkpoint,
            &medians,
            plan_guid,
            name_map,
            observer,
            events,
            baseline,
            &mut written,
        );
        return written;
    }
    let best_median = medians
        .values()
        .map(|v| median_of(v))
        .fold(0.0f64, f64::max);
    // Канонический GUID для журнала (первое встречное написание из прогонов).
    let canon = |lower: &str| {
        checkpoint
            .runs
            .iter()
            .find(|r| r.scheme_id.eq_ignore_ascii_case(lower))
            .map(|r| r.scheme_id.clone())
            .unwrap_or_else(|| lower.to_string())
    };
    let mut keys: Vec<String> = medians.keys().cloned().collect();
    keys.sort();
    // Типичный P1 по каждой фазе: опора для правила провала фазы. Считаем до
    // цикла, потому что лидер по медиане не обязательно лидер по худшему окну.
    let typical_p1_by_phase = typical_p1_per_phase(checkpoint);
    for lower in keys {
        let id = canon(&lower);
        let run_medians = &medians[&lower];
        // Лидер по медиане деградировать не может — сравнивать его с собой.
        let own_median = median_of(run_medians);
        if let Some(spread) =
            unstable_spread(run_medians, MIN_RUNS_FOR_JUDGMENT, UNSTABLE_MAD_LIMIT)
        {
            if let Some(entry) = quarantine_scheme(
                &id,
                name_map,
                QuarantineKind::Unstable,
                &format!(
                    "разброс прогонов {spread:.0}% от медианы при {} прогонах \
                     (порог {UNSTABLE_MAD_LIMIT:.0}%)",
                    run_medians.len()
                ),
                plan_guid,
                observer,
                events,
            ) {
                written.push(entry);
            }
            continue;
        }
        // Провал фазы по худшему окну: живая медиана, но P1 в разы ниже, чем у
        // лидера той же фазы. Ловится с одного прогона, потому что сравнение
        // одновременное, и медленная машина сокращается из обеих сторон.
        if let Some(reason) = phase_collapse_reason(checkpoint, &id, &typical_p1_by_phase) {
            if let Some(entry) = quarantine_scheme(
                &id,
                name_map,
                QuarantineKind::Degraded,
                &reason,
                plan_guid,
                observer,
                events,
            ) {
                written.push(entry);
            }
            continue;
        }
        // Катастрофическая деградация проверяется уже по одному прогону: в
        // режиме «Быстро» раунд ровно один, и без этого правила заведомо
        // сломанная схема проходила замер наравне с рабочей.
        if catastrophic_share(own_median, best_median) {
            if let Some(entry) = quarantine_scheme(
                &id,
                name_map,
                QuarantineKind::Degraded,
                &format!(
                    "медиана {own_median:.1} тик/с — менее {:.0}% от лучшей ({best_median:.1})",
                    CATASTROPHIC_SHARE_LIMIT * 100.0
                ),
                plan_guid,
                observer,
                events,
            ) {
                written.push(entry);
            }
            continue;
        }
        // MSRV 1.85: схлопывание через let-цепочки требует Rust 1.88+.
        #[allow(clippy::collapsible_if)]
        if run_medians.len() >= DEGRADED_MIN_RUNS {
            if degraded_share(own_median, best_median, DEGRADED_SHARE_LIMIT) {
                if let Some(entry) = quarantine_scheme(
                    &id,
                    name_map,
                    QuarantineKind::Degraded,
                    &format!(
                        "медиана {own_median:.1} тик/с — менее {:.0}% от лучшей ({best_median:.1})",
                        DEGRADED_SHARE_LIMIT * 100.0
                    ),
                    plan_guid,
                    observer,
                    events,
                ) {
                    written.push(entry);
                }
            }
        }
    }
    written
}

fn matched_stats(times: &[f64]) -> RunStats {
    run_stats(times).unwrap_or_else(zero_run_stats)
}

/// Собрать StoredRun из данных прогона. Отображаемое имя схемы (PlanName)
/// берётся из карты «guid → имя», но только для вывода.
fn build_stored_run(
    run: &RunData,
    plan: &SessionConfig,
    name_map: &BTreeMap<String, String>,
) -> StoredRun {
    let _ = plan;
    StoredRun {
        key: run.key.clone(),
        round: run.round,
        scheme_id: run.scheme_id.clone(),
        scheme_name: name_map.get(&run.scheme_id.to_ascii_lowercase()).cloned(),
        started_at_ns: run.started_at_ns,
        duration_ms: run.duration_ms,
        ticks: run.ticks,
        supercycles: run.supercycles,
        first_tick_checksums: run.first_tick_checksums,
        run_checksums: run.run_checksums,
        phases: run
            .phase_times
            .iter()
            .enumerate()
            .map(|(i, (idx, times))| PhaseStats {
                phase_index: *idx,
                stats: matched_stats(times),
                power: run.phase_power.get(i).copied(),
            })
            .collect(),
        combined: run.combined,
        cross_phase_consistency: run.cross_phase_consistency,
        burst_retention_percent: run.burst_retention_percent,
        background: run.background.clone(),
        spike_windows: run.spike_windows_total,
        power: Some(run.power),
        background_cpu_p50: run.background_cpu.0,
        background_cpu_p95: run.background_cpu.1,
        background_sample_seconds: run.background_cpu.2,
    }
}

/// Итог агрегации: агрегаты по схемам, карта «guid → имя», рекомендация.
type AggregationOutcome = (
    Vec<(String, AggregateResult)>,
    BTreeMap<String, String>,
    Option<Recommendation>,
);

/// Агрегация по схемам и формирование рекомендации.
fn build_aggregation(
    checkpoint: &Checkpoint,
    signature: &CompatibilitySignature,
    expected_runs: usize,
) -> AggregationOutcome {
    let mut rejection_reasons = BTreeMap::new();
    let mut aggregates: Vec<(String, AggregateResult)> = Vec::new();
    let mut items: Vec<SchemeAggregate> = Vec::new();

    let mut scheme_ids: Vec<String> = checkpoint.plan.scheme_ids.clone();
    // Схемы, отсутствующие в плане, но имеющие прогоны, добавляем в конец
    // (поиск по guid, регистронезависимый; имена не участвуют).
    for run in &checkpoint.runs {
        if !scheme_ids
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&run.scheme_id))
        {
            scheme_ids.push(run.scheme_id.clone());
        }
    }

    let is_original = |scheme_id: &str| -> bool {
        checkpoint
            .original_scheme_guid
            .as_deref()
            .map(|o| o.eq_ignore_ascii_case(scheme_id))
            .unwrap_or(false)
    };

    for scheme_id in scheme_ids {
        let rejected = checkpoint.is_rejected(&scheme_id);
        if rejected {
            if let Some(reason) = checkpoint.rejection_reason(&scheme_id) {
                rejection_reasons.insert(scheme_id.clone(), reason.clone());
            }
            items.push(SchemeAggregate {
                scheme_id: scheme_id.clone(),
                rejected: true,
                signature: signature.clone(),
                determinism: DeterminismSignature::new(Vec::new()),
                aggregate: AggregateResult {
                    runs: 0,
                    mean_average_throughput: f64::NAN,
                    sample_std: 0.0,
                    t_value: 0.0,
                    margin: 0.0,
                    ci_95: [0.0, 0.0],
                    run_variation_percent: 0.0,
                    cv_warning: false,
                    median_throughput: 0.0,
                    median_p1_throughput: 0.0,
                    median_p01_throughput: 0.0,
                    median_p95_execution_time_ms: 0.0,
                    median_p99_execution_time_ms: 0.0,
                    median_consistency_percent: 0.0,
                    median_burst_retention_percent: 0.0,
                    median_jitter_p99_ms: 0.0,
                    median_worst_window_throughput: 0.0,
                    median_background_purity: None,
                    run_duration_ms: 0,
                    started_at_min_ns: 0,
                },
                runs: Vec::new(),
                is_original: is_original(&scheme_id),
                is_active: is_original(&scheme_id),
            });
            continue;
        }
        let runs: Vec<&StoredRun> = checkpoint
            .runs
            .iter()
            .filter(|r| r.scheme_id.eq_ignore_ascii_case(&scheme_id))
            .collect();
        if runs.is_empty() {
            continue;
        }
        let summaries: Vec<powerbench_metrics::RunSummary> = runs
            .iter()
            .map(|r| r.to_summary(signature.clone()))
            .collect();
        let aggregate = match aggregate_runs(&summaries) {
            Ok(a) => a,
            Err(e) => {
                rejection_reasons.insert(
                    scheme_id.clone(),
                    // Называем фазу: «контрольная сумма различается» без фазы
                    // бесполезно — приходится гадать, где искать.
                    format!(
                        "не удалось агрегировать прогоны: {e}{}",
                        checksum_phase_hint(&runs)
                    ),
                );
                continue;
            }
        };
        let compact: Vec<RunCompact> = runs
            .iter()
            .map(|r| RunCompact::from_stats(&r.combined))
            .collect();
        let first_det = DeterminismSignature::new(
            runs.first()
                .map(|r| r.first_tick_checksums.to_vec())
                .unwrap_or_default(),
        );
        items.push(SchemeAggregate {
            scheme_id: scheme_id.clone(),
            rejected: false,
            signature: signature.clone(),
            determinism: first_det,
            aggregate: aggregate.clone(),
            runs: compact,
            is_original: is_original(&scheme_id),
            is_active: is_original(&scheme_id),
        });
        aggregates.push((scheme_id, aggregate));
    }

    let recommendation = recommend(&items, expected_runs);
    (aggregates, rejection_reasons, Some(recommendation))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Драйвер, запоминающий переключения схемы.
    struct RecordingDriver {
        active: Mutex<String>,
        calls: Mutex<Vec<String>>,
    }

    impl RecordingDriver {
        fn new(initial: &str) -> Self {
            Self {
                active: Mutex::new(initial.to_string()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl SchemeDriver for RecordingDriver {
        fn list_schemes(&self) -> Result<Vec<PowerScheme>, String> {
            let active = self
                .active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            Ok(vec![PowerScheme {
                guid: active,
                name: "Схема".into(),
                active: true,
            }])
        }

        fn set_active(&self, guid: &str) -> Result<(), String> {
            *self.active.lock().unwrap_or_else(|e| e.into_inner()) = guid.to_string();
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(guid.to_string());
            Ok(())
        }

        fn ac_power_online(&self) -> Result<bool, String> {
            Ok(true)
        }

        fn is_admin(&self) -> bool {
            true
        }
    }

    /// Паника в сессии возвращает исходную схему питания.
    ///
    /// Раньше guard создавался после `run_session_loop`, и паника внутри цикла
    /// разворачивала стек мимо него: тестовая схема оставалась активной до
    /// перезапуска приложения, а маркер прогона — на диске, и следующий запуск
    /// отправлял исправную схему в карантин как HardFreeze.
    #[test]
    fn os_restore_guard_returns_scheme_on_panic() {
        let driver = RecordingDriver::new("original-guid");
        let restored = Arc::new(AtomicBool::new(false));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = OsRestoreGuard {
                original: Some("original-guid".to_string()),
                driver: &driver,
                restored: Arc::clone(&restored),
            };
            panic!("движок упал посреди фазы");
        }));
        assert!(outcome.is_err(), "паника должна дойти до catch_unwind");
        assert_eq!(
            *driver.active.lock().unwrap(),
            "original-guid",
            "после паники активной осталась тестовая схема"
        );
        assert_eq!(driver.calls.lock().unwrap().len(), 1);
    }

    /// Штатно восстановленная схема повторно не переключается.
    #[test]
    fn os_restore_guard_is_silent_after_restore() {
        let driver = RecordingDriver::new("original-guid");
        let restored = Arc::new(AtomicBool::new(true));
        drop(OsRestoreGuard {
            original: Some("original-guid".to_string()),
            driver: &driver,
            restored: Arc::clone(&restored),
        });
        assert!(
            driver.calls.lock().unwrap().is_empty(),
            "после штатного восстановления схему переключили заново"
        );
    }

    /// Guard создаётся до вызова цикла, а не после.
    ///
    /// Порядок объявления в `run_session` — часть контракта: только guard,
    /// живущий на Drop, спасает при панике. Проверяется по исходнику, потому
    /// что воспроизвести панику именно в этой точке без настоящего движка
    /// нельзя, а регресс здесь молчаливый: код компилируется и работает, пока
    /// не случится именно то, ради чего guard и написан.
    #[test]
    fn restore_guards_are_created_before_the_session_loop() {
        let src = include_str!("session.rs");
        let start = src
            .find("pub fn run_session(")
            .expect("не найдена run_session");
        let body = &src[start..];
        let guards = body
            .find("let _os_guard = OsRestoreGuard {")
            .expect("OsRestoreGuard не создан в run_session");
        let marker = body
            .find("let _marker_guard = TestingMarkerGuard;")
            .expect("TestingMarkerGuard не создан в run_session");
        let loop_call = body
            .find("let loop_result = run_session_loop(")
            .expect("вызов run_session_loop не найден");
        assert!(
            guards < loop_call,
            "OsRestoreGuard создан после вызова цикла: паника развернёт стек мимо него"
        );
        assert!(
            marker < loop_call,
            "TestingMarkerGuard создан после вызова цикла: маркер останется на диске"
        );
    }

    /// Медленно ≠ сломано: пока разгон не закончился, низкий темп нормален.
    #[test]
    fn collapse_detector_ignores_slow_start() {
        let mut d = CollapseDetector::with_reference(Some(1000.0));
        // Первые секунды фазы — низкий темп из-за разгона, обрыва быть не должно,
        // сколько бы секунд мы ни наблюдали.
        for t in 0..COLLAPSE_ARM_SECS {
            assert_eq!(
                d.observe(5, Duration::from_secs(t)),
                None,
                "разгон на {t}-й секунде не должен считаться поломкой"
            );
        }
    }

    /// Без ориентира по истории машины решение не принимается вовсе.
    #[test]
    fn collapse_detector_is_silent_without_machine_baseline() {
        let mut d = CollapseDetector::with_reference(None);
        for t in 0..600 {
            assert_eq!(
                d.observe(1, Duration::from_secs(t)),
                None,
                "без истории машины обрывать нельзя: норма неизвестна"
            );
        }
    }

    /// Порог считается от того, что машина показывала раньше, а не от
    /// константы: медленная машина не должна обрываться за «нормальный» темп.
    #[test]
    fn collapse_detector_threshold_follows_the_machine() {
        // Медленная машина: её норма — 100 тиков, обрыв ниже 20.
        let mut slow = CollapseDetector::with_reference(Some(100.0));
        assert_eq!(slow.observe(50, Duration::from_secs(5)), None);
        // Быстрая машина: норма — 5000, те же 50 тиков — обрыв.
        let mut fast = CollapseDetector::with_reference(Some(5000.0));
        let mut fired = false;
        for t in COLLAPSE_ARM_SECS..=20 {
            if fast.observe(50, Duration::from_secs(t)).is_some() {
                fired = true;
                break;
            }
        }
        assert!(fired, "на быстрой машине 50 тиков — это обрыв");
        // А на медленной машине 50 тиков — норма, и 10 тиков уже нет.
        let mut stopped = false;
        for t in COLLAPSE_ARM_SECS..=20 {
            if slow.observe(10, Duration::from_secs(t)).is_some() {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "на медленной машине 10 тиков — это обрыв");
    }

    /// Реальный случай: план душит машину до единиц тиков в секунду.
    #[test]
    fn collapse_detector_stops_a_crawling_machine() {
        let mut d = CollapseDetector::with_reference(Some(1000.0));
        let mut stopped_at = None;
        for t in COLLAPSE_ARM_SECS..=20 {
            if d.observe(10, Duration::from_secs(t)).is_some() {
                stopped_at = Some(t);
                break;
            }
        }
        assert_eq!(
            stopped_at,
            Some(COLLAPSE_ARM_SECS + COLLAPSE_HOLD_SECS),
            "обрыв должен наступить через {COLLAPSE_HOLD_SECS} с после разгона"
        );
    }

    /// Единичный просад не обрывает замер: темп считается по одному батчу и
    /// сам скачет.
    #[test]
    fn collapse_detector_survives_single_dip() {
        let mut d = CollapseDetector::with_reference(Some(1000.0));
        assert_eq!(d.observe(5000, Duration::from_secs(5)), None);
        assert_eq!(d.observe(10, Duration::from_secs(6)), None);
        // Темп вернулся — отсчёт сброшен, до упора держимся.
        assert_eq!(d.observe(5000, Duration::from_secs(7)), None);
        assert_eq!(d.observe(5000, Duration::from_secs(20)), None);
    }

    /// Здоровая машина с тиками тысячами не должна обрываться никогда.
    #[test]
    fn collapse_detector_stays_quiet_on_a_healthy_machine() {
        let mut d = CollapseDetector::with_reference(Some(4000.0));
        for t in 0..600 {
            assert_eq!(d.observe(3000, Duration::from_secs(t)), None);
        }
    }

    /// Порог устойчив к выбросам: считается по медианному абсолютному
    /// отклонению, а не по σ.
    #[test]
    fn spike_threshold_is_robust_to_outliers() {
        assert_eq!(spike_threshold(&[]), 0.0);

        // 1000 тиков по 1.0 мс и 50 огромных скачков. Формула σ раздула бы
        // порог выше самих скачков; MAD держит его в разумных пределах.
        let mut times = vec![1.0f64; 1000];
        times.extend(std::iter::repeat_n(500.0, 50));
        let threshold = spike_threshold(&times);
        assert!(
            threshold > 1.0 && threshold < 10.0,
            "порог {threshold} неустойчив к выбросам"
        );
        // Все реальные скачки находятся, фоновые значения — нет.
        let windows = spike_windows_for(&times, threshold, 0, 0.001, "Тяжёлая");
        assert_eq!(windows.len(), 1, "скачки не объединились в одно окно");
    }

    /// Почти постоянные времена: порог всё равно должен что-то ловить.
    #[test]
    fn spike_threshold_handles_zero_variance() {
        let threshold = spike_threshold(&[2.0; 100]);
        assert!(threshold > 2.0, "при нулевом разбросе порог не поднялся");
        assert_eq!(
            spike_windows_for(&[2.0; 100], threshold, 0, 0.01, "X").len(),
            0
        );
    }

    #[test]
    fn spike_windows_group_consecutive_overflows() {
        // Порог = 10: превышение на индексах 2..=3 и 6.
        let times = vec![1.0, 2.0, 11.0, 12.0, 3.0, 4.0, 20.0];
        let windows = spike_windows_for(&times, 10.0, 1000, 1.0, "Лёгкая");
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].start_second, 1002);
        assert_eq!(windows[0].end_second, 1004);
        assert_eq!(windows[1].start_second, 1006);
        assert_eq!(windows[1].end_second, 1007);
        assert_eq!(windows[0].phase_label, "Лёгкая");
    }

    #[test]
    fn no_overflows_mean_no_windows() {
        let windows = spike_windows_for(&[1.0, 2.0, 3.0], 5.0, 0, 1.0, "X");
        assert!(windows.is_empty());
    }

    const fn fallback_stats() -> RunStats {
        zero_run_stats()
    }

    #[test]
    fn last_planned_run_detection() {
        let plan = SessionConfig {
            duration_seconds: 9,
            warmup_seconds: 2,
            cooling_seconds: 1,
            repetitions: 2,
            background_threshold_percent: 5.0,
            worker_count: None,
            scheme_ids: vec!["a".to_string(), "b".to_string()],
            plan_guid: "g".to_string(),
            reference_scheme_id: None,
        };
        // В последнем раунде (1): cycle 0, source [a,b], shift 1 → порядок [b, a],
        // последняя схема — «a».
        assert!(!is_last_planned_run(&plan, 0, "a"));
        assert!(is_last_planned_run(&plan, 1, "a"));
        assert!(!is_last_planned_run(&plan, 1, "b"));
    }

    #[test]
    fn fallback_stats_is_zero() {
        let s = fallback_stats();
        assert_eq!(s.samples, 0);
        assert_eq!(s.average_throughput, 0.0);
    }

    #[test]
    fn phase_label_map() {
        assert_eq!(phase_label(Phase::Light), "Лёгкая");
        assert_eq!(phase_label(Phase::Heavy), "Тяжёлая");
        assert_eq!(phase_label(Phase::Response), "Отклик");
    }

    #[test]
    fn note_forwards_to_observer_and_log() {
        struct Probe {
            received: Arc<Mutex<Vec<SessionEvent>>>,
        }
        impl TelemetryObserver for Probe {
            fn event(&self, e: &SessionEvent) {
                self.received.lock().unwrap().push(e.clone());
            }
        }
        let seen: Arc<Mutex<Vec<SessionEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let obs: Option<Arc<dyn TelemetryObserver>> = Some(Arc::new(Probe {
            received: Arc::clone(&seen),
        }));
        let mut events = Vec::new();
        note(
            &obs,
            &mut events,
            SessionEvent::Started {
                plan_guid: "g".to_string(),
                rounds: 1,
                schemes: 1,
            },
        );
        assert_eq!(events.len(), 1);
        assert_eq!(seen.lock().unwrap().len(), 1);
        note(&None, &mut events, SessionEvent::Finished);
        assert_eq!(events.len(), 2);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn telemetry_pump_delivers_ticks_and_exits() {
        struct TickRecorder {
            ticks: Arc<Mutex<Vec<u64>>>,
        }
        impl TelemetryObserver for TickRecorder {
            fn tick(&self, snap: &ProgressSnapshot, _label: &str, _start: &Instant) {
                self.ticks.lock().unwrap().push(snap.ticks_done);
            }
        }
        let mut engine = Engine::new(Some(1));
        let monitor_map: Arc<Mutex<BTreeMap<u64, Vec<ProcessSample>>>> =
            Arc::new(Mutex::new(BTreeMap::new()));
        let recorded: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let obs: Option<Arc<dyn TelemetryObserver>> = Some(Arc::new(TickRecorder {
            ticks: Arc::clone(&recorded),
        }));
        let result = run_measured_phase(
            &mut engine,
            Arc::new(AtomicBool::new(false)),
            Phase::Heavy,
            1,
            "фаза-тест",
            &monitor_map,
            obs,
            None,
        );
        assert!(result.is_ok());
        let got = recorded.lock().unwrap().len();
        assert!(got >= 1, "ожидались тики телеметрии, получено {got}");
        assert!(!engine.progress_snapshot().running);
    }
}
