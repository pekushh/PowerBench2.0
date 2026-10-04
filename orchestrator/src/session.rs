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
    RunStats, burst_retention_percent, consistency_group, median, run_stats,
};
use powerbench_metrics::{
    AggregateResult, CompatibilitySignature, DeterminismSignature, aggregate_runs,
};
use powerbench_recommend::{Recommendation, RunCompact, SchemeAggregate, recommend};
use powerbench_windows::monitor::{
    CorrelatedProcess, ProcessSample, ProcessSampler, SpikeWindow, correlate,
};
use powerbench_windows::powercfg::PowerScheme;

use crate::checkpoint::{Checkpoint, LaunchConditions, PhaseStats, PowerSnapshot, StoredRun};
use crate::config::{
    PHASES_PER_RUN, SessionConfig, canonical_scheme_order, phase_durations, round_order, run_key,
    validate_config,
};
use crate::quarantine::{
    CATASTROPHIC_SHARE_LIMIT, DEGRADED_MIN_RUNS, DEGRADED_SHARE_LIMIT, MACHINE_COLLAPSE_SHARE,
    MIN_RUNS_FOR_JUDGMENT, QuarantineKind, TestingMarker, UNSTABLE_MAD_LIMIT,
    below_machine_baseline, catastrophic_share, clear_testing_marker, degraded_share,
    phase_floor_collapse, preflight_filter, quarantine_add, unstable_spread,
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
pub use crate::config::{
    SCHEME_STABILIZE_ESTIMATE_SECS, SCHEME_STABILIZE_MAX_SECS, SCHEME_STABILIZE_MIN_SECS,
};
/// Длительность одного замера фоновой нагрузки (спецификация).
pub const BACKGROUND_MEASURE_MS: u64 = 700;
/// Пауза между повторными замерами фона (спецификация).
pub const BACKGROUND_RETRY_PAUSE_MS: u64 = 1200;
/// Сколько раз схема в раунде проверяется на «фон не слишком тяжёлый».
///
/// Повторы, а не карантин: тяжёлый фон — свойство машины в данный момент
/// (антивирус, синхронизация, резервное копирование), а не свойство схемы.
pub const BACKGROUND_RUN_ATTEMPTS: u32 = 3;
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
    /// Прогоны в точке сделаны при других условиях запуска, чем текущие.
    ///
    /// Продолжение такого плана смешало бы в одном отчёте числа, измеренные
    /// разным числом воркеров, разной привязкой или разной нагрузкой, —
    /// а ранжировать такую смесь нельзя.
    RunConditionsMismatch {
        difference: String,
        recorded: String,
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
            SessionError::RunConditionsMismatch {
                difference,
                recorded,
            } => write!(
                f,
                "прогоны в контрольной точке сделаны при других условиях запуска ({difference}; \
                 записано: {recorded}). Продолжение смешало бы несравнимые замеры в одном \
                 отчёте — начните новую сессию"
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
    /// Фон слишком тяжёлый: замер не начат, идёт повтор.
    BackgroundTooDirty {
        measured_total_percent: f64,
        threshold_percent: f64,
        attempt: u32,
        attempts: u32,
    },
    /// Прогон признан недействительным и в результаты не попал.
    ///
    /// Отдельное событие, а не `SchemeRejected`, потому что это НЕ свойство
    /// схемы: виновата машина (пропало питание от сети), и карантить схему
    /// здесь нельзя — она в порядке.
    RunInvalid {
        scheme_id: String,
        reason: String,
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
    /// Сессия остановлена досрочно: перевес лидера уже статистически значим.
    EarlyStopped {
        reason: String,
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
            SessionEvent::BackgroundTooDirty {
                measured_total_percent,
                threshold_percent,
                attempt,
                attempts,
            } => {
                if attempt < attempts {
                    write!(
                        f,
                        "Фон слишком загружен ({measured_total_percent:.1}% при критическом пороге \
                         {threshold_percent:.1}% суммарно), замер отложен ({attempt}/{attempts})"
                    )
                } else {
                    write!(
                        f,
                        "Фон слишком загружен ({measured_total_percent:.1}% при критическом пороге \
                         {threshold_percent:.1}% суммарно), прогон пропущен после {attempts} попыток"
                    )
                }
            }
            SessionEvent::RunInvalid { scheme_id, reason } => {
                write!(f, "Прогон недействителен (схема {scheme_id}): {reason}")
            }
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
            SessionEvent::EarlyStopped { reason } => {
                write!(f, "Сессия остановлена досрочно: {reason}")
            }
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
    /// GUID активной схемы — чтение состояния, а не установка.
    ///
    /// Нужен для ПОДТВЕРЖДЕНИЯ переключения и для обнаружения подмены схемы
    /// посторонним процессом. `powercfg /setactive` возвращает код 0 и при
    /// этом ничего не переключает (конфликт политики, OEM-агент), поэтому
    /// «команда прошла» и «схема применена» — разные утверждения.
    fn active_scheme(&self) -> Result<String, String>;
    fn ac_power_online(&self) -> Result<bool, String>;
    fn is_admin(&self) -> bool;
}

/// Сколько раз повторяем чтение активной схемы после `setactive`.
///
/// Windows применяет план асинхронно: команда возвращается, а запись может
/// дойти до Power Manager позже. Один poll сразу после команды попадал в эту
/// дыру и давал ложное «ОС не переключила схему».
pub const SCHEME_APPLY_ATTEMPTS: u32 = 4;
/// Пауза между попытками подтверждения смены схемы, мс.
pub const SCHEME_APPLY_RETRY_MS: u64 = 250;

/// Применить схему и **убедиться**, что ОС её приняла.
///
/// `powercfg /setactive` проверяет только код возврата процесса. Если схема не
/// применилась, замер пойдёт на чужой схеме, и в отчёте это неотличимо от
/// штатного результата — хуже всего. Поэтому единственная точка применения
/// схемы во всём проекте — эта функция.
pub fn apply_and_verify(driver: &dyn SchemeDriver, guid: &str) -> Result<(), String> {
    driver
        .set_active(guid)
        .map_err(|e| format!("команда применения не удалась: {e}"))?;
    let mut last_seen = String::new();
    for attempt in 0..SCHEME_APPLY_ATTEMPTS {
        match driver.active_scheme() {
            Ok(active) => {
                if active.eq_ignore_ascii_case(guid) {
                    return Ok(());
                }
                last_seen = active;
            }
            Err(e) => {
                // Нечитаемое состояние — не «применилось». Молчать здесь
                // опаснее, чем лишний повтор.
                last_seen = format!("не прочитано ({e})");
            }
        }
        if attempt + 1 < SCHEME_APPLY_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(SCHEME_APPLY_RETRY_MS));
        }
    }
    Err(format!(
        "ОС не переключила схему за {} мс: активна «{last_seen}», требовалась «{guid}»",
        SCHEME_APPLY_ATTEMPTS as u64 * SCHEME_APPLY_RETRY_MS
    ))
}

/// Сколько раз повторяем ЧТО ИМЕННО: возврат исходной схемы дороже отказа
/// применения тестовой.
///
/// Применение тестовой схемы можно повторить ещё раз. Возврат исходной — нет:
/// если пользовательский план не вернулся, машина остаётся на тестовом, и
/// следующий запуск не сможет ни подтвердить восстановление, ни отличить
/// «уже вернули» от «думаем, вернули». Поэтому и попыток больше, и пауза
/// между ними длиннее.
pub const RESTORE_ATTEMPTS: u32 = 3;
/// Пауза между попытками возврата исходной схемы, мс.
pub const RESTORE_RETRY_MS: u64 = 750;

/// Вернуть исходную схему питания и **убедиться**, что ОС её приняла.
///
/// Единственная точка возврата на исходный план во всём проекте: её зовут
/// `restore_original`, `OsRestoreGuard` на панике, продолжение сессии и
/// восстановление после прерывания. Все четыре прежних звали «голый»
/// `set_active`, который проверяет только код возврата `powercfg.exe`:
/// команда завершается успехом, план при этом может не смениться (конфликт
/// доменной политики, OEM-агент). Ставить `original_restored = true` по
/// такому «успеху» нельзя — тогда точка навсегда осталась бы в состоянии
/// «восстановлено», а машина продолжала бы работать на тестовой схеме.
///
/// Повтор делается целиком: и команда, и подтверждение. План мог не
/// примениться из-за занятого Power Manager, и следующая попытка обычно
/// проходит.
pub fn restore_verified(driver: &dyn SchemeDriver, guid: &str) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 0..RESTORE_ATTEMPTS {
        match apply_and_verify(driver, guid) {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
        if attempt + 1 < RESTORE_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(RESTORE_RETRY_MS));
        }
    }
    Err(format!(
        "исходную схему «{guid}» вернуть в ОС не удалось за {} мс: {last}. \
         Схема питания машины осталась другой — верните план вручную \
         (powercfg /list, затем powercfg /setactive <GUID>)",
        RESTORE_ATTEMPTS as u64 * (SCHEME_APPLY_RETRY_MS + RESTORE_RETRY_MS)
    ))
}

/// Убедиться, что во время измерения осталась та же схема.
///
/// Проверка на границе каждой фазы ловит подмену плана посторонним
/// процессом, OEM-утилитой или обновлением Windows. Без неё такой прогон
/// выглядит как обычный результат, хотя измерен был вовсе не тот план.
pub fn verify_scheme_active(driver: &dyn SchemeDriver, expected: &str) -> Result<(), String> {
    match driver.active_scheme() {
        Ok(active) if active.eq_ignore_ascii_case(expected) => Ok(()),
        Ok(active) => Err(format!(
            "активна схема «{active}» вместо «{expected}» — план сменили извне"
        )),
        Err(e) => Err(format!("не удалось прочитать активную схему: {e}")),
    }
}

/// Полный текст настроек схемы (`powercfg /query <guid>`).
///
/// Дамп снимается один раз на прогон: без него отчёт через год содержит
/// только GUID, а по нему не сказать, какие ограничения были заданы, —
/// Windows и OEM-агенты молча правят планы при обновлениях. Съём стоит
/// единицы миллисекунд, поэтому ошибка не должна проваливать измерение:
/// она уходит в предупреждение, а дамп остаётся `None`.
fn capture_scheme_dump(
    guid: &str,
    observer: &Option<Arc<dyn TelemetryObserver>>,
    events: &mut Vec<SessionEvent>,
) -> Option<String> {
    match powerbench_windows::powercfg::query(guid) {
        Ok(dump) => Some(dump),
        Err(e) => {
            note(
                observer,
                events,
                SessionEvent::Warn(format!(
                    "не удалось сохранить настройки схемы {guid}: {}. \
                     Результат верный, но воспроизвести его по одному GUID нельзя",
                    e.message
                )),
            );
            None
        }
    }
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

    fn active_scheme(&self) -> Result<String, String> {
        powerbench_windows::powercfg::active_scheme().map_err(|e| e.message)
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
    /// Настоящая причина досрочной остановки (перевес стал значимым).
    pub early_stop_reason: Option<String>,
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

/// Решение по фоновой загрузке.
///
/// Раньше здесь был булев «чист/не чист», и оба ответа означали одно и то же:
/// замер продолжался. Фоном в 15 % CPU результат занижался молча, а схема
/// получала худшую оценку и могла попасть в карантин по `phase_floor_collapse`
/// с ложной причиной.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundVerdict {
    /// Фон ниже порога.
    Clean { measured: f64 },
    /// Фон выше порога, но не критично: замер продолжается с пометкой.
    Noisy { measured: f64 },
    /// Фон слишком тяжёлый: замер начинать нельзя.
    TooDirty { measured: f64 },
}

/// Во сколько раз порог считается критичным для отказа от замера.
///
/// Не «1 + запас», а именно множитель: 5 % на ядро при 16 ядрах — это 80 %,
/// и фон в 200 % при таком пороге означает загрузку примерно половины
/// машины. Такой фон съедает столько, что сравнивать планы бессмысленно, но
/// это ещё не поломка машины — поэтому отказ с повтором, а не карантин.
pub const BACKGROUND_DIRTY_FACTOR: f64 = 3.0;

/// Классифицировать измеренную фоновую загрузку.
///
/// Нормировка `cpu_percent` у sysinfo — процент **одного** ядра, поэтому порог
/// умножается на число логических CPU.
pub fn classify_background(
    measured: f64,
    threshold_percent: f64,
    logical_cpus: usize,
) -> BackgroundVerdict {
    let threshold = threshold_percent * logical_cpus as f64;
    if !measured.is_finite() {
        // Неизвестную загрузку нельзя считать чистой: это был бы замер по
        // умолчанию «всё хорошо» без единого подтверждения.
        return BackgroundVerdict::TooDirty { measured };
    }
    if measured < threshold {
        BackgroundVerdict::Clean { measured }
    } else if measured < threshold * BACKGROUND_DIRTY_FACTOR {
        BackgroundVerdict::Noisy { measured }
    } else {
        BackgroundVerdict::TooDirty { measured }
    }
}

/// Измерить фоновую загрузку один раз. `None` — пользователь отменил ожидание.
pub fn measure_background(
    logical_cpus: usize,
    threshold_percent: f64,
    user_cancel: &AtomicBool,
) -> Option<BackgroundVerdict> {
    let mut sampler = ProcessSampler::new();
    // Прогрев накопителей: без него первый замер всегда нулевой.
    let _ = sampler.sample();
    // Окно измерения само прерываемо, иначе кнопка «Стоп» ждала бы 700 мс.
    if interruptible_sleep_ms(user_cancel, BACKGROUND_MEASURE_MS) {
        return None;
    }
    let samples = sampler.sample();
    let measured = samples.iter().map(|s| s.cpu_percent).sum();
    Some(classify_background(
        measured,
        threshold_percent,
        logical_cpus,
    ))
}

/// Вердикт сторожевого таймера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogVerdict {
    /// Прогресса нет вовсе: тики не идут. Заведомо поломка машины или
    /// измерительного устройства — в карантин.
    Hung,
    UserCancelled,
    /// Машина СЛОМАЛАСЬ: темп упал относительно того, что она же показывала
    /// минуту назад. Замер недостоверен — в карантин.
    Collapsed {
        ticks_per_sec: u64,
        own_median: u64,
    },
    /// Машина идёт РОВНО, но медленно: темп держится у своей же медианы, то
    /// есть провалов нет. Это может быть и сознательно консервативная схема
    /// (`max processor state` = 50 %), и просто тяжёлая машина.
    ///
    /// Замер при этом достоверен и полезен: медленно — да, но верно. Поэтому
    /// фаза НЕ прерывается и схема НЕ идёт в карантин; в отчёт попадает
    /// только предупреждение с объяснением.
    SlowButStable {
        ticks_per_sec: u64,
        own_median: u64,
    },
}

/// Доля СОБСТВЕННОЙ медианы фазы, ниже которой темп считается поломкой.
///
/// Провал относительно того, что машина показывала минуту назад, — это
/// событие внутри измерения (троттлинг, фон, вытеснение), и замер после
/// него недостоверен.
pub const COLLAPSE_IN_RUN_SHARE: f64 = 0.5;
/// Доля от лучшего результата машины в этой фазе, ниже которой темп считается
/// «низким, но ровным».
///
/// Абсолютный ориентир, взятый из истории машины: 20 % от её лучшего результата
/// означает, что машина работает в пять раз медленнее, чем когда-либо могла.
/// Само по себе это не поломка.
pub const LOW_RATE_SHARE: f64 = 0.20;
/// Первые секунды фазы не смотрим: разгон частот и холодный кэш дают низкий
/// темп даже у совершенно здоровой машины.
pub const COLLAPSE_ARM_SECS: u64 = 4;
/// Сколько секунд темп должен оставаться ниже порога, прежде чем остановиться.
///
/// Несколько секунд, а не один замер: мгновенный темп считается по времени
/// одного батча и сам по себе скачет.
pub const COLLAPSE_HOLD_SECS: u64 = 3;
/// Сколько замеров темпа нужно, чтобы узнать медиану фазы.
///
/// При 4 опросах в секунду это примерно секунда — достаточно, чтобы медиана
/// отражала установившийся темп, и мало, чтобы не тратить память на длинной
/// фазе: 4 замера в секунду на 60-секундной фазе дают 240 чисел, то есть
/// меньше двух килобайт.
pub const RATE_SAMPLES_FOR_MEDIAN: usize = 8;
/// Сколько секунд ровный низкий темп держится до предупреждения.
pub const SLOW_HOLD_SECS: u64 = 3;

/// Детектор темпа фазы: поломка относительно собственной медианы и ровная
/// медлительность относительно истории машины.
///
/// Два ОРИЕНТИРА, и это суть исправления:
///
/// * **Поломка** (`Collapsed`) — темп упал относительно медианы ЭТОГО ЖЕ
///   прогона. Это признак того, что машина сломалась ПО ХОДУ измерения:
///   троттлинг, фон, вытеснение. Замер недостоверен.
/// * **Медлительность** (`SlowButStable`) — темп ровный, но ниже того, что эта
///   машина показывала раньше. Это может быть и сознательно консервативная
///   схема (`max processor state` = 50 %), и просто тяжёлая машина. Замер
///   ДОСТОВЕРЕН: темп не проседал, просто такой.
///
/// Прежнее правило использовало только абсолютный ориентир, и медленная, но
/// ровная машина попадала под тот же вердикт, что и сломанная: и то и другое
/// уходило в перманентный карантин с причиной «схема не тянет». Теперь эти
/// случаи разделены, и карантин достаётся только настоящей поломке.
///
/// Вынесен отдельным типом с явным временем на входе, чтобы правило можно было
/// проверить без реального прогона: иначе любой тест на «обрыв замера» стоил бы
/// минуты работы сторожевого таймера.
#[derive(Debug, Default)]
pub struct RateWatchdog {
    /// Наблюдённые темпы фазы (для собственной медианы).
    samples: Vec<u64>,
    /// С какого момента темп держится ниже половины своей медианы.
    below_since: Option<Duration>,
    /// С какого момента темп ниже абсолютного ориентира машины.
    slow_since: Option<Duration>,
    /// Уже сообщили о медлительности: повторять каждую фазу незачем.
    slow_reported: bool,
    /// Абсолютный ориентир «низкий темп» в тиках в секунду, из истории машины.
    ///
    /// `None` — истории нет, и тогда медлительность просто НЕ ОПРЕДЕЛЯЕТСЯ.
    /// Это лучше, чем угадывать: без ориентира медленная ровная машина не
    /// получает ни одного вердикта, а значит и никогда не карантинится.
    floor_abs: Option<f64>,
}

/// Решение детектора темпа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateVerdict {
    /// Всё в порядке.
    Ok,
    /// Темп провалился относительно собственной медианы — замер недостоверен.
    Collapsed { ticks_per_sec: u64, own_median: u64 },
    /// Темп ровный и низкий, но без провалов — замер достоверен.
    SlowButStable { ticks_per_sec: u64, own_median: u64 },
}

impl RateWatchdog {
    /// Детектор без ориентира «низкий темп»: поломка по своей медиане
    /// определяется, медлительность — нет.
    pub fn new() -> Self {
        Self::default()
    }

    /// Детектор с ориентиром «низкий темп» из истории машины для этой фазы.
    pub fn with_reference(reference_p1: Option<f64>) -> Self {
        Self {
            floor_abs: reference_p1
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| v * LOW_RATE_SHARE),
            ..Self::default()
        }
    }

    /// Отметить очередной замер темпа; вернуть решение, если оно достигнуто.
    pub fn observe(&mut self, ticks_per_sec: u64, elapsed: Duration) -> RateVerdict {
        // До разгона не смотрим вовсе.
        if elapsed.as_secs() < COLLAPSE_ARM_SECS {
            return RateVerdict::Ok;
        }
        if self.samples.len() < RATE_SAMPLES_FOR_MEDIAN {
            self.samples.push(ticks_per_sec);
            return RateVerdict::Ok;
        }
        let own_median = self.own_median();
        if own_median == 0 {
            return RateVerdict::Ok;
        }
        let ratio = ticks_per_sec as f64 / own_median as f64;

        // --- Поломка: темп упал относительно собственной медианы ---
        if ratio < COLLAPSE_IN_RUN_SHARE {
            self.slow_since = None;
            let since = *self.below_since.get_or_insert(elapsed);
            let held = elapsed.saturating_sub(since);
            if held.as_secs() >= COLLAPSE_HOLD_SECS {
                return RateVerdict::Collapsed {
                    ticks_per_sec,
                    own_median,
                };
            }
            return RateVerdict::Ok;
        }
        self.below_since = None;

        // --- Ровная медлительность ---
        //
        // Провала нет, но темп ниже того, что машина показывала раньше. Если
        // так держится несколько секунд — это свойство режима, а не авария:
        // замер остаётся, в отчёт идёт предупреждение, карантина нет.
        if let Some(floor) = self.floor_abs
            && !self.slow_reported
            && (ticks_per_sec as f64) < floor
        {
            let since = *self.slow_since.get_or_insert(elapsed);
            let held = elapsed.saturating_sub(since);
            if held.as_secs() >= SLOW_HOLD_SECS {
                self.slow_reported = true;
                return RateVerdict::SlowButStable {
                    ticks_per_sec,
                    own_median,
                };
            }
            return RateVerdict::Ok;
        }
        self.slow_since = None;
        RateVerdict::Ok
    }

    /// Медиана наблюдённых темпов фазы.
    fn own_median(&self) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut v = self.samples.clone();
        v.sort_unstable();
        v[v.len() / 2]
    }

    /// Медиана темпов фазы (для отчёта и тестов).
    pub fn median_rate(&self) -> u64 {
        self.own_median()
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
    rates: RateWatchdog,
    phase_finished: Arc<AtomicBool>,
) -> (JoinHandle<()>, Receiver<WatchdogVerdict>) {
    let (tx, rx) = mpsc::channel();
    let handle: JoinHandle<()> = std::thread::spawn(move || {
        let mut last_ticks: u64 = 0;
        let mut last_progress = Instant::now();
        let mut rates = rates;
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
            // Темп фазы против ЕЁ СОБСТВЕННОЙ медианы: провал означает поломку
            // (замер недостоверен, карантин), ровная медлительность — нет
            // (замер достоверен, только предупреждение).
            //
            // Медлительность замер НЕ прерывает: фаза и так идёт положенное
            // время, а низкий темп — это характеристика режима, а не событие.
            match rates.observe(
                snap.current_ticks_per_sec,
                Duration::from_secs(snap.elapsed_secs),
            ) {
                RateVerdict::Ok => {}
                RateVerdict::Collapsed {
                    ticks_per_sec,
                    own_median,
                } => {
                    let _ = tx.send(WatchdogVerdict::Collapsed {
                        ticks_per_sec,
                        own_median,
                    });
                    cancel.store(true, Ordering::Relaxed);
                    return;
                }
                RateVerdict::SlowButStable {
                    ticks_per_sec,
                    own_median,
                } => {
                    let _ = tx.send(WatchdogVerdict::SlowButStable {
                        ticks_per_sec,
                        own_median,
                    });
                    // Продолжаем наблюдение: медлительность — не повод
                    // прерывать фазу, а отмена здесь испортила бы замер.
                    return;
                }
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
                // Отравленный мьютекс НЕ должен убивать поток мониторинга
                // (регресс H38): один заход с паникой в другом потоке оставлял бы
                // прогон без фоновых корреляций до конца сессии. `into_inner`
                // отдаёт внутренности и снимает отравление.
                map.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(sec, samples);
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
    interruptible_sleep_ms(user_cancel, seconds.saturating_mul(1000))
}

/// Сон в миллисекундах с быстрой реакцией на отмену пользователя.
///
/// Технические паузы сессии обязаны быть прерываемыми: иначе кнопка «Стоп»
/// игнорируется на всём их протяжении. Раньше не прерывались пауза после
/// схемы, стабилизация после фазы и вся проверка фона — вместе это до 9,5 с
/// молчания после нажатия, при 90 схемах — десятки минут за сессию.
pub fn interruptible_sleep_ms(user_cancel: &AtomicBool, millis: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(millis);
    loop {
        if user_cancel.load(Ordering::Relaxed) {
            return true;
        }
        // `saturating_duration_since` вместо вычитания: между проверкой и
        // вычитанием поток может быть вытеснен, и `deadline - now()` на
        // отрицательном остатке паниковал бы прямо в потоке сессии.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return user_cancel.load(Ordering::Relaxed);
        }
        std::thread::sleep(Duration::from_millis(50).min(left));
    }
}

/// Итог ожидания стабилизации схемы.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StabilizationReport {
    /// Схема подтверждена и потолок частот держится.
    pub settled: bool,
    /// Сколько ждали, мс.
    pub waited_ms: u64,
    /// Разрешённый потолок частот, МГц (0 — данные недоступны).
    pub ceiling_mhz: u32,
    /// Причина, почему не дождались (пусто — дождались).
    pub reason: Option<SchemeStabilizeFailure>,
}

/// Почему стабилизация не завершилась штатно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemeStabilizeFailure {
    /// Схему так и не подтвердили как активную.
    SchemeUnconfirmed,
    /// Потолок частот всё менялся до потолка ожидания.
    CeilingNotStable,
}

/// Как часто опрашиваем потолок частот при стабилизации, мс.
pub const SCHEME_STABILIZE_POLL_MS: u64 = 250;
/// Сколько замеров подряд должны совпасть, чтобы считать потолок устоявшимся.
///
/// Два, а не один: одиночный замер совпадает и просто по совпадению, а два
/// подряд означают, что система перестала пересчитывать потолок.
pub const SCHEME_STABILIZE_STABLE_SAMPLES: u32 = 2;

/// Дождаться, пока схема применится и частота устоится.
///
/// Вместо фиксированных трёх секунд: ждём, пока `MaxMhz` (разрешённый
/// процессором потолок) перестанет меняться И активная схема подтверждена.
/// Так ожидание короче, когда Windows переключила план мгновенно, и длиннее,
/// когда плановая запись задержалась — ровно тот случай, который фиксированная
/// пауза пропускала.
///
/// Проверка `MaxMhz` вместо температуры осознанна: температура недоступна без
/// драйвера, а потолок частот — это и есть то, что меняет схема питания.
pub fn wait_scheme_stabilized(
    driver: &dyn SchemeDriver,
    expected_guid: &str,
    user_cancel: &AtomicBool,
) -> StabilizationReport {
    let started = Instant::now();
    let min_deadline = started + Duration::from_secs(SCHEME_STABILIZE_MIN_SECS);
    let max_deadline = started + Duration::from_secs(SCHEME_STABILIZE_MAX_SECS);

    let mut last_ceiling = 0u32;
    let mut stable = 0u32;
    let mut confirmed = false;
    let mut ceiling_known = false;

    loop {
        if user_cancel.load(Ordering::Relaxed) {
            return StabilizationReport {
                settled: false,
                waited_ms: started.elapsed().as_millis() as u64,
                ceiling_mhz: last_ceiling,
                reason: Some(SchemeStabilizeFailure::CeilingNotStable),
            };
        }
        if !confirmed {
            confirmed = verify_scheme_active(driver, expected_guid).is_ok();
        }
        let ceiling = powerbench_windows::power::power_state().max_mhz;
        if ceiling != 0 {
            if ceiling_known && ceiling == last_ceiling {
                stable += 1;
            } else {
                stable = 0;
                ceiling_known = true;
            }
            last_ceiling = ceiling;
        }
        let now = Instant::now();
        if confirmed && stable >= SCHEME_STABILIZE_STABLE_SAMPLES && now >= min_deadline {
            return StabilizationReport {
                settled: true,
                waited_ms: now.saturating_duration_since(started).as_millis() as u64,
                ceiling_mhz: last_ceiling,
                reason: None,
            };
        }
        if now >= max_deadline {
            return StabilizationReport {
                settled: false,
                waited_ms: now.saturating_duration_since(started).as_millis() as u64,
                ceiling_mhz: last_ceiling,
                reason: Some(if confirmed {
                    SchemeStabilizeFailure::CeilingNotStable
                } else {
                    SchemeStabilizeFailure::SchemeUnconfirmed
                }),
            };
        }
        if interruptible_sleep_ms(
            user_cancel,
            SCHEME_STABILIZE_POLL_MS.min(
                max_deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64,
            ),
        ) {
            return StabilizationReport {
                settled: false,
                waited_ms: started.elapsed().as_millis() as u64,
                ceiling_mhz: last_ceiling,
                reason: Some(SchemeStabilizeFailure::SchemeUnconfirmed),
            };
        }
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
    slow_note: &mut Option<SlowPhaseNote>,
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
        // Ориентир «низкий темп» — из истории машины, как и раньше: без него
        // медлительность просто не определяется. А вот ПОЛОМКА определяется по
        // собственной медиане фазы, которую детектор набирает сам. Так разделены
        // два случая, которые прежнее правило смешивало в один вердикт с
        // карантином: ровная медленная машина и машина, сломавшаяся на ходу.
        RateWatchdog::with_reference(baseline.as_ref().and_then(|b| b.p1_of(label))),
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
    // Классификация отмены: блокирующее ожидание вердикта нужно только при
    // отмене — тогда вердикт обязательно будет. Раньше `recv_timeout`
    // выполнялся при любой ошибке (включая ошибки движка, которых сторож не
    // присылает) — это гарантированная лишняя пауза в 5 секунд.
    //
    // При УСПЕШНОЙ фазе вердикт забираем без блокировки: сторож к этому
    // моменту либо уже отправил «ровно и медленно», либо молчит, и ждать
    // было бы лишней паузой в конце каждой фазы.
    let verdict = if matches!(result, Err(RunError::Cancelled)) {
        rx.recv_timeout(Duration::from_millis(WATCHDOG_CANCEL_GRACE_SECS * 1000))
            .ok()
    } else {
        rx.try_recv().ok()
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
        (
            Err(RunError::Cancelled),
            Some(WatchdogVerdict::Collapsed {
                ticks_per_sec,
                own_median,
            }),
        ) => Err(PhaseFailure::Collapsed {
            phase: label.to_string(),
            ticks_per_sec,
            own_median,
        }),
        (Err(RunError::Cancelled), _) => Err(PhaseFailure::UserCancelled),
        (Err(err), _) => Err(PhaseFailure::Engine(err)),
        (Ok(report), slow) => {
            // Фаза завершилась штатно. Если сторож успел сообщить о ровной
            // медлительности, замер ДОСТОВЕРЕН — он просто медленный, и
            // превращать это в ошибку значило бы выбросить хорошее измерение.
            // Пометка уходит наружу, прогон записывается как есть.
            if let Some(WatchdogVerdict::SlowButStable {
                ticks_per_sec,
                own_median,
            }) = slow
            {
                *slow_note = Some(SlowPhaseNote {
                    ticks_per_sec,
                    own_median,
                });
            }
            Ok((report, engine.samples().to_vec()))
        }
    }
}

/// Пометка «фаза отмерена, но машина шла ровно медленно».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlowPhaseNote {
    pub ticks_per_sec: u64,
    /// Медиана темпа этой же фазы.
    pub own_median: u64,
}

/// Ошибка отдельной фазы.
#[derive(Debug, Clone, PartialEq)]
enum PhaseFailure {
    Engine(RunError),
    Hung {
        phase: String,
    },
    /// Машина сломалась: темп упал относительно её собственной медианы.
    ///
    /// Замер недостоверен, и виновата МАШИНА, а не схема (регресс H40): так
    /// ведут себя термальный троттлинг, энергосбережение по температуре,
    /// посторонний процесс, уход ноутбука в сон. Поэтому прогон помечается
    /// невалидным, схема остаётся допущенной и перемеривается при следующем
    /// запуске. В карантин уходят только зависания самой схемы
    /// ([`PhaseFailure::Hung`]).
    Collapsed {
        phase: String,
        ticks_per_sec: u64,
        own_median: u64,
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
    /// Готовые итоги одной фазы: статистика, срез питания, номинальная
    /// длительность. Хранятся числами, а не сэмплами: времена тиков нужны
    /// только в момент расчёта статистики.
    phase_stats: Vec<PhaseStats>,
    combined: RunStats,
    cross_phase_consistency: f64,
    burst_retention_percent: f64,
    background: Vec<CorrelatedProcess>,
    spike_windows_total: usize,
    /// Полный дамп настроек схемы на момент прогона.
    scheme_dump: Option<String>,
    /// Снимок питания в конце прогона.
    power: PowerSnapshot,
    /// Фоновая нагрузка (сумма по процессам, % одного ядра) по секундам
    /// прогона: p50 и p95.
    background_cpu: (f64, f64, u32),
}

const fn zero_run_stats() -> RunStats {
    RunStats {
        samples: 0,
        samples_raw: 0,
        excluded_fraction: 0.0,
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
    //
    // Файл карантина читается мягко: при повреждении он считается пустым, и
    // запись в него запрещена (см. `quarantine_add`). Но молчать об этом
    // нельзя — пользователь увидел бы «все схемы разрешены» и не понял бы,
    // почему ранее заблокированная схема снова попала в замер. Поэтому
    // повреждение сообщается ОДИН раз за сессию.
    let quarantine = match crate::quarantine::load_quarantine_checked() {
        Ok(entries) => entries,
        Err(cause) => {
            note(
                &observer,
                &mut events,
                SessionEvent::Warn(format!(
                    "файл карантина не читается, список браковки пуст ({cause}). \
                     Карантин не применялся; сохраните файл для разбора и удалите, \
                     чтобы вернуть список"
                )),
            );
            Vec::new()
        }
    };
    let plan = {
        let (admitted, skipped) = preflight_filter(&plan.scheme_ids, &quarantine);
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
    let baseline = crate::history::machine_baseline(
        &crate::result::IdentityJson::from_signature_with_machine(
            &signature,
            &powerbench_windows::power::os_build(),
            &powerbench_windows::power::cpu_brand(),
            powerbench_windows::power::memory_gib(),
        ),
    );

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
            // Условия запуска тоже обязаны совпадать. Совпадение plan_guid
            // говорит лишь, что план тот же; число воркеров, привязка и
            // конфигурация нагрузки — часть измерения, и без их проверки
            // `resume` с другими параметрами перештамповывает старые прогоны
            // подписью текущего движка (регресс H41).
            if let Some(mismatch) = run_conditions_mismatch(&existing.runs, engine) {
                return Err(mismatch);
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
    //
    // Тот же контракт, что и у [`restore_original`]: возврат идёт через
    // [`restore_verified`], поэтому `original_restored = true` означает
    // подтверждённое ОС состояние. Раньше здесь стоял «голый» `set_active`, и
    // точка могла быть помечена восстановленной при плане, который так и не
    // вернулся.
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
                restore_verified(driver, &original)
                    .map_err(|cause| SessionError::RestoreScheme(format!("{original}: {cause}")))?;
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

    let SessionLoopOutcome {
        cancelled,
        early_stop_reason,
    } = match loop_result {
        Err(e) => {
            // Восстановление уже предпринято; возвращаем первичную ошибку.
            return Err(e);
        }
        Ok(outcome) => outcome,
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
        early_stop_reason,
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
) -> Result<SessionLoopOutcome, SessionError> {
    let durs = phase_durations(plan.duration_seconds);
    let mut cancelled = false;
    let mut early_stop_reason: Option<String> = None;
    let run_total = (plan.scheme_ids.len() as u32) * plan.repetitions;
    let mut run_index: u32 = 0;
    // Условия запуска фиксируются ОДИН раз на сессию и пишутся в каждый
    // прогон: движок не переинициализируется посреди цикла, и подпись,
    // которую увидит `resume`, должна совпадать с той, что была при замере.
    let run_conditions = current_run_conditions(engine);

    'outer: for round in 0..plan.repetitions {
        let order = round_order(&plan.scheme_ids, round);
        // Ключ строго `"{round}:{plan-guid}"` без имени/guid схемы: отметка
        // «раунд выполнен целиком». Внутри раунда выполненные схемы
        // распознаются по записям прогонов (см. `Checkpoint::has_run`).
        let key = run_key(round, &plan.plan_guid);
        // Копия ротации нужна после цикла: по ней проверяется, покрыт ли раунд
        // целиком, а сам `order` расходуется перебором.
        let round_order_schemes = order.clone();
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
            // --- Применение схемы С ПОДТВЕРЖДЕНИЕМ ---
            //
            // `setactive` проверяет только код возврата. Если план не
            // применился, замер пойдёт на чужой схеме — и это неотличимо от
            // нормального результата. Поэтому применяем через
            // `apply_and_verify`: без подтверждения от ОС замер не начинается.
            apply_and_verify(driver, &scheme_id).map_err(|cause| SessionError::ApplyScheme {
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

            // --- Стабилизация схемы: не фиксированная пауза, а ожидание
            //     устоявшегося потолка частот И подтверждённой активной схемы.
            let stabilization = wait_scheme_stabilized(driver, &scheme_id, user_cancel);
            if user_wants_stop(user_cancel) {
                cancelled = true;
                clear_testing_marker();
                break 'outer;
            }
            if !stabilization.settled {
                // Не отказ, а предупреждение: ждать дольше уже нельзя, а
                // Windows могла применить план с задержкой и без нас.
                note(
                    observer,
                    events,
                    SessionEvent::Warn(format!(
                        "схема «{scheme_id}» не подтверждена за {} с: {}; потолок частот {} МГц",
                        SCHEME_STABILIZE_MAX_SECS,
                        stabilization
                            .reason
                            .map(|r| match r {
                                SchemeStabilizeFailure::SchemeUnconfirmed =>
                                    "ОС не показывает её активной",
                                SchemeStabilizeFailure::CeilingNotStable =>
                                    "потолок частот всё меняется",
                            })
                            .unwrap_or("причина неизвестна"),
                        stabilization.ceiling_mhz
                    )),
                );
            }

            // --- Проверка фона: грязный фон отменяет замер, а не портит его ---
            //
            // Раньше превышение порога давало только строку в журнале, и
            // результат всё равно записывался: антивирус, съедающий 15 % CPU,
            // молча занижал скорость схемы, и та могла попасть в карантин по
            // «провалу фазы» с ложной причиной. Теперь слишком тяжёлый фон
            // откладывает прогон и после попыток пропускает его, а в карантин
            // не идёт — тяжёлый фон свойство машины, а не схемы.
            let threshold = plan.background_threshold_percent * engine.logical_cpus() as f64;
            let mut background_ok = false;
            // Последняя измеренная загрузка: без неё в сообщении о пропуске
            // нечего было бы назвать.
            let mut dirtiest = 0.0f64;
            for attempt in 1..=BACKGROUND_RUN_ATTEMPTS {
                let Some(verdict) = measure_background(
                    engine.logical_cpus(),
                    plan.background_threshold_percent,
                    user_cancel,
                ) else {
                    cancelled = true;
                    break 'outer;
                };
                match verdict {
                    BackgroundVerdict::Clean { measured } => {
                        note(
                            observer,
                            events,
                            SessionEvent::BackgroundClean {
                                measured_total_percent: measured,
                                threshold_percent: threshold,
                            },
                        );
                        background_ok = true;
                    }
                    BackgroundVerdict::Noisy { measured } => {
                        note(
                            observer,
                            events,
                            SessionEvent::BackgroundNoisy {
                                measured_total_percent: measured,
                                threshold_percent: threshold,
                            },
                        );
                        background_ok = true;
                    }
                    BackgroundVerdict::TooDirty { measured } => {
                        dirtiest = dirtiest.max(measured);
                        note(
                            observer,
                            events,
                            SessionEvent::BackgroundTooDirty {
                                measured_total_percent: measured,
                                threshold_percent: threshold * BACKGROUND_DIRTY_FACTOR,
                                attempt,
                                attempts: BACKGROUND_RUN_ATTEMPTS,
                            },
                        );
                        // Пользователь нажал «продолжить с риском»: гейт по фону
                        // для этой сессии выключен, и ждать второй попытки незачем —
                        // фон не станет чище сам. Иначе кнопка в интерфейсе была бы
                        // формой согласия, которая ничего не разрешает.
                        if plan.accept_dirty_background {
                            background_ok = true;
                            break;
                        }
                        if attempt < BACKGROUND_RUN_ATTEMPTS
                            && interruptible_sleep_ms(user_cancel, BACKGROUND_RETRY_PAUSE_MS)
                        {
                            cancelled = true;
                            break 'outer;
                        }
                    }
                }
                if background_ok {
                    break;
                }
            }
            if !background_ok {
                // Причина пропуска обязана попасть в отчёт. Раньше здесь был
                // голый `continue`, и прогон, не начавшийся из-за фоновой
                // нагрузки, не оставлял следов: в результате схема
                // показывалась с `runs: 0`, `rejected: false` и без единого
                // предупреждения — выглядело как поломка приложения, а не
                // как «машина была занята». Причина идёт в `rejections`
                // (из неё берутся `rejected` и `rejection_reason`), но НЕ в
                // карантин: тяжёлый фон — свойство машины, а не схемы.
                let critical = threshold * BACKGROUND_DIRTY_FACTOR;
                let reason = format!(
                    "фон {dirtiest:.1} % CPU при критическом пороге {critical:.0} % \
                     (порог из настроек — {:.1} % на ядро × {} ядер × {BACKGROUND_DIRTY_FACTOR}) — \
                     прогон не начался после {BACKGROUND_RUN_ATTEMPTS} попыток",
                    plan.background_threshold_percent,
                    engine.logical_cpus(),
                );
                note(
                    observer,
                    events,
                    SessionEvent::RunInvalid {
                        scheme_id: scheme_id.clone(),
                        reason: reason.clone(),
                    },
                );
                checkpoint.rejections.insert(scheme_id.clone(), reason);
                store.save(checkpoint).map_err(SessionError::Persist)?;
                clear_testing_marker();
                continue 'scheme;
            }

            // --- Питание: АКБ вместо сети во время сессии ---
            //
            // Проверка в начале сессии уже была, но она ничего не значит для
            // прогона, начавшегося через полчаса: выдернутый кабель не
            // прерывает замер, а молча занижает его на десятки процентов, и
            // виновата оказывается схема.
            //
            // Пропадание питания останавливает ВСЮ сессию, а не только прогон:
            // на батарее не пройдёт ни одна последующая схема, а перебор 90
            // схем с проверками фона только истёк бы временем.
            match driver.ac_power_online() {
                Ok(true) => {}
                Ok(false) => {
                    note(
                        observer,
                        events,
                        SessionEvent::RunInvalid {
                            scheme_id: scheme_id.clone(),
                            reason: "питание от сети пропало до начала замера (система на батарее)"
                                .to_string(),
                        },
                    );
                    cancelled = true;
                    clear_testing_marker();
                    break 'outer;
                }
                Err(e) => {
                    clear_testing_marker();
                    return Err(SessionError::Config(format!(
                        "не удалось определить питание от сети: {e}"
                    )));
                }
            }

            // --- Разогрев профилем «Отклик» (результаты отбрасываются) ---
            if !engine.reset() {
                clear_testing_marker();
                return Err(SessionError::LoadDidNotStop);
            }
            // Разогрев идёт фазой «Лёгкая», а не «Отклик». Фаза «Отклик» — это
            // короткий замер суперциклов, и на длинном разогреве она
            // переполняет буфер сэмплов и переписывает его много раз; кроме
            // того, она нагружает ровно одно ядро, то есть прогревает не то,
            // что затем измеряется. Растёт буфер по той же оценке сэмплов,
            // что и раньше, поэтому объём памяти не изменился.
            let warmup_phase = Phase::Light;
            engine.prepare_sample_buffer(warmup_phase, plan.warmup_seconds.max(1));
            let warmup_cancel = engine.canceller();
            let warmup_scoreboard = engine.scoreboard_arc();
            let warmup_finished = Arc::new(AtomicBool::new(false));
            let (warmup_handle, warmup_rx) = spawn_watchdog(
                warmup_scoreboard,
                warmup_cancel,
                Arc::clone(user_cancel),
                // Разогрев — не измерение: обрывать его по темпу нельзя, иначе
                // плохая схема «успела бы» не начать замер вовсе.
                RateWatchdog::default(),
                Arc::clone(&warmup_finished),
            );
            let warmup_result = engine.run_phase(
                warmup_phase,
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
            // Дамп настроек схемы снимается здесь: прогрев уже закончился, но ни
            // одна измеряемая фаза ещё не началась, поэтому в дампе ровно то,
            // что сейчас активно и что сейчас же будет измерено. Снимок
            // делается один раз на прогон, а не на фазу.
            let scheme_dump = capture_scheme_dump(&scheme_id, observer, events);
            // Статистика каждой фазы считается сразу же после её измерения,
            // а сами `Vec<f64>` с временами тиков выбрасываются. Раньше они
            // жили до конца прогона в четырёх копиях (`times_by_phase`,
            // `all_times`, `group_refs`, `RunData.phase_times`), и на длинной
            // сессии четыре полных массива сэмплов держались в памяти ради
            // чисел, которые уже посчитаны.
            let mut phase_stats: Vec<PhaseStats> = Vec::new();
            // Единственный оставшийся массив сэмплов: объединение фаз для
            // `combined`. Процентили объединённого набора не выводятся из
            // пофазных статистик, поэтому сами времена тиков нужны.
            let mut all_times: Vec<f64> = Vec::new();
            // Среднее удержание темпа: средние Light/Heavy и вклад фаз в
            // ConsistencyPercent накапливаются числами, без хранения сэмплов.
            let mut light_avg = 0.0f64;
            let mut heavy_avg = 0.0f64;
            let mut cross_weighted = 0.0f64;
            let mut cross_weight = 0.0f64;
            let mut spike_count_total = 0usize;
            let mut all_spike_windows: Vec<SpikeWindow> = Vec::new();
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
                // --- Граница фазы: та ли схема, и то ли питание ---
                //
                // Здесь ловится подмена плана посторонним процессом, OEM-утилитой
                // или обновлением Windows, и пропадание питания от сети. Обе
                // проверки обязаны быть на границе, а не в конце: чтобы фаза, в
                // которой событие уже произошло, вообще не начиналась.
                if let Err(why) = verify_scheme_active(driver, &scheme_id) {
                    note(
                        observer,
                        events,
                        SessionEvent::RunInvalid {
                            scheme_id: scheme_id.clone(),
                            reason: why,
                        },
                    );
                    clear_testing_marker();
                    break 'scheme;
                }
                match driver.ac_power_online() {
                    Ok(true) => {}
                    Ok(false) => {
                        note(
                            observer,
                            events,
                            SessionEvent::RunInvalid {
                                scheme_id: scheme_id.clone(),
                                reason: format!(
                                    "питание от сети пропало перед фазой «{}»",
                                    phase_label(phase)
                                ),
                            },
                        );
                        // Питание пропало посреди сессии: дальше ни одна схема
                        // не пройдёт, поэтому останавливаем сессию целиком.
                        cancelled = true;
                        clear_testing_marker();
                        break 'outer;
                    }
                    Err(e) => {
                        clear_testing_marker();
                        return Err(SessionError::Config(format!(
                            "не удалось определить питание от сети: {e}"
                        )));
                    }
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
                let mut slow_note: Option<SlowPhaseNote> = None;
                let phase_result = run_measured_phase(
                    engine,
                    Arc::clone(user_cancel),
                    phase,
                    secs,
                    label,
                    &monitor_map,
                    observer.clone(),
                    &mut slow_note,
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
                        // Машина шла РОВНО и медленно: темп держался у своей
                        // медианы, то есть не проседал. Замер достоверен — он
                        // просто низкий, поэтому он учтён, а не отброшен.
                        // Ни отбраковки, ни карантина: заведомо консервативная
                        // схема («max processor state» = 50 %) законный объект
                        // измерения, и её нельзя наказать за то, что она
                        // ограничивает частоты намеренно.
                        if let Some(note_slow) = slow_note {
                            note(
                                observer,
                                events,
                                SessionEvent::Warn(format!(
                                    "фаза «{label}» отмерена на низком, но ровном темпе: \
                                     {} тик/с при медиане {} фазы. Замер достоверен и учтён; \
                                     если низкий темп не задуман, проверьте ограничения схемы",
                                    note_slow.ticks_per_sec, note_slow.own_median
                                )),
                            );
                        }
                        ticks += report.ticks;
                        if phase == Phase::Response {
                            supercycles = report.supercycles_completed;
                        }
                        first_tick_checksums[idx] = report.first_tick_checksum;
                        run_checksums[idx] = report.run_checksum;
                        // Срез питания в конце фазы: троттлинг может начаться
                        // и кончиться внутри прогона, и по одному срезу в
                        // конце всего прогона это не поймать.
                        let phase_snapshot = PowerSnapshot::capture();
                        // Питание могло пропасть ВНУТРИ фазы — тогда её данные
                        // недостоверны целиком, и продолжать прогон незачем:
                        // все следующие фазы тоже пойдут на батарее. Снимок
                        // уже содержит `on_ac`, поэтому лишнего вызова API не
                        // нужно.
                        if !phase_snapshot.on_ac {
                            let reason = format!("питание от сети пропало во время фазы «{label}»");
                            note(
                                observer,
                                events,
                                SessionEvent::RunInvalid {
                                    scheme_id: scheme_id.clone(),
                                    reason,
                                },
                            );
                            // Фаза недостоверна целиком, а следующие на батарее
                            // тоже не пройдут — останавливаем сессию.
                            cancelled = true;
                            clear_testing_marker();
                            break 'outer;
                        }
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
                        let window_count = windows.len();
                        spike_count_total += window_count;
                        all_spike_windows.extend(windows);
                        // Всё, что нужно от сэмплов фазы, считается здесь же,
                        // после чего `times` освобождается вместе с `windows`.
                        let stats = matched_stats(&times);
                        if phase == Phase::Light {
                            light_avg = stats.average_throughput;
                        }
                        // Индекс берём у самой фазы, а не литералом: после
                        // появления фазы «Частичная» литерал 1 перестал быть
                        // «Тяжёлой», и метрика удержания темпа молча считалась
                        // по чужой фазе.
                        if phase == Phase::Heavy {
                            heavy_avg = stats.average_throughput;
                        }
                        let (score, n) = consistency_group(&times);
                        cross_weighted += n * score;
                        cross_weight += n;
                        all_times.extend_from_slice(&times);
                        phase_stats.push(PhaseStats {
                            phase_index: phase.index(),
                            stats,
                            power: Some(phase_snapshot),
                            seconds: secs,
                        });
                        // Стабилизационная пауза после каждой фазы —
                        // прерываемая, иначе кнопка «Стоп» ждала бы её целиком.
                        if interruptible_sleep(user_cancel, STABILIZATION_SECS) {
                            cancelled = true;
                            clear_testing_marker();
                            break 'outer;
                        }
                        if spike_count_total > 0 {
                            note(
                                observer,
                                events,
                                SessionEvent::SpikeWindows {
                                    label: label.to_string(),
                                    count: window_count,
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
                        own_median,
                    }) => {
                        // Машина сломалась ПО ХОДУ фазы: темп упал относительно
                        // того, что она же показывала минуту назад.
                        //
                        // Регресс H40: здесь раньше схема попадала в
                        // ПЕРМАНЕНТНЫЙ карантин как `Degraded`. Но падение темпа
                        // посреди фазы — это не свойство схемы: так ведёт себя
                        // термальный троттлинг, уход в энергосбережение по
                        // температуре, посторонний процесс, уход ноутбука в сон.
                        // Пользователю после такого оставался заблокированный
                        // здоровый план питания — вернуть его можно было только
                        // руками через карантин в интерфейсе, и причина
                        // блокировки («деградация схемы») была ложной.
                        //
                        // Теперь прогон помечается невалидным и схема
                        // остаётся допущенной: раунд не закрывается (см.
                        // `round_is_covered`), и следующий запуск перемерит её
                        // заново. В карантин уходят только зависания самой
                        // схемы (`Hung` → `NoProgress`) и статистические
                        // правила пост-сессии.
                        let reason = format!(
                            "темп упал до {ticks_per_sec} тик/с против медианы фазы \
                             {own_median} (фаза «{phase}») — замер недостоверен, причина \
                             на стороне машины (охлаждение, троттлинг, посторонняя нагрузка)"
                        );
                        note(
                            observer,
                            events,
                            SessionEvent::RunInvalid {
                                scheme_id: scheme_id.clone(),
                                reason: reason.clone(),
                            },
                        );
                        clear_testing_marker();
                        // Зависла одна фаза — остальные фазы и схемы идут дальше.
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

            let burst = burst_retention_percent(heavy_avg, light_avg);

            // Объединённая статистика прогона. `all_times` — единственный
            // массив сэмплов, переживающий фазы, и он нужен только здесь.
            let combined = run_stats(&all_times).unwrap_or_else(zero_run_stats);
            drop(all_times);
            let cross = if cross_weight == 0.0 {
                0.0
            } else {
                cross_weighted / cross_weight
            };

            // Регресс H3: прогон без единого валидного сэмпла — это НЕ измерение.
            //
            // Раньше `run_stats` возвращал `None`, и подставлялся `RunStats` из
            // нулей. Такой прогон попадал в чекпоинт как обычный: нули тянули
            // среднее и σ всей схемы к нулю, а сама схема ранжировалась наравне
            // с честно измеренными — и выглядела при этом «очень нестабильной».
            // Именно по нулевому среднему `admitted()` и отсекает кандидата, но
            // не из-за отсутствия данных, а как будто они есть и равны нулю.
            if combined.samples == 0 {
                note(
                    observer,
                    events,
                    SessionEvent::RunInvalid {
                        scheme_id: scheme_id.clone(),
                        reason: format!(
                            "прогон не дал ни одного валидного сэмпла тика (тиков всего \
                             {ticks}) — измерение не засчитано"
                        ),
                    },
                );
                clear_testing_marker();
                continue 'scheme;
            }

            // Фоновые корреляции по окнам скачков этого прогона.
            //
            // Отравление мьютекса не должно ронять сессию (регресс H38): раньше
            // здесь стоял `lock().unwrap()`, и паника в потоке мониторинга или в
            // любом другом владельце карты завершала бы весь прогон паникой вместо
            // того, чтобы просто отдать неполные фоновые данные.
            let map_snapshot = monitor_map
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
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
                phase_stats: phase_stats.clone(),
                combined,
                cross_phase_consistency: cross,
                burst_retention_percent: burst,
                background,
                spike_windows_total: spike_count_total,
                scheme_dump,
                power: PowerSnapshot::capture(),
                background_cpu,
            });

if let Some(run) = run_outcome {
                let stored = build_stored_run(&run, plan, name_map, &run_conditions);
                checkpoint.runs.push(drop_duplicate_scheme_dump(checkpoint, stored));
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
        // Раунд отработан целиком (у каждой схемы есть запись прогона либо
        // браковка): отмечаем ключ раунда, чтобы повторный запуск пропустил
        // ротацию одним махом.
        //
        // Проверка обязательна. Раньше ключ попадал в `completed_keys`
        // безусловно — сразу после выхода из цикла по схемам, даже если
        // половина схем ушла через `break 'scheme` (план сменили извне, машина
        // не загрузилась, не нашлось ни одного валидного сэмпла). При
        // `resume` ключ означал «раунд доделан», и непокрытые схемы
        // перетестировались уже никогда: сессия выглядела завершённой, а данные
        // о схемах в ней не было.
        let round_covered = round_is_covered(checkpoint, round, &round_order_schemes);
        if round_covered {
            checkpoint.completed_keys.push(key.clone());
            store.save(checkpoint).map_err(SessionError::Persist)?;
        } else {
            let missing: Vec<&str> = round_order_schemes
                .iter()
                .filter(|id| {
                    !checkpoint.has_run(round, id) && !checkpoint.is_rejected(id)
                })
                .map(String::as_str)
                .collect();
            note(
                observer,
                events,
                SessionEvent::Warn(format!(
                    "раунд {round} отмечен незавершённым: без записи остались схемы \
                     [{}]. Их можно перемерить: powerbench-cli resume",
                    missing.join(", ")
                )),
            );
        }

        // --- Адаптивная ранняя остановка ---
        //
        // Проверяется после КАЖДОГО целого раунда. Схемы, признанные
        // забракованными, из статистики исключены: их средние не отвечают ни за
        // что, а решение о лидерстве принимается по тем, что честно отработали
        // своё число раундов.
        //
        // Незавершённый раунд исключает остановку: решение принимается по
        // статистике, а принимать его нельзя, пока часть плана не измерена.
        // Иначе сессия завершалась бы досрочно, оставив схему вовсе без данных.
        if !round_covered {
            continue;
        }
        // Число завершённых раундов — по ключам в точке, а не по номеру
        // итерации. Раньше здесь стояло `done_rounds.max(round + 1)`, то есть
        // незавершённый раунд всё равно засчитывался: «осталось N раундов» в
        // сообщении о решении было неверным, и при `resume` могло выглядеть так,
        // будто часть плана уже отработана.
        let done_rounds = completed_rounds(checkpoint, round, &plan.plan_guid);
        let inputs = crate::early_stop::leader_inputs(&checkpoint.runs, &checkpoint.rejections);
        let decision = crate::early_stop::early_stop_decision(
            &inputs,
            done_rounds,
            plan.repetitions,
        );
        if decision.stop {
            early_stop_reason = Some(decision.reason.clone());
            note(
                observer,
                events,
                SessionEvent::EarlyStopped {
                    reason: decision.reason,
                },
            );
            break 'outer;
        }
    }

    Ok(SessionLoopOutcome {
        cancelled,
        early_stop_reason,
    })
}

/// Условия запуска, под которыми работает текущий движок.
fn current_run_conditions(engine: &Engine) -> LaunchConditions {
    LaunchConditions {
        worker_count: engine.worker_count(),
        affinity_mode: engine.affinity_mode().as_str().to_string(),
        affinity_signature: engine.affinity_signature(),
        config_hash: engine.config_hash().to_string(),
    }
}

/// Первый прогон точки, сделанный при других условиях, чем у текущего движка.
///
/// Точки, записанные прошлыми версиями, условий не содержат: они пропускаются
/// (`has_launch_conditions`), иначе любое чтение старой точки ломало бы работу.
fn run_conditions_mismatch(
    runs: &[StoredRun],
    engine: &Engine,
) -> Option<SessionError> {
    let now = current_run_conditions(engine);
    for run in runs {
        if !run.has_launch_conditions() {
            continue;
        }
        let recorded = run.launch_conditions();
        if let Some(difference) = recorded.first_difference(&now) {
            return Some(SessionError::RunConditionsMismatch {
                difference,
                recorded: recorded.describe(),
            });
        }
    }
    None
}

/// Сколько раундов плана до `up_to` включительно реально завершено.
///
/// Считается по ключам в контрольной точке, а не по номеру итерации: раунд,
/// в котором не покрыты все схемы, ключа не получает (см.
/// [`round_is_covered`]) и потому завершённым не считается.
///
/// Забытый `max(round + 1)` означал ровно обратное: незавершённый раунд
/// засчитывался, и в сообщении о досрочной остановке показывалось неверное
/// «осталось N раундов».
fn completed_rounds(checkpoint: &Checkpoint, up_to: u32, plan_guid: &str) -> u32 {
    (0..=up_to)
        .filter(|r| checkpoint.is_completed(&run_key(*r, plan_guid)))
        .count() as u32
}

/// Покрыт ли раунд целиком: у каждой его схемы есть запись прогона либо
/// браковка.
///
/// Пропущенная по любой причине схема (план сменили извне, машина не
/// загрузилась, фоновой нагрузки слишком много, не нашлось валидных сэмплов)
/// делает раунд незавершённым, и ключ раунда в `completed_keys` ставиться не
/// должен: при `resume` иначе непокрытые схемы считались бы уже измеренными и
/// никогда бы не перетестировались.
fn round_is_covered(checkpoint: &Checkpoint, round: u32, order: &[String]) -> bool {
    order
        .iter()
        .all(|id| checkpoint.has_run(round, id) || checkpoint.is_rejected(id))
}

/// Итог цикла раундов.
#[derive(Debug, Clone, Default, PartialEq)]
struct SessionLoopOutcome {
    /// Сессия прервана пользователем (или недостижимым условием среды).
    cancelled: bool,
    /// Причина досрочной остановки, если она была.
    early_stop_reason: Option<String>,
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
            // Исходную схему так и не запомнили (powercfg не отдал активную
            // при старте). Схему вернуть нечем, но сказать об этом надо:
            // молча выйти значит оставить машину на тестовом плане.
            eprintln!(
                "PowerBench: исходная схема питания неизвестна — вернуть её нечем, \
                 на машине остался план последнего замера"
            );
            return;
        };
        // С подтверждением от ОС, а не «команда ушла»: после паники никто
        // больше не проверит состояние, и неподтверждённый возврат здесь —
        // молчаливая потеря пользовательского плана.
        if let Err(e) = restore_verified(self.driver, &original) {
            // Паника при разворачивании паники = аварийный останов процесса,
            // поэтому ограничиваемся сообщением: восстановить схему не удалось,
            // и пользователю нужно сказать об этом прямо.
            eprintln!("PowerBench: {e}");
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
    let Some(original) = checkpoint.original_scheme_guid.clone() else {
        // Исходную схему так и не запомнили (powercfg не отдал активную).
        // Помечать «восстановлено» нельзя: подтверждения от ОС нет, а сам
        // `Ok(())` означал бы «всё в порядке» — и точка осталась бы навсегда
        // в состоянии «не восстановлено», и следующий запуск думал бы, что
        // есть что чинить, хотя чинить нечем.
        //
        // Возвращаем Err: тестовая схема могла остаться активной, и сказать
        // об этом пользователю важнее, чем отчитаться об успехе. Поверх
        // ошибки виден результат сессии — она уже записана в чекпоинт и
        // разбирается из него.
        note(
            observer,
            events,
            SessionEvent::Warn(
                "исходная схема питания неизвестна — вернуть её нечем, на машине \
                 остался план последнего замера"
                    .to_string(),
            ),
        );
        return Err(SessionError::RestoreScheme(
            "исходная схема питания не была зафиксирована (powercfg не отдал активную \
             до первого прогона), вернуть её нечем; на машине остался план последнего \
             замера — верните его вручную через powercfg"
                .to_string(),
        ));
    };
    // Возврат только с подтверждением от ОС: `set_active` сам по себе означает
    // лишь «команда ушла». Флаг `original_restored` взводится ниже только
    // после успеха, то есть после `active_scheme()` с нужным GUID.
    restore_verified(driver, &original)
        .map_err(|cause| SessionError::RestoreScheme(format!("{original}: {cause}")))?;
    note(
        observer,
        events,
        SessionEvent::Restored {
            scheme_id: original.clone(),
            label: "после теста".to_string(),
        },
    );
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
///
/// Условия запуска (число воркеров, привязка, конфигурация нагрузки)
/// Убрать повторный дамп настроек схемы (регресс M35).
///
/// Чекпоинт переписывается целиком после каждого прогона. Дамп
/// `powercfg /query <guid>` писался в каждый прогон, поэтому при R повторах и
/// S схемах в файле оказывалось R×S копий одного и того же текста об одних и
/// те�� же настройках — и они переписывались после каждого прогона. На типовой
/// сессии это мегабайты, а данных там на единицы килобайт.
///
/// Оставляем дамп у ПЕРВОГО прогона схемы: настройки щита между повторами не
/// меняются (их меняют между сессиями), так что копии были побайтово
/// одинаковыми, а воспроизводимость результата не пострадала — dump по GUID и
/// раньше требовался ровно один.
///
/// Сравнение GUID регистронезависимо: `powercfg` отдаёт идентификаторы в
/// верхнем регистре, а чекпоинт мог сохранить их в нижнем.
fn drop_duplicate_scheme_dump(checkpoint: &Checkpoint, mut stored: StoredRun) -> StoredRun {
    if stored.scheme_dump.is_some()
        && checkpoint.runs.iter().any(|r| {
            r.scheme_id.eq_ignore_ascii_case(&stored.scheme_id) && r.scheme_dump.is_some()
        })
    {
        stored.scheme_dump = None;
    }
    stored
}

/// записи в каждый прогон: без них `resume` не может отличить свои
/// прогоны от чужих и перештампует их подписью текущего движка.
fn build_stored_run(
    run: &RunData,
    plan: &SessionConfig,
    name_map: &BTreeMap<String, String>,
    conditions: &LaunchConditions,
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
        phases: run.phase_stats.clone(),
        combined: run.combined,
        cross_phase_consistency: run.cross_phase_consistency,
        burst_retention_percent: run.burst_retention_percent,
        background: run.background.clone(),
        spike_windows: run.spike_windows_total,
        power: Some(run.power),
        scheme_dump: run.scheme_dump.clone(),
        background_cpu_p50: run.background_cpu.0,
        background_cpu_p95: run.background_cpu.1,
        background_sample_seconds: run.background_cpu.2,
        worker_count: conditions.worker_count,
        affinity_mode: conditions.affinity_mode.clone(),
        affinity_signature: conditions.affinity_signature.clone(),
        config_hash: conditions.config_hash.clone(),
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
    #[derive(Default)]
    struct RecordingDriver {
        active: Mutex<String>,
        calls: Mutex<Vec<String>>,
        /// Сколько раз `active_scheme` должно сообщать «старую» схему, прежде
        /// чем сообщить новую (эмуляция асинхронной записи в Power Manager).
        stale_reads: Mutex<usize>,
        /// Ответ `active_scheme`: `Err` эмулирует «состояние не читается».
        active_read_fails: Mutex<bool>,
        /// `set_active` возвращает успех, но НЕ переключает схему — так ведёт
        /// себя `powercfg` под доменной политикой.
        lie_on_set: Mutex<bool>,
        /// Сколько следующих вызовов `set_active` провалятся (не считая
        /// `lie_on_set`): эмуляция занятого Power Manager.
        set_failures: Mutex<usize>,
        /// Сколько раз звали `set_active` — по этому счётчику видно, что
        /// восстановление не поверило одному коду возврата.
        set_calls: Mutex<u32>,
    }

    impl RecordingDriver {
        fn new(initial: &str) -> Self {
            Self {
                active: Mutex::new(initial.to_string()),
                calls: Mutex::new(Vec::new()),
                stale_reads: Mutex::new(0),
                active_read_fails: Mutex::new(false),
                lie_on_set: Mutex::new(false),
                set_failures: Mutex::new(0),
                set_calls: Mutex::new(0),
            }
        }

        /// Драйвер, у которого `set_active` возвращает успех, но схема НЕ
        /// меняется — ровно поведение powercfg при конфликте политики.
        fn lying(initial: &str) -> Self {
            let d = Self::new(initial);
            *d.lie_on_set.lock().unwrap() = true;
            d
        }

        /// Драйвер, который первые `times` вызовов `set_active` провалит, а
        /// дальше работает нормально.
        fn failing_first(initial: &str, times: usize) -> Self {
            let d = Self::new(initial);
            *d.set_failures.lock().unwrap() = times;
            d
        }

        fn set_calls(&self) -> u32 {
            *self.set_calls.lock().unwrap()
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
            *self.set_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            let mut failures = self.set_failures.lock().unwrap_or_else(|e| e.into_inner());
            if *failures > 0 {
                *failures -= 1;
                return Err("мост питания занят".to_string());
            }
            if !*self.lie_on_set.lock().unwrap_or_else(|e| e.into_inner()) {
                *self.active.lock().unwrap_or_else(|e| e.into_inner()) = guid.to_string();
                self.calls
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(guid.to_string());
            }
            Ok(())
        }

        fn active_scheme(&self) -> Result<String, String> {
            if *self
                .active_read_fails
                .lock()
                .unwrap_or_else(|e| e.into_inner())
            {
                return Err("мост питания недоступен".to_string());
            }
            let mut stale = self.stale_reads.lock().unwrap_or_else(|e| e.into_inner());
            if *stale > 0 {
                *stale -= 1;
                return Ok("old-guid".to_string());
            }
            Ok(self
                .active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone())
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
    fn rate_watchdog_ignores_slow_start() {
        let mut d = RateWatchdog::new();
        // Первые секунды фазы — низкий темп из-за разгона, обрыва быть не должно,
        // сколько бы секунд мы ни наблюдали.
        for t in 0..COLLAPSE_ARM_SECS {
            assert_eq!(
                d.observe(5, Duration::from_secs(t)),
                RateVerdict::Ok,
                "разгон на {t}-й секунде не должен считаться поломкой"
            );
        }
    }

    /// Без истории машины ПОЛОМКА всё равно определяется — по собственной медиане
    /// фазы. Прежде этого требовался ориентир из истории, и на чистой машине
    /// зависание по темпу не ловилось вовсе.
    #[test]
    fn rate_watchdog_detects_collapse_without_machine_history() {
        let mut d = RateWatchdog::default();
        let fill = COLLAPSE_ARM_SECS + RATE_SAMPLES_FOR_MEDIAN as u64;
        for t in COLLAPSE_ARM_SECS..fill {
            assert_eq!(d.observe(5000, Duration::from_secs(t)), RateVerdict::Ok);
        }
        let mut fired = false;
        for t in fill..=fill + 10 {
            if d.observe(100, Duration::from_secs(t)) != RateVerdict::Ok {
                fired = true;
                break;
            }
        }
        assert!(
            fired,
            "падение в 50 раз относительно собственной медианы обязано ловиться \
         без истории машины"
        );
    }

    /// Заведомо консервативная схема: машина идёт ровно и медленно, но ВЫШЕ
    /// половины собственной медианы.
    ///
    /// Прежнее правило сравнивало темп с абсолютной величиной из истории, и такая
    /// схема получала тот же вердикт, что и сломанная: и то и другое уходило в
    /// перманентный карантин с причиной «схема не тянет». Теперь это
    /// `SlowButStable` — медленно, но верно, карантина нет.
    #[test]
    fn deliberately_slow_scheme_is_reported_not_rejected() {
        // История машины: она раньше показывала 1000 тик/с. Ориентир «низкий» = 200.
        let mut d = RateWatchdog::with_reference(Some(1000.0));
        let fill = COLLAPSE_ARM_SECS + RATE_SAMPLES_FOR_MEDIAN as u64;
        for t in COLLAPSE_ARM_SECS..fill {
            assert_eq!(d.observe(100, Duration::from_secs(t)), RateVerdict::Ok);
        }
        assert_eq!(d.median_rate(), 100);
        // Темп ровно 100 — это 100 % собственной медианы, то есть НЕ поломка.
        let mut saw_slow = None;
        let mut collapsed = false;
        for t in fill..=fill + 30 {
            match d.observe(100, Duration::from_secs(t)) {
                RateVerdict::Ok => {}
                RateVerdict::SlowButStable { ticks_per_sec, .. } => saw_slow = Some(ticks_per_sec),
                RateVerdict::Collapsed { .. } => {
                    collapsed = true;
                    break;
                }
            }
        }
        assert!(
            !collapsed,
            "ровный темп на собственной медиане не может считаться поломкой"
        );
        assert_eq!(
            saw_slow,
            Some(100),
            "медленная ровная машина обязана быть помечена как таковая"
        );
    }

    /// Настоящая поломка: темп ПРОСАЛ относительно того, что машина только что
    /// показывала. Это единственный случай, когда замер недостоверен.
    #[test]
    fn rate_watchdog_detects_a_real_collapse() {
        let mut d = RateWatchdog::new();
        for t in COLLAPSE_ARM_SECS..COLLAPSE_ARM_SECS + RATE_SAMPLES_FOR_MEDIAN as u64 {
            assert_eq!(d.observe(5000, Duration::from_secs(t)), RateVerdict::Ok);
        }
        assert_eq!(d.median_rate(), 5000);
        // Темп упал вчетверо и держится так.
        let mut fired = None;
        for t in (COLLAPSE_ARM_SECS + RATE_SAMPLES_FOR_MEDIAN as u64)..=40 {
            if d.observe(1000, Duration::from_secs(t)) != RateVerdict::Ok {
                fired = Some(t);
                break;
            }
        }
        assert!(
            fired.is_some(),
            "падение вчетверо относительно собственной медианы обязано быть замечено"
        );
    }

    /// Обрыв наступает не мгновенно: ровно через COLLAPSE_HOLD_SECS устойчивого
    /// низкого темпа после разгона.
    #[test]
    fn rate_watchdog_requires_a_sustained_drop() {
        let mut d = RateWatchdog::new();
        let fill = COLLAPSE_ARM_SECS + RATE_SAMPLES_FOR_MEDIAN as u64;
        for t in COLLAPSE_ARM_SECS..fill {
            assert_eq!(d.observe(4000, Duration::from_secs(t)), RateVerdict::Ok);
        }
        let mut fired_at = None;
        for t in fill..=fill + 10 {
            if d.observe(100, Duration::from_secs(t)) != RateVerdict::Ok {
                fired_at = Some(t);
                break;
            }
        }
        assert_eq!(
            fired_at,
            Some(fill + COLLAPSE_HOLD_SECS),
            "обрыв должен наступить через {COLLAPSE_HOLD_SECS} с устойчивого провала"
        );
    }

    /// Единичный просад не обрывает замер: темп считается по одному батчу и
    /// сам скачет.
    #[test]
    fn rate_watchdog_survives_single_dip() {
        let mut d = RateWatchdog::new();
        let fill = COLLAPSE_ARM_SECS + RATE_SAMPLES_FOR_MEDIAN as u64;
        for t in COLLAPSE_ARM_SECS..fill {
            assert_eq!(d.observe(5000, Duration::from_secs(t)), RateVerdict::Ok);
        }
        assert_eq!(d.observe(10, Duration::from_secs(fill)), RateVerdict::Ok);
        // Темп вернулся — отсчёт сброшен, до упора держимся.
        for t in fill..fill + 5 {
            assert_eq!(d.observe(5000, Duration::from_secs(t)), RateVerdict::Ok);
        }
    }

    /// Здоровая машина с темпом около своей медианы не должна вызывать НИ ОДНОГО
    /// решения — ни поломки, ни медлительности.
    #[test]
    fn rate_watchdog_stays_quiet_on_a_healthy_machine() {
        let mut d = RateWatchdog::new();
        for t in COLLAPSE_ARM_SECS..600 {
            let v = d.observe(3000, Duration::from_secs(t));
            assert_eq!(
                v,
                RateVerdict::Ok,
                "на здоровой машине решений быть не должно: {v:?}"
            );
        }
    }

    /// Медлительность сообщается ОДИН раз на фазу: иначе в журнал уходила бы
    /// простыня одинаковых строк.
    #[test]
    fn slow_but_stable_is_reported_once() {
        // С ориентиром из истории: машина раньше показывала 1000, порог 200.
        let mut d = RateWatchdog::with_reference(Some(1000.0));
        let fill = COLLAPSE_ARM_SECS + RATE_SAMPLES_FOR_MEDIAN as u64;
        for t in COLLAPSE_ARM_SECS..fill {
            assert_eq!(d.observe(100, Duration::from_secs(t)), RateVerdict::Ok);
        }
        let mut reports = 0;
        for t in fill..=fill + 30 {
            if matches!(
                d.observe(100, Duration::from_secs(t)),
                RateVerdict::SlowButStable { .. }
            ) {
                reports += 1;
            }
        }
        assert_eq!(
            reports, 1,
            "медлительность обязана сообщаться ровно один раз"
        );
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
            accept_dirty_background: false,
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
        let mut slow_note: Option<SlowPhaseNote> = None;
        let result = run_measured_phase(
            &mut engine,
            Arc::new(AtomicBool::new(false)),
            Phase::Heavy,
            1,
            "фаза-тест",
            &monitor_map,
            obs,
            &mut slow_note,
            None,
        );
        assert!(result.is_ok());
        assert!(
            slow_note.is_none(),
            "здоровая машина не должна давать пометку о медлительности"
        );
        let got = recorded.lock().unwrap().len();
        assert!(got >= 1, "ожидались тики телеметрии, получено {got}");
        assert!(!engine.progress_snapshot().running);
    }

    // ------------------------------------------------------------------
    // Фаза 2: применение схемы только при подтверждении от ОС
    // ------------------------------------------------------------------

    /// Штатное переключение подтверждается с первого чтения.
    #[test]
    fn scheme_application_is_confirmed() {
        let driver = RecordingDriver::new("old-guid");
        assert_eq!(
            apply_and_verify(&driver, "new-guid"),
            Ok(()),
            "корректное переключение обязано подтверждаться"
        );
        assert_eq!(driver.active_scheme().unwrap(), "new-guid");
    }

    /// Ключевой случай: `powercfg /setactive` вернул код 0, но план не
    /// применился. Без проверки замер пошёл бы на чужой схеме, и в отчёте это
    /// не отличилось бы от нормального результата.
    #[test]
    fn a_scheme_that_did_not_apply_is_an_error_not_a_measurement() {
        let driver = RecordingDriver::lying("old-guid");
        let err = apply_and_verify(&driver, "new-guid").expect_err("ложное применение");
        let text = err.to_lowercase();
        assert!(
            text.contains("не переключила схему"),
            "ошибка обязана называть суть, а не код возврата: {err}"
        );
        assert!(
            err.contains("old-guid"),
            "в ошибке должно быть видно активную схему: {err}"
        );
    }

    /// Windows применяет план асинхронно: сразу после команды ещё может быть
    /// старая схема. Одиночный poll попадал в эту дыру и давал ложный отказ.
    #[test]
    fn late_scheme_write_is_not_mistaken_for_a_failure() {
        let driver = RecordingDriver::new("old-guid");
        // Два чтения подряд показывают старую схему, третье — новую.
        *driver.stale_reads.lock().unwrap() = 2;
        assert_eq!(
            apply_and_verify(&driver, "new-guid"),
            Ok(()),
            "задержка применения не должна считаться отказом"
        );
    }

    /// Нечитаемое состояние — не «применилось». Проверка не может молча
    /// пропустить фазу, потому что не смогла прочитать схему.
    #[test]
    fn unreadable_scheme_state_is_not_treated_as_applied() {
        let driver = RecordingDriver::new("old-guid");
        *driver.active_read_fails.lock().unwrap() = true;
        let err = apply_and_verify(&driver, "new-guid")
            .expect_err("непрочитанное состояние обязано быть отказом");
        assert!(err.contains("мост питания недоступен"), "{err}");
    }

    /// Подмена схемы посторонним процессом видна на границе фазы.
    #[test]
    fn foreign_scheme_is_detected_at_the_phase_boundary() {
        let driver = RecordingDriver::new("new-guid");
        assert_eq!(verify_scheme_active(&driver, "new-guid"), Ok(()));
        let err = verify_scheme_active(&driver, "other-guid")
            .expect_err("чужая схема обязана обнаруживаться");
        assert!(err.contains("other-guid"), "{err}");
        assert!(err.contains("new-guid"), "{err}");
        assert!(err.contains("извне"), "{err}");
    }

    /// Регистр GUID не должен считаться подменой: powercfg отдаёт его в
    /// верхнем регистре, а план — как его ввёл пользователь.
    #[test]
    fn scheme_comparison_ignores_case() {
        let driver = RecordingDriver::new("381B4222-F694-41F0-9685-FF5BB260DF2E");
        assert_eq!(
            verify_scheme_active(&driver, "381b4222-f694-41f0-9685-ff5bb260df2e"),
            Ok(())
        );
    }

    // ------------------------------------------------------------------
    // Волна 0, C8: возврат исходной схемы только с подтверждением от ОС
    // ------------------------------------------------------------------

    /// Хранилище чекпоинта в памяти: `restore_original` не должен писать на
    /// диск пользователя.
    #[derive(Default)]
    struct MemStore {
        checkpoint: Option<Checkpoint>,
        saves: usize,
    }

    impl CheckpointStore for MemStore {
        fn load(&self) -> Result<Option<Checkpoint>, String> {
            Ok(self.checkpoint.clone())
        }
        fn save(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
            self.saves += 1;
            self.checkpoint = Some(checkpoint.clone());
            Ok(())
        }
    }

    fn plan() -> SessionConfig {
        SessionConfig {
            duration_seconds: 1,
            warmup_seconds: 0,
            cooling_seconds: 0,
            repetitions: 1,
            background_threshold_percent: 5.0,
            accept_dirty_background: false,
            worker_count: None,
            scheme_ids: vec!["test-guid".to_string()],
            plan_guid: "c8-plan".to_string(),
            reference_scheme_id: None,
        }
    }

    fn checkpoint_on(original: Option<&str>) -> Checkpoint {
        let mut cp = Checkpoint::new(plan());
        cp.original_scheme_guid = original.map(str::to_string);
        cp
    }

    /// Ключевой случай C8: `powercfg /setactive` вернул код 0, но план не
    /// вернулся. Прежние пути восстановления на этом строили вывод
    /// «схема восстановлена» и ставили `original_restored = true` — точка
    /// навсегда оставалась в состоянии «восстановлено», а машина продолжала
    /// работать на тестовом плане.
    #[test]
    fn restore_is_not_claimed_when_the_os_ignored_the_command() {
        let driver = RecordingDriver::lying("test-guid");
        let err = restore_verified(&driver, "original-guid")
            .expect_err("ОС не приняла схему — восстановление не состоялось");
        assert!(err.contains("original-guid"), "в ошибке нет GUID: {err}");
        assert!(
            err.contains("вернуть в ОС не удалось"),
            "ошибка обязана называть суть, а не код возврата: {err}"
        );
        assert!(
            err.contains("powercfg /setactive"),
            "в ошибке нужна инструкция, как починить руками: {err}"
        );
        assert!(
            driver.set_calls() >= RESTORE_ATTEMPTS,
            "возврат обязан быть повторён, а не выполнен один раз: {} попыток",
            driver.set_calls()
        );
    }

    /// Power Manager бывает занят: одна неудачная команда не должна
    /// оставлять машину на тестовой схеме.
    #[test]
    fn restore_retries_before_giving_up() {
        let driver = RecordingDriver::failing_first("test-guid", 2);
        assert_eq!(
            restore_verified(&driver, "original-guid"),
            Ok(()),
            "две неудачные попытки не должны считаться отказом"
        );
        assert_eq!(driver.active_scheme().unwrap(), "original-guid");
        assert_eq!(driver.set_calls(), 3, "ожидались три попытки");
    }

    /// Штатное восстановление: схема возвращена, ОС это подтвердила, флаг
    /// взведён и точка сохранена.
    #[test]
    fn restore_marks_the_flag_only_after_the_os_confirms() {
        let driver = RecordingDriver::new("test-guid");
        let mut store = MemStore::default();
        let mut cp = checkpoint_on(Some("original-guid"));
        let mut events = Vec::new();
        assert_eq!(
            restore_original(&driver, &mut cp, &mut events, &mut store, &None),
            Ok(())
        );
        assert!(cp.original_restored, "подтверждённый возврат не помечен");
        assert_eq!(store.saves, 1, "точка не сохранена после восстановления");
        assert_eq!(driver.active_scheme().unwrap(), "original-guid");
    }

    /// Регресс C8: неподтверждённый возврат не должен выставлять
    /// `original_restored` — иначе следующий запуск увидит «всё в порядке» и
    /// не станет чинить схему.
    #[test]
    fn unconfirmed_restore_keeps_the_flag_down() {
        let driver = RecordingDriver::lying("test-guid");
        let mut store = MemStore::default();
        let mut cp = checkpoint_on(Some("original-guid"));
        let mut events = Vec::new();
        let err = restore_original(&driver, &mut cp, &mut events, &mut store, &None)
            .expect_err("ОС не переключила схему — восстановление не состоялось");
        let text = format!("{err:?}");
        assert!(text.contains("original-guid"), "{text}");
        assert!(
            !cp.original_restored,
            "флаг взведён без подтверждения от ОС: следующий запуск не починит схему"
        );
        assert_eq!(store.saves, 0, "неподтверждённое восстановление не пишется");
        // И наружу успех объявлять нельзя: событие «схема восстановлена»
        // попало бы в отчёт и в журнал сессии.
        assert!(
            !events.iter().any(|e| matches!(e, SessionEvent::Restored { .. })),
            "восстановление объявлено успехом, хотя ОС не переключила схему: {events:?}"
        );
        assert!(
            cp.original_scheme_guid.is_some(),
            "исходный GUID обязан сохраниться для следующей попытки"
        );
    }

    /// Регресс C8: `original_scheme_guid == None` больше не выглядит как
    /// успешное восстановление. Прежний `Ok(())` означал «всё в порядке» при
    /// схеме, которую никто не может вернуть.
    #[test]
    fn unknown_original_scheme_is_not_a_successful_restore() {
        let driver = RecordingDriver::new("test-guid");
        let mut store = MemStore::default();
        let mut cp = checkpoint_on(None);
        let mut events = Vec::new();
        restore_original(&driver, &mut cp, &mut events, &mut store, &None)
            .expect_err("нечего восстанавливать — это не успех");
        assert!(
            !cp.original_restored,
            "неизвестная исходная схема не должна помечаться восстановленной"
        );
        assert!(
            events.iter().any(|e| matches!(e, SessionEvent::Warn(w) if w.contains("неизвестна"))),
            "нужно предупреждение о том, что вернуть нечем: {events:?}"
        );
    }

    /// Паника в сессии — тоже путь возврата, и он обязан повторять команду,
    /// а не верить одному коду возврата.
    #[test]
    fn os_restore_guard_retries_instead_of_trusting_one_command() {
        let driver = RecordingDriver::failing_first("test-guid", 1);
        drop(OsRestoreGuard {
            original: Some("original-guid".to_string()),
            driver: &driver,
            restored: Arc::new(AtomicBool::new(false)),
        });
        assert_eq!(
            driver.active_scheme().unwrap(),
            "original-guid",
            "после паники схема не вернулась, а попытка была одна"
        );
        assert_eq!(driver.set_calls(), 2);
    }

    /// Четыре пути возврата не должны снова начать звать «голый» `set_active`:
    /// это молчаливо вернуло бы баг C8 целиком. Проверяется по исходнику —
    /// воспроизвести отказ ОС во всех четырёх местах через публичный API
    /// нельзя, а регресс здесь молчаливый: код компилируется и работает, пока
    /// не вернётся ровно тот дефект, ради которого написаны проверки.
    #[test]
    fn no_restore_path_calls_bare_set_active() {
        let src = include_str!("session.rs");
        let slice = |from: &str, to: &str| -> String {
            let start = src.find(from).unwrap_or_else(|| panic!("не найдено: {from}"));
            let rest = &src[start..];
            let end = rest.find(to).unwrap_or_else(|| panic!("не найдено: {to}"));
            rest[..end].to_string()
        };
        let sites = [
            (
                "fn restore_original(",
                "/// Последний ли это плановый прогон",
            ),
            ("impl Drop for OsRestoreGuard", "fn restore_original("),
            (
                "    // Восстановление после прерывания: если активна не исходная схема —",
                "let n_schemes = plan.scheme_ids.len();",
            ),
        ];
        for (from, to) in sites {
            let body = slice(from, to);
            assert!(
                !body.contains(".set_active("),
                "путь восстановления снова зовёт «голый» set_active без \
                 подтверждения от ОС:\n{body}"
            );
        }
        // Продолжение сессии обязано идти через общий проверенный вход.
        let resume = slice(
            "    // Восстановление после прерывания: если активна не исходная схема —",
            "let n_schemes = plan.scheme_ids.len();",
        );
        assert!(
            resume.contains("restore_verified(driver"),
            "продолжение сессии обязано возвращать схему через restore_verified"
        );
    }

    // ------------------------------------------------------------------
    // Фаза 2: фон влияет на замер
    // ------------------------------------------------------------------

    /// Границы «чисто / шумно / слишком грязно».
    #[test]
    fn background_thresholds_are_ordered() {
        let cpus = 16;
        let threshold = 5.0;
        // Порог = 5 % * 16 = 80 % одного ядра суммарно.
        assert!(matches!(
            classify_background(79.0, threshold, cpus),
            BackgroundVerdict::Clean { .. }
        ));
        assert!(matches!(
            classify_background(80.0, threshold, cpus),
            BackgroundVerdict::Noisy { .. }
        ));
        // Критический порог = 3 * 80 = 240.
        assert!(matches!(
            classify_background(239.0, threshold, cpus),
            BackgroundVerdict::Noisy { .. }
        ));
        assert!(matches!(
            classify_background(240.0, threshold, cpus),
            BackgroundVerdict::TooDirty { .. }
        ));
    }

    /// Ровно на границе фон считается превышением, иначе «грязный» фон на
    /// ровно пороговом значении проходил бы как чистый.
    #[test]
    fn background_threshold_is_exclusive_at_the_bottom() {
        let (clean, noisy) = (
            classify_background(80.0, 5.0, 16),
            classify_background(80.01, 5.0, 16),
        );
        assert!(matches!(clean, BackgroundVerdict::Noisy { .. }));
        assert!(matches!(noisy, BackgroundVerdict::Noisy { .. }));
    }

    /// Неизвестная загрузка (NaN/бесконечность) не считается чистой.
    ///
    /// Иначе сломанный sysinfo дал бы «фон чистый» без единого подтверждения,
    /// то есть ровно то молчание, которое Фаза 2 и убирает.
    #[test]
    fn unknown_background_is_never_clean() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                matches!(
                    classify_background(bad, 5.0, 16),
                    BackgroundVerdict::TooDirty { .. }
                ),
                "{bad} не должен считаться чистым фоном"
            );
        }
    }

    /// Тяжёлый фон отменяет замер, а не портит его: без проверки схема
    /// получала заниженную оценку и могла уйти в карантин с ложной причиной.
    #[test]
    fn too_dirty_background_blocks_the_measurement() {
        assert!(matches!(
            classify_background(1000.0, 5.0, 16),
            BackgroundVerdict::TooDirty { .. }
        ));
        // Множитель критичности — именно множитель, а не «1 + запас».
        assert_eq!(BACKGROUND_DIRTY_FACTOR, 3.0);
    }

    // ------------------------------------------------------------------
    // Фаза 2: паузы прерываемы
    // ------------------------------------------------------------------

    /// Любая техническая пауза обязана прерываться по кнопке «Стоп».
    ///
    /// Раньше пауза после схемы, стабилизация после фазы и вся проверка фона
    /// были обычным `sleep` — вместе до 9,5 с молчания после нажатия.
    #[test]
    fn technical_sleeps_are_interrupted_by_user_cancel() {
        let cancel = AtomicBool::new(false);
        // Без отмены сон отрабатывает полностью.
        let started = Instant::now();
        assert!(!interruptible_sleep(&cancel, 0));
        assert!(started.elapsed() < Duration::from_secs(1));

        // С отменой — немедленно, независимо от запрошенной длительности.
        cancel.store(true, Ordering::Relaxed);
        let started = Instant::now();
        assert!(interruptible_sleep(&cancel, 600));
        assert!(interruptible_sleep_ms(&cancel, 60_000));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "отмена обязана будить сон мгновенно, ждали {:?}",
            started.elapsed()
        );
    }

    /// Отмена не должна «съедать» сон молча: без флага сон идёт до конца.
    #[test]
    fn a_long_sleep_actually_waits_when_not_cancelled() {
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        interruptible_sleep_ms(&cancel, 200);
        let spent = started.elapsed();
        assert!(
            spent >= Duration::from_millis(150),
            "сон отработал за {spent:?}"
        );
        assert!(spent < Duration::from_secs(3), "сон затянулся: {spent:?}");
    }

    // ------------------------------------------------------------------
    // Фаза 2: стабилизация схемы
    // ------------------------------------------------------------------

    /// Границы ожидания стабилизации упорядочены и осмысленны.
    ///
    /// Константы проверяются на неизменность: смысл теста именно в том, чтобы
    /// правка этих чисел не прошла молча.
    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn stabilization_bounds_are_sane() {
        assert!(
            SCHEME_STABILIZE_MIN_SECS >= 1,
            "минимум нужен для ramp частот"
        );
        assert!(
            SCHEME_STABILIZE_MAX_SECS > SCHEME_STABILIZE_MIN_SECS,
            "потолок обязан быть выше минимума, иначе ожидание бессмысленно"
        );
        assert!(
            SCHEME_STABILIZE_MAX_SECS <= 30,
            "пользователь не должен ждать полминуты"
        );
        assert!(SCHEME_STABILIZE_POLL_MS > 0);
        assert!(SCHEME_STABILIZE_STABLE_SAMPLES >= 2);
    }

    /// Отмена пользователем прерывает стабилизацию и не ждёт потолка.
    #[test]
    fn stabilization_stops_on_user_cancel() {
        let driver = RecordingDriver::new("scheme-guid");
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        let report = wait_scheme_stabilized(&driver, "scheme-guid", &cancel);
        assert!(!report.settled);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "отмена обязана прервать стабилизацию сразу, ждали {:?}",
            started.elapsed()
        );
    }

    /// Схема, которую ОС не показывает активной, стабилизацию не проходит.
    #[test]
    fn stabilization_fails_when_the_scheme_is_not_active() {
        let driver = RecordingDriver::new("other-guid");
        let cancel = AtomicBool::new(false);
        let report = wait_scheme_stabilized(&driver, "scheme-guid", &cancel);
        assert!(
            !report.settled,
            "неподтверждённая схема обязана давать сбой"
        );
        assert_eq!(
            report.reason,
            Some(SchemeStabilizeFailure::SchemeUnconfirmed)
        );
        assert!(report.waited_ms <= SCHEME_STABILIZE_MAX_SECS * 1000 + 1000);
    }

    // ------------------------------------------------------------------
    // Волна 1: C9 (незавершённый раунд), H3 (прогон без сэмплов),
    // H41 (подпись условий запуска)
    // ------------------------------------------------------------------

    fn order_of(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// Регресс H38: отравленный мьютекс фоновой карты не должен ронять ни поток
/// мониторинга, ни сессию.
///
/// Раньше стоял `lock().unwrap()`. Любая паника в другом владельце карты
/// (мониторинг, отчёт, тест) отравляла мьютекс, после чего первый же
/// `unwrap` убивал поток мониторинга — и прогон оставался без фоновых
/// корреляций до конца сессии, а сама сессия паниковала вместо того, чтобы
/// отдать неполные данные.
#[test]
fn a_poisoned_monitor_map_does_not_kill_the_session() {
    use std::collections::BTreeMap as Map;

    let map: Arc<Mutex<Map<u64, Vec<powerbench_windows::monitor::ProcessSample>>>> =
        Arc::new(Mutex::new(Map::new()));
    // Отравляем мьютекс паникой в его владельце.
    let poisoned = Arc::clone(&map);
    let _ = std::thread::spawn(move || {
        let _guard = poisoned.lock().expect("первая блокировка обязана удаться");
        panic!("отравляем карту фоновых измерений");
    })
    .join();
    assert!(map.lock().is_err(), "мьютекс не отравлен — тест бессмыслен");

    // Чтение обязано выдать хоть что-то, а не паниковать.
    let snapshot = map.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert!(snapshot.is_empty(), "из отравленной карты взяты не те данные");

    // И запись в отравленный мьютекс тоже обязана работать.
    map.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(1, Vec::new());
    assert_eq!(map.lock().unwrap_or_else(|e| e.into_inner()).len(), 1);
}

/// Мониторинг обязан переживать отравление мьютекса своей карты.
///
/// Иначе один заход с паникой в другом потоке оставлял бы весь прогон без
/// фоновых корреляций: `spawn_monitor` звал `map.lock().unwrap()`.
#[test]
fn the_monitor_thread_survives_a_poisoned_map() {
    use std::collections::BTreeMap as Map;

    let map: Arc<Mutex<Map<u64, Vec<powerbench_windows::monitor::ProcessSample>>>> =
        Arc::new(Mutex::new(Map::new()));
    let poisoned = Arc::clone(&map);
    let _ = std::thread::spawn(move || {
        let _guard = poisoned.lock().expect("первая блокировка обязана удаться");
        panic!("отравляем карту до старта мониторинга");
    })
    .join();

    let run = Arc::new(AtomicBool::new(true));
    let handle = spawn_monitor(Arc::clone(&map), Arc::clone(&run));
    std::thread::sleep(Duration::from_millis(1500));
    run.store(false, Ordering::Relaxed);
    // Поток обязан завершиться штатно: если бы он умер на панике, `join`
    // вернул бы ошибку, и мы бы это увидели.
    assert!(
        handle.join().is_ok(),
        "поток мониторинга умер на отравленном мьютексе"
    );
}
    /// Регресс M35: дамп настроек схемы хранится один раз на схему, а не в
    /// каждом прогоне.
    ///
    /// Чекпоинт переписывается целиком после каждого прогона. Дамп
    /// `powercfg /query` писался в каждый прогон, поэтому при повторах в файле
    /// накапливались копии одного и того же текста об одних и тех же настройках:
    /// размер чекпоинта рос линейно по числу прогонов, хотя уникальных данных
    /// в нём не прибавлялось. Итоговый JSON истории уносил эти копии с собой.
    ///
    /// Проверяем все три свойства: первая копия остаётся, повтор у той же схемы
    /// убирается, а другая схема сохраняет свой дамп.
    #[test]
    fn a_scheme_dump_is_stored_once_per_scheme() {
        let mut cp = checkpoint_on(None);
        cp.runs.clear();

        // Схема A: первый прогон — дамп нужен.
        let mut a1 = minimal_stored_run();
        a1.scheme_id = "AAAAAAAA-0000-0000-0000-000000000001".to_string();
        a1.round = 1;
        a1.scheme_dump = Some("НАСТРОЙКИ A".to_string());
        let a1 = drop_duplicate_scheme_dump(&cp, a1);
        assert!(
            a1.scheme_dump.is_some(),
            "первый прогон схемы обязан сохранить дамп: без него результат \
             невоспроизводим"
        );
        cp.runs.push(a1);

        // Тот же щит, второй раунд — тот же дамп, хранить его нечего.
        let mut a2 = minimal_stored_run();
        a2.scheme_id = "aaaaaaaa-0000-0000-0000-000000000001".to_string(); // другой регистр
        a2.round = 2;
        a2.scheme_dump = Some("НАСТРОЙКИ A".to_string());
        let a2 = drop_duplicate_scheme_dump(&cp, a2);
        assert!(
            a2.scheme_dump.is_none(),
            "дамп той же схемы сохранён повторно (и по нижнему регистру GUID — \
             тоже): чекпоинт снова распух"
        );
        cp.runs.push(a2);

        // Схема B — своя, её дамп обязан остаться.
        let mut b1 = minimal_stored_run();
        b1.scheme_id = "BBBBBBBB-0000-0000-0000-000000000002".to_string();
        b1.round = 1;
        b1.scheme_dump = Some("НАСТРОЙКИ B".to_string());
        let b1 = drop_duplicate_scheme_dump(&cp, b1);
        assert!(
            b1.scheme_dump.is_some(),
            "дамп другой схемы убран: это уже не дубликат"
        );
        cp.runs.push(b1);

        // Итог: два уникальных дампа на четыре прогона, а не четыре копии.
        let dumps = cp.runs.iter().filter(|r| r.scheme_dump.is_some()).count();
        assert_eq!(dumps, 2, "в чекпоинте {dumps} дампов вместо двух");
        assert_eq!(cp.runs.len(), 3);

        // Ничего не потеряно: по любому GUID настроек восстановить можно.
        for scheme in ["AAAAAAAA-0000-0000-0000-000000000001", "BBBBBBBB-0000-0000-0000-000000000002"] {
            assert!(
                cp.runs
                    .iter()
                    .any(|r| r.scheme_id.eq_ignore_ascii_case(scheme) && r.scheme_dump.is_some()),
                "по GUID {scheme} настройки восстановить нельзя"
            );
        }
    }

    /// Регресс M35 (продолжение): размер сериализованного чекпоинта не должен
    /// расти вместе с числом повторов одной схемы.
    ///
    /// Это и есть исходная жалоба: не «сколько полей», а сколько байт
    /// переписывается после каждого прогона.
    #[test]
    fn checkpoint_size_does_not_grow_with_repeats_of_one_scheme() {
        let dump = "x".repeat(4096);
        let dump_len = dump.len();
        let mut cp = checkpoint_on(None);
        cp.runs.clear();

        // Первый прогон схемы — с дампом.
        let mut first = minimal_stored_run();
        first.scheme_id = "AAAAAAAA-0000-0000-0000-000000000001".to_string();
        first.round = 1;
        first.scheme_dump = Some(dump.clone());
        cp.runs.push(drop_duplicate_scheme_dump(&cp, first));
        let with_first = serde_json::to_vec(&cp).expect("чекпоинт сериализуется").len();

        // Пять повторов той же схемы — без дампа.
        for round in 2..=6 {
            let mut next = minimal_stored_run();
            next.scheme_id = "AAAAAAAA-0000-0000-0000-000000000001".to_string();
            next.round = round;
            next.scheme_dump = Some(dump.clone());
            cp.runs.push(drop_duplicate_scheme_dump(&cp, next));
        }
        let with_all = serde_json::to_vec(&cp).expect("чекпоинт сериализуется").len();

        // Пять лишних прогонов не должны стоить шесть копий дампа: прирост
        // заметно меньше одной копии.
        let per_run = (with_all - with_first) / 5;
        assert!(
            per_run * 2 < dump_len,
            "повтор схемы добавляет {per_run} байт — это заметная часть дампа \
             в {dump_len} байт, копии снова пишутся"
        );
        // И дамп в файле ровно один.
        let dumps = cp.runs.iter().filter(|r| r.scheme_dump.is_some()).count();
        assert_eq!(dumps, 1);
    }

    /// Минимальный прогон для проверок, не касающихся его содержимого.
    fn minimal_stored_run() -> StoredRun {
        StoredRun {
            key: "0:plan".to_string(),
            round: 0,
            scheme_id: String::new(),
            scheme_name: None,
            started_at_ns: 0,
            duration_ms: 0,
            ticks: 0,
            supercycles: 0,
            first_tick_checksums: [0; PHASES_PER_RUN as usize],
            run_checksums: [0; PHASES_PER_RUN as usize],
            phases: Vec::new(),
            combined: zero_run_stats(),
            cross_phase_consistency: 0.0,
            burst_retention_percent: 0.0,
            background: Vec::new(),
            spike_windows: 0,
            power: None,
            scheme_dump: None,
            background_cpu_p50: 0.0,
            background_cpu_p95: 0.0,
            background_sample_seconds: 0,
            worker_count: 4,
            affinity_mode: "p-only".to_string(),
            affinity_signature: "p-only:test".to_string(),
            config_hash: "cfg-test".to_string(),
        }
    }

    fn checkpoint_with_runs(plan: SessionConfig, runs: &[(&str, u32)]) -> Checkpoint {
        let mut cp = Checkpoint::new(plan);
        for &(scheme, round) in runs {
            cp.runs.push(StoredRun {
                round,
                scheme_id: scheme.to_string(),
                ..minimal_stored_run()
            });
        }
        cp
    }

    /// Регресс C9: ключ раунда ставится только когда покрыты ВСЕ схемы раунда.
    ///
    /// Раньше ключ попадал в `completed_keys` безусловно, сразу после выхода из
    /// цикла по схемам. Раунд, где одна схема упала (план сменили извне, машина
    /// не загрузилась, не нашлось валидных сэмплов), считался завершённым, и
    /// при `resume` пропускался целиком: непокрытые схемы не перетестировались
    /// никогда, а сессия выглядела завершённой.
    #[test]
    fn a_round_with_a_missing_scheme_is_not_marked_completed() {
        let cp = checkpoint_with_runs(plan(), &[("a", 0), ("b", 0)]);
        let order = order_of(&["a", "b", "c"]);
        assert!(
            !round_is_covered(&cp, 0, &order),
            "раунд без схемы «c» помечен завершённым: resume её пропустит"
        );

        // Все три на месте — раунд закрыт.
        let full = checkpoint_with_runs(plan(), &[("a", 0), ("b", 0), ("c", 0)]);
        assert!(round_is_covered(&full, 0, &order));

        // Бракованная схема покрыта: её не перетестируют, и это правильно.
        let mut rejected = checkpoint_with_runs(plan(), &[("a", 0), ("b", 0)]);
        rejected.rejections.insert("c".to_string(), "брак".to_string());
        assert!(
            round_is_covered(&rejected, 0, &order),
            "забракованная схема считается непокрытой: её придётся мерить снова"
        );
    }

    /// Регресс C9: прогон ДРУГОГО раунда не закрывает текущий.
    ///
    /// Иначе `resume`, у которого в точке есть прогоны поздних раундов, счёл бы
    /// ранний раунд завершённым по чужим записям.
    #[test]
    fn runs_from_another_round_do_not_close_this_one() {
        let cp = checkpoint_with_runs(plan(), &[("a", 1), ("b", 1)]);
        let order = order_of(&["a", "b"]);
        assert!(
            !round_is_covered(&cp, 0, &order),
            "прогоны раунда 1 закрыли раунд 0"
        );
        assert!(round_is_covered(&cp, 1, &order));
    }

    /// Ключ раунда ставится по результату проверки покрытия, а не безусловно.
    #[test]
    fn the_round_key_is_written_only_when_the_round_is_covered() {
        let src = include_str!("session.rs");
        let start = src
            .find("fn run_session_loop(")
            .expect("не найден run_session_loop");
        let body = &src[start..];
        let guard_at = body
            .find("let round_covered = round_is_covered(checkpoint, round, &round_order_schemes);")
            .expect("покрытие раунда не проверяется");
        let push_at = body
            .find("checkpoint.completed_keys.push(key.clone());")
            .expect("не найдено добавление ключа раунда");
        assert!(
            guard_at < push_at,
            "ключ раунда добавляется раньше проверки покрытия"
        );
    }

    /// Незавершённый раунд исключает раннюю остановку (заметка из Волны 1).
    ///
    /// Раньше `done_rounds.max(round + 1)` засчитывал незавершённый раунд как
    /// завершённый, и «осталось N раундов» в решении о досрочной остановке
    /// показывалось неверно. Договорённость: пока часть плана не измерена,
    /// останавливаться рано нельзя.
    #[test]
    fn completed_rounds_counts_only_rounds_with_a_key() {
        let mut cp = Checkpoint::new(plan());
        // Ничего не завершено.
        assert_eq!(completed_rounds(&cp, 0, "c8-plan"), 0);
        // Раунд 0 закрыт, раунд 1 — нет.
        cp.completed_keys.push(run_key(0, "c8-plan"));
        assert_eq!(completed_rounds(&cp, 0, "c8-plan"), 1);
        assert_eq!(completed_rounds(&cp, 1, "c8-plan"), 1);
        // Оба закрыты.
        cp.completed_keys.push(run_key(1, "c8-plan"));
        assert_eq!(completed_rounds(&cp, 1, "c8-plan"), 2);
        // Ключ чужого плана не засчитывается.
        cp.completed_keys.push(run_key(2, "other-plan"));
        assert_eq!(
            completed_rounds(&cp, 2, "c8-plan"),
            2,
            "чужой ключ попал в счётчик завершённых раундов"
        );
        // Ключ будущего раунда тоже не влияет на счёт за текущий.
        cp.completed_keys.push(run_key(9, "c8-plan"));
        assert_eq!(completed_rounds(&cp, 1, "c8-plan"), 2);
    }

    /// Ранняя остановка не должна срабатывать на незакрытом раунде.
    ///
    /// Проверяется по исходнику: решение о досрочной остановке принимается
    /// внутри цикла раундов, куда без живого движка не попасть.
    #[test]
    fn an_incomplete_round_forbids_early_stop() {
        let src = include_str!("session.rs");
        let start = src
            .find("fn run_session_loop(")
            .expect("не найден run_session_loop");
        let body = &src[start..];
        let guard_at = body
            .find("if !round_covered {")
            .expect("незавершённый раунд не исключает раннюю остановку");
        let decision_at = body
            .find("crate::early_stop::early_stop_decision(")
            .expect("не найдено решение о ранней остановке");
        assert!(
            guard_at < decision_at,
            "решение о ранней остановке принимается до проверки покрытия раунда"
        );
        // Число завершённых раундов идёт из ключей точки, а не выводится из
        // номера итерации (сама арифметика — в `completed_rounds`).
        let between = &body[guard_at..decision_at];
        assert!(
            between.contains("completed_rounds(checkpoint, round, &plan.plan_guid)"),
            "число завершённых раундов не берётся из ключей точки"
        );
    }

    /// Регресс H3: прогон без валидных сэмплов обязан быть опознан как
    /// несостоявшийся.
    ///
    /// `run_stats` возвращает `None`, и раньше подставлялся `RunStats` из нулей.
    /// Такой прогон попадал в чекпоинт как обычный: нули тянули среднее и σ
    /// схемы к нулю, а сама схема ранжировалась наравне с честно измеренными.
    #[test]
    fn a_run_without_valid_samples_is_not_treated_as_a_measurement() {
        let stats = run_stats(&[]);
        assert!(stats.is_none(), "пустая выборка обязана давать None");
        let zeros = stats.unwrap_or_else(zero_run_stats);
        assert_eq!(
            zeros.samples, 0,
            "прогон без сэмплов не отличить от настоящего измерения"
        );
        assert_eq!(zeros.average_throughput, 0.0);

        // И наоборот: прогон с данными — это измерение с положительным средним,
        // и никакой нулевой маркер на нём не должен срабатывать.
        let real = run_stats(&[1.0, 2.0, 1.5]).expect("есть валидные сэмплы");
        assert!(real.samples > 0, "валидные сэмплы не опознаны");
        assert!(real.average_throughput > 0.0);
        assert_ne!(real.average_throughput, zeros.average_throughput);
    }

    /// Нулевой прогон обязан отбрасываться до записи в чекпоинт.
    ///
    /// Иначе нули попадут в агрегат и испортят среднее и σ всей схемы.
    #[test]
    fn a_run_with_zero_samples_is_skipped_before_being_stored() {
        let src = include_str!("session.rs");
        let start = src
            .find("let combined = run_stats(&all_times)")
            .expect("не найден расчёт объединённой статистики");
        let body = &src[start..];
        let end = body
            .find("if let Some(run) = run_outcome {")
            .expect("не найдена запись прогона в чекпоинт");
        let build = &body[..end];
        let guard_at = build
            .find("if combined.samples == 0 {")
            .expect("нулевой прогон не отбрасывается");
        assert!(
            build[guard_at..].contains("continue 'scheme"),
            "нулевой прогон обязан уйти из цикла по схемам, а не быть записан"
        );
        assert!(
            !build[guard_at..].contains("build_stored_run"),
            "нулевой прогон всё равно попал в чекпоинт"
        );
    }

    /// Регресс H41: подпись условий запуска обязана ловить расхождение.
    ///
    /// Именно на этом держался дефект: `StoredRun` условий не содержал, и
    /// `resume` с другим числом воркеров перештамповывал старые прогоны
    /// подписью текущего движка — в одном отчёте оказывались числа, измеренные
    /// при разных условиях.
    #[test]
    fn run_conditions_mismatch_names_the_field_that_differs() {
        let base = LaunchConditions {
            worker_count: 4,
            affinity_mode: "p-only".to_string(),
            affinity_signature: "p-only:0,1,2,3".to_string(),
            config_hash: "cfg-a".to_string(),
        };
        assert_eq!(
            base.first_difference(&base),
            None,
            "своя подпись сравнивается с собой"
        );

        // Порядок влияния на величину: сначала число воркеров.
        let other_workers = LaunchConditions {
            worker_count: 8,
            ..base.clone()
        };
        let diff = other_workers
            .first_difference(&base)
            .expect("разное число воркеров не замечено");
        assert!(diff.contains("воркер"), "{diff}");

        let other_affinity = LaunchConditions {
            affinity_signature: "p-only:0,2,4,6".to_string(),
            ..base.clone()
        };
        let diff = other_affinity
            .first_difference(&base)
            .expect("разная привязка не замечена");
        assert!(diff.contains("привязк"), "{diff}");

        let other_config = LaunchConditions {
            config_hash: "cfg-b".to_string(),
            ..base.clone()
        };
        let diff = other_config
            .first_difference(&base)
            .expect("разная конфигурация не замечена");
        assert!(diff.contains("конфигурац"), "{diff}");

        // Условия, которых нет ни у кого, — это не расхождение: старые точки
        // читаются, а не отвергаются.
        let unknown = LaunchConditions {
            worker_count: 0,
            affinity_mode: String::new(),
            affinity_signature: String::new(),
            config_hash: String::new(),
        };
        assert_eq!(
            unknown.first_difference(&base),
            None,
            "отсутствующие условия объявлены расхождением: старые точки \
             перестанут читаться"
        );
    }

    /// Точки, записанные прошлой версией, условий не содержат и обязаны
    /// продолжать читаться — иначе волна обновлений ломает все старые сессии.
    #[test]
    fn a_checkpoint_without_launch_conditions_still_resumes() {
        let mut old = minimal_stored_run();
        old.worker_count = 0;
        old.affinity_mode = String::new();
        old.affinity_signature = String::new();
        old.config_hash = String::new();
        assert!(
            !old.has_launch_conditions(),
            "старая точка не должна считаться хранящей условия"
        );
        assert_eq!(old.launch_conditions().worker_count, 0);

        // И десериализация старого JSON не падает: полей нет вовсе, `default`
        // подставляет нули и пустые строки.
        let legacy = r#"{
            "key": "0:plan",
            "round": 0,
            "scheme_id": "s1",
            "started_at_ns": 0,
            "duration_ms": 1000,
            "ticks": 100,
            "supercycles": 0,
            "first_tick_checksums": [1, 2, 3, 4],
            "run_checksums": [1, 2, 3, 4],
            "phases": [],
            "combined": {"samples": 10, "samples_raw": 10, "excluded_fraction": 0.0,
                "work_units": 10, "active_time_ms_total": 2.0, "average_throughput": 5000.0,
                "average_execution_time_ms": 0.2, "median_throughput": 5000.0,
                "p1_throughput": 5000.0, "p01_throughput": 5000.0,
                "p95_execution_time_ms": 0.2, "p99_execution_time_ms": 0.2,
                "consistency_percent": 100.0, "jitter_p99_ms": 0.0},
            "cross_phase_consistency": 100.0,
            "burst_retention_percent": 100.0,
            "background": [],
            "spike_windows": 0
        }"#;
        let back: StoredRun = serde_json::from_str(legacy).expect("старый формат читается");
        assert_eq!(back.worker_count, 0);
        assert!(back.config_hash.is_empty());
        assert!(!back.has_launch_conditions());

        // Проверка на движке такие прогоны пропускает: неизвестные условия —
        // не повод отказывать в продолжении.
        let engine = Engine::new(Some(4));
        assert!(
            run_conditions_mismatch(&[old], &engine).is_none(),
            "старая точка без условий блокирует продолжение сессии"
        );
    }

    /// А вот расхождение условий блокировать обязано.
    #[test]
    fn a_checkpoint_with_other_conditions_blocks_resume() {
        let engine = Engine::new(Some(4));
        let now = current_run_conditions(&engine);

        let mut mismatched = minimal_stored_run();
        mismatched.worker_count = now.worker_count + 4;
        mismatched.config_hash = "cfg-other".to_string();
        let err = run_conditions_mismatch(&[mismatched], &engine)
            .expect("чужие условия продолжения не замечены");
        let text = format!("{err}");
        assert!(text.contains("других условиях запуска"), "{text}");
        assert!(text.contains("воркеров"), "{text}");
        assert!(
            text.contains("начните новую сессию"),
            "пользователю нужно сказать, что делать: {text}"
        );

        // Совпадающие условия продолжению не мешают.
        let same = StoredRun {
            worker_count: now.worker_count,
            affinity_mode: now.affinity_mode.clone(),
            affinity_signature: now.affinity_signature.clone(),
            config_hash: now.config_hash.clone(),
            ..minimal_stored_run()
        };
        assert!(
            run_conditions_mismatch(&[same], &engine).is_none(),
            "прогон, сделанный при тех же условиях, блокирует продолжение"
        );
    }

    /// Каждый записанный прогон обязан нести условия запуска: иначе проверка
    /// при `resume` не увидит расхождения, потому что сведёт его «ни к чему».
    #[test]
    fn stored_runs_always_carry_launch_conditions() {
        let src = include_str!("session.rs");
        let start = src
            .find("fn build_stored_run(")
            .expect("не найдена build_stored_run");
        let body = &src[start..];
        let end = body
            .find("\n/// Итог агрегации")
            .expect("не найден конец build_stored_run");
        let body = &body[..end];
        for field in [
            "worker_count: conditions.worker_count",
            "affinity_mode: conditions.affinity_mode",
            "affinity_signature: conditions.affinity_signature",
            "config_hash: conditions.config_hash",
        ] {
            assert!(
                body.contains(field),
                "StoredRun не сохраняет {field}: resume перештампует прогон подписью \
                 текущего движка"
            );
        }
    }
}
