//! Фоновый запуск сессии из интерфейса: поток-исполнитель, наблюдатель
//! телеметрии (события `log`, `telemetry`, `test-finished`), гарантия ОС
//! (запрет сна, восстановление исходной схемы — внутри orchestator::run_session).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

// `creation_flags` живёт не в общем API `Command`, а в Windows-расширении.
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use powerbench_core::engine::{Engine, ProgressSnapshot};
use powerbench_metrics::AggregateResult;
use powerbench_orchestrator::appsettings::AppSettings;
use powerbench_orchestrator::checkpoint::StoredRun;
use powerbench_orchestrator::config::SessionConfig;
use powerbench_orchestrator::result::{
    IdentityJson, RecommendationScheme, SessionJson, build_session_json,
};
use powerbench_orchestrator::session::{
    DiskCheckpointStore, RealSchemeDriver, SessionEvent, TelemetryObserver, run_session,
    session_signature,
};
use powerbench_recommend::{EvidenceLevel, Recommendation};
use powerbench_windows::power::SleepGuard;

/// Дескриптор запущенной сессии для остановки и проверки занятости.
pub struct RunnerHandle {
    pub cancel: Arc<AtomicBool>,
    pub join: Option<JoinHandle<()>>,
    /// Что измеряется прямо сейчас; `None`, когда сессии нет.
    pub status: Arc<Mutex<Option<String>>>,
}

/// Сколько ждём завершения сессии при закрытии окна, мс.
const JOIN_TIMEOUT_MS: u64 = 10_000;

/// Сообщение о завершении сессии (событие `test-finished`).
#[derive(Clone, Serialize)]
pub struct FinishedPayload {
    pub ok: bool,
    pub error: Option<String>,
    pub plan_guid: Option<String>,
    pub cancelled: Option<bool>,
    pub early_stopped: bool,
    pub result_path: Option<String>,
    pub report_path: Option<String>,
    pub level: Option<String>,
    pub level_label: Option<String>,
    pub recommended_scheme: Option<String>,
    pub recommended_name: Option<String>,
    pub expected_margin_percent: Option<f64>,
    pub winner_margin_percent: Option<f64>,
}

/// Контекст текущей измеряемой фазы для вычисления скорости в teлеметрии.
struct ObsCtx {
    run_index: u32,
    run_total: u32,
    round: u32,
    scheme_id: String,
    scheme_name: String,
    phase: String,
    phase_seconds: u64,
    phase_started: Instant,
    last_ticks: u64,
    last_time: Instant,
    /// Замеренная фоновая нагрузка, % CPU. `None` — ещё не измерялось.
    background_percent: Option<f64>,
    /// Фон превысил порог и влияет на результат.
    background_noisy: bool,
}

impl Default for ObsCtx {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            run_index: 0,
            run_total: 0,
            round: 0,
            scheme_id: String::new(),
            scheme_name: String::new(),
            phase: String::new(),
            phase_seconds: 0,
            phase_started: now,
            last_ticks: 0,
            last_time: now,
            background_percent: None,
            background_noisy: false,
        }
    }
}

/// Наблюдатель сессии: рассылает события и 10-Гц телеметрию во фронтенд.
struct AppObserver {
    app: AppHandle,
    ctx: Mutex<ObsCtx>,
    /// Общая с владельцем сессии строка «что сейчас измеряется». Нужна
    /// отчёту для поддержки: он снимается посреди замера, и без этой
    /// строки неизвестно, на какой фазе всё остановилось.
    shared_status: Option<Arc<Mutex<Option<String>>>>,
}

/// Телеметрия одного тика (~10 Гц во время измеряемой фазы).
#[derive(Clone, Serialize)]
struct TelemetryMsg {
    running: bool,
    phase: String,
    phase_seconds: u64,
    ticks_done: u64,
    ticks_per_sec: f64,
    ms_per_tick: f64,
    run_index: u32,
    run_total: u32,
    round: u32,
    scheme_id: String,
    scheme_name: String,
    phase_elapsed_ms: u64,
    /// Фоновая нагрузка, % CPU: `null`, пока не измерена.
    background_percent: Option<f64>,
    /// Фон превысил порог — результату стоит доверять с оговоркой.
    background_noisy: bool,
}

#[derive(Clone, Serialize)]
struct LogMsg {
    level: String,
    text: String,
    ts_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn log_level(e: &SessionEvent) -> &'static str {
    match e {
        SessionEvent::Warn(_)
        | SessionEvent::BackgroundNoisy { .. }
        | SessionEvent::SchemeRejected { .. } => "warn",
        // Ранняя остановка — не сбой и не брак: это штатное завершение, когда
        // перевес лидера уже перекрыт разбросом повторов. Уровень `info`, а не
        // `warn`: жёлтый указывал бы на проблему там, где её нет, и
        // пользователь искал бы неисправность, которой не было.
        SessionEvent::EarlyStopped { .. } => "info",
        SessionEvent::RunCompleted { .. }
        | SessionEvent::PhaseFinished { .. }
        | SessionEvent::Restored { .. }
        | SessionEvent::BackgroundClean { .. }
        | SessionEvent::Finished => "success",
        _ => "info",
    }
}

impl AppObserver {
    /// Захват контекста телеметрии.
    ///
    /// Отравление мьютекса игнорируем: если поток паники случится внутри
    /// критической секции, обычный `unwrap()` превратил бы каждое следующее
    /// обновление телеметрии в новую панику, и сессия «залипла» бы намертво.
    fn lock_ctx(&self) -> std::sync::MutexGuard<'_, ObsCtx> {
        self.ctx.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl TelemetryObserver for AppObserver {
    fn event(&self, e: &SessionEvent) {
        // Фоновая нагрузка попадает в телеметрию: интерфейс должен прямо
        // предупредить, что фон мешает и результат может быть занижен.
        match e {
            SessionEvent::BackgroundNoisy {
                measured_total_percent,
                ..
            } => {
                let mut ctx = self.lock_ctx();
                ctx.background_percent = Some(*measured_total_percent);
                ctx.background_noisy = true;
            }
            SessionEvent::BackgroundClean {
                measured_total_percent,
                ..
            } => {
                let mut ctx = self.lock_ctx();
                ctx.background_percent = Some(*measured_total_percent);
                ctx.background_noisy = false;
            }
            _ => {}
        }
        let msg = LogMsg {
            level: log_level(e).to_string(),
            text: e.to_string(),
            ts_ms: now_ms(),
        };
        persist_log(&self.app, &msg.level, &msg.text);
        let _ = self.app.emit("log", msg);
    }

    fn phase(
        &self,
        run_index: u32,
        run_total: u32,
        round: u32,
        scheme_id: &str,
        scheme_name: &str,
        label: &str,
        seconds: u64,
    ) {
        let now = Instant::now();
        // Первая фаза прогона — момент, когда ещё нечего видеть в журнале,
        // кроме безымянного «фаза <Лёгкая>: 12 с». Таких строк за сессию
        // десятки, и без схемы с раундом непонятно, чей это был замер.
        // Одна строка «прогон N/M, раунд R, схема …» делает журнал пригодным
        // для разбора без обращения к файлу результата.
        let is_first_phase = {
            let ctx = self.lock_ctx();
            ctx.phase.is_empty() || ctx.scheme_id != scheme_id || ctx.round != round
        };
        if is_first_phase {
            persist_log(
                &self.app,
                "info",
                &format!(
                    "прогон {run_index}/{run_total}, раунд {round}: схема «{scheme_name}» \
                     [{scheme_id}], первая фаза «{label}» ({seconds} с)"
                ),
            );
        }
        let mut ctx = self.lock_ctx();
        ctx.run_index = run_index;
        ctx.run_total = run_total;
        ctx.round = round;
        ctx.scheme_id = scheme_id.to_string();
        ctx.scheme_name = scheme_name.to_string();
        ctx.phase = label.to_string();
        ctx.phase_seconds = seconds;
        ctx.phase_started = now;
        ctx.last_ticks = 0;
        ctx.last_time = now;
        if let Some(shared) = &self.shared_status
            && let Ok(mut g) = shared.lock()
        {
            *g = Some(format!(
                "прогон {run_index}/{run_total}, раунд {round}, схема «{scheme_name}», \
                 фаза «{label}»"
            ));
        }
    }

    fn tick(&self, snap: &ProgressSnapshot, phase_label: &str, phase_started: &Instant) {
        let ctx = self.lock_ctx();
        let now = Instant::now();
        let dt_ms = now.duration_since(ctx.last_time).as_millis().max(1) as f64;
        let delta = snap.ticks_done.saturating_sub(ctx.last_ticks);
        let mut msg = TelemetryMsg {
            running: snap.running,
            phase: phase_label.to_string(),
            phase_seconds: ctx.phase_seconds,
            ticks_done: snap.ticks_done,
            ticks_per_sec: 0.0,
            ms_per_tick: 0.0,
            run_index: ctx.run_index,
            run_total: ctx.run_total,
            round: ctx.round,
            scheme_id: ctx.scheme_id.clone(),
            scheme_name: ctx.scheme_name.clone(),
            phase_elapsed_ms: phase_started.elapsed().as_millis() as u64,
            background_percent: ctx.background_percent,
            background_noisy: ctx.background_noisy,
        };
        if delta > 0 && dt_ms > 0.0 {
            let tps = delta as f64 / dt_ms * 1000.0;
            msg.ticks_per_sec = tps;
            msg.ms_per_tick = 1000.0 / tps;
        }
        drop(ctx);
        let _ = self.app.emit("telemetry", msg);
    }
}

/// Идентичность нагрузки (для страницы Настроек) из реального движка.
pub fn identity_of(engine: &Engine) -> IdentityJson {
    let sig = session_signature(engine);
    IdentityJson {
        workload_version: sig.workload_version,
        config_hash: sig.config_hash,
        seed_hex: format!("{:016X}", sig.seed),
        worker_count: sig.worker_count,
        logical_cpus: sig.logical_cpus,
        timer_hz: sig.timer_hz,
        cpu_identifier: sig.cpu_identifier,
        diagnostics_version: sig.diagnostics_version,
        os_build: powerbench_windows::power::os_build(),
        memory_gib: powerbench_windows::power::memory_gib(),
        cpu_brand: powerbench_windows::power::cpu_brand(),
        affinity_mode: sig.affinity_mode,
        affinity_signature: sig.affinity_signature,
    }
}

fn prepare_engine(workers: Option<usize>) -> Result<Engine, String> {
    let mut engine = Engine::new(workers);
    engine
        .self_check()
        .map_err(|e| format!("самопроверка ядра не прошла: {e:?}"))?;
    Ok(engine)
}

fn empty_aggregate() -> AggregateResult {
    AggregateResult {
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
    }
}

fn empty_recommendation() -> Recommendation {
    Recommendation {
        level: EvidenceLevel::None,
        recommended_scheme: None,
        runner_up_scheme: None,
        reason: "рекомендация не сформирована".to_string(),
        three_probabilities: None,
        expected_margin_percent: None,
        bootstrap_mode: None,
        tie_criterion: None,
    }
}

/// Настоящая ошибка, а не предупреждение.
///
/// Уровень `error` существует в фильтре журнала, но раньше не использовался:
/// сбои вроде «не удалось сохранить в историю» попадали в `warn`, и фильтр
/// «Ошибки» в UI всегда показывал ноль. Теперь они действительно ошибки.
fn error_of(app: &AppHandle, text: &str) {
    log_at(app, "error", text);
}

fn log_at(app: &AppHandle, level: &str, text: &str) {
    let msg = LogMsg {
        level: level.into(),
        text: text.to_string(),
        ts_ms: now_ms(),
    };
    persist_log(app, level, text);
    let _ = app.emit("log", msg);
}

/// Дублировать запись журнала в персистентный `Logger` (если состояние есть).
fn persist_log(app: &AppHandle, level: &str, text: &str) {
    if let Some(state) = app.try_state::<crate::bridge::AppState>() {
        state.log.append(level, text);
    }
}

/// Тело фонового потока сессии.
/// Потокобезопасное описание текущей сессии для отчёта и журнала.
///
/// Отчёт для поддержки снимают посреди замера чаще, чем после него: если
/// приложение «зависло», снять отчёт — единственный способ узнать, на чём
/// именно оно остановилось. Поэтому строка обновляется на каждой фазе.
fn run_test(
    app: AppHandle,
    plan: SessionConfig,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<Option<String>>>,
) {
    let mut engine = match prepare_engine(plan.worker_count) {
        Ok(e) => e,
        Err(e) => {
            error_of(&app, &format!("не удалось подготовить движок: {e}"));
            let _ = app.emit(
                "test-finished",
                FinishedPayload {
                    ok: false,
                    error: Some(e),
                    ..empty_finished()
                },
            );
            return;
        }
    };
    let guard = match SleepGuard::prevent() {
        Ok(g) => g,
        Err(e) => {
            error_of(&app, &format!("не удалось запретить сон: {e:?}"));
            let _ = app.emit(
                "test-finished",
                FinishedPayload {
                    ok: false,
                    error: Some(format!("не удалось запретить сон: {e:?}")),
                    ..empty_finished()
                },
            );
            return;
        }
    };
    let _ = guard;
    // Параметры замера и идентичность движка пишем в журнал ДО старта сессии.
    // Именно этой строкой потом объясняется «контрольная сумма различается
    // между повторами»: если хэш конфигурации или число воркеров отличаются
    // от прошлой сессии, расхождение ожидаемо, а не дефект. Топология и
    // привязка — здесь же: они тоже часть идентичности замера.
    persist_log(
        &app,
        "info",
        &format!(
            "старт замера: план {}, схем {}, раундов {}, длительность прогона {} с, \
             разогрев {} с, охлаждение {} с, порог фона {} %; \
             нагрузка {}, конфигурация {}, seed {:016X}, воркеров {}, ядер {}; {}",
            plan.plan_guid,
            plan.scheme_ids.len(),
            plan.repetitions,
            plan.duration_seconds,
            plan.warmup_seconds,
            plan.cooling_seconds,
            plan.background_threshold_percent,
            engine.version(),
            short_hash(engine.config_hash()),
            engine.seed(),
            engine.worker_count(),
            engine.logical_cpus(),
            engine.topology().describe(),
        ),
    );
    let observer = Arc::new(AppObserver {
        app: app.clone(),
        ctx: Mutex::new(ObsCtx::default()),
        shared_status: Some(Arc::clone(&status)),
    });
    let mut store = DiskCheckpointStore;
    let outcome = match run_session(
        &mut engine,
        &RealSchemeDriver,
        plan.clone(),
        cancel,
        &mut store,
        Some(observer.clone()),
    ) {
        Ok(o) => o,
        Err(e) => {
            error_of(&app, &format!("ошибка сессии: {e}"));
            let _ = app.emit(
                "test-finished",
                FinishedPayload {
                    ok: false,
                    error: Some(format!("ошибка сессии: {e}")),
                    plan_guid: Some(plan.plan_guid),
                    ..empty_finished()
                },
            );
            return;
        }
    };

    let rec = outcome
        .recommendation
        .clone()
        .unwrap_or_else(empty_recommendation);
    let recommendation_schemes: Vec<RecommendationScheme> = outcome
        .checkpoint
        .plan
        .scheme_ids
        .iter()
        .map(|id| {
            let agg = outcome
                .aggregates
                .iter()
                .find(|(sid, _)| sid.eq_ignore_ascii_case(id))
                .map(|(_, a)| a.clone())
                .unwrap_or_else(empty_aggregate);
            let rejected = outcome.rejection_reasons.contains_key(id);
            let reason = outcome.rejection_reasons.get(id).cloned();
            let per_run: Vec<StoredRun> = outcome
                .checkpoint
                .runs
                .iter()
                .filter(|r| r.scheme_id.eq_ignore_ascii_case(id))
                .cloned()
                .collect();
            (id.clone(), rejected, reason, agg, per_run)
        })
        .collect();

    let warnings: Vec<String> = outcome
        .events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Warn(w) => Some(w.clone()),
            _ => None,
        })
        .collect();
    let settings = AppSettings::load();
    let score_weights = [
        settings.scoring.performance,
        settings.scoring.stability,
        settings.scoring.worst_second,
    ];
    let json = build_session_json(
        &outcome.checkpoint,
        identity_of(&engine),
        outcome
            .aggregates
            .iter()
            .map(|(id, a)| (id.clone(), a.clone()))
            .collect(),
        &recommendation_schemes,
        &rec,
        warnings,
        outcome.cancelled,
        score_weights,
        outcome.early_stop_reason,
    );
    let saved = match powerbench_orchestrator::history::save_result(&json) {
        Ok(path) => Some(path.display().to_string()),
        Err(e) => {
            error_of(&app, &format!("не удалось сохранить в историю: {e}"));
            None
        }
    };
    let report_path = match write_session_report(&json) {
        Some(p) => Some(p),
        None => {
            error_of(&app, "не удалось сформировать HTML-отчёт по сессии");
            None
        }
    };
    // Сессия записана в историю — точка долетала своё, иначе следующий
    // новый запуск упрётся в «контрольная точка другого плана».
    if let Err(e) = powerbench_orchestrator::checkpoint::clear_checkpoint() {
        error_of(&app, &format!("не удалось очистить контрольную точку: {e}"));
    }
    let winner = json
        .recommendation
        .recommended_scheme
        .as_ref()
        .and_then(|id| {
            json.schemes
                .iter()
                .find(|sch| sch.scheme_id.eq_ignore_ascii_case(id))
        })
        .cloned();
    let _ = app.emit(
        "test-finished",
        FinishedPayload {
            ok: true,
            error: None,
            plan_guid: Some(json.plan_guid.clone()),
            cancelled: Some(outcome.cancelled),
            early_stopped: outcome.cancelled || json.rounds_completed < json.rounds_planned,
            result_path: saved,
            report_path,
            level: Some(json.recommendation.level.clone()),
            level_label: Some(json.recommendation.level_label.clone()),
            recommended_scheme: json.recommendation.recommended_scheme.clone(),
            recommended_name: winner.as_ref().and_then(|s| s.name.clone()),
            expected_margin_percent: json.recommendation.expected_margin_percent,
            winner_margin_percent: winner
                .as_ref()
                .filter(|s| s.margin.is_finite())
                .map(|s| s.margin),
        },
    );
}

/// Флаг `CREATE_NO_WINDOW` из `winbase.h`.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Запретить дочернему процессу создавать окно консоли.
///
/// Приложение — GUI. Без этого флага `cmd /C start` (и `rundll32`) мигали
/// чёрным окном поверх интерфейса при каждом открытии отчёта или папки.
pub fn hide_console(cmd: &mut std::process::Command) {
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    #[cfg(not(target_os = "windows"))]
    let _ = cmd;
}

/// Открыть файл приложением по умолчанию (для HTML — браузер).
/// На Windows пробует несколько способов: rundll32 (надёжнее всего для
/// URL/file-протокола), затем `cmd /C start`, затем проводник.
pub fn open_with_default_app(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let arg = path.to_string_lossy().to_string();
        // Способ 1: rundll32 — штатный запуск ассоциации файла.
        let mut c1 = std::process::Command::new("rundll32");
        c1.args(["url.dll,FileProtocolHandler", &arg]);
        hide_console(&mut c1);
        if c1.spawn().is_ok() {
            return Ok(());
        }
        // Способ 2: cmd /C start "" <path> (пустой заголовок обязателен,
        // иначе путь в кавычках трактуется как заголовок окна).
        let mut c2 = std::process::Command::new("cmd");
        c2.args(["/C", "start", "", &arg]);
        hide_console(&mut c2);
        if c2.spawn().is_ok() {
            return Ok(());
        }
        // Способ 3: explorer делегирует открытие ассоциированному приложению.
        let mut c3 = std::process::Command::new("explorer");
        c3.arg(&arg);
        hide_console(&mut c3);
        c3.spawn()
            .map(|_| ())
            .map_err(|e| format!("не удалось открыть «{}»: {e}", path.display()))
    }
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("не удалось открыть «{}»: {e}", path.display()));
    }
    #[cfg(target_os = "linux")]
    {
        return std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("не удалось открыть «{}»: {e}", path.display()));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        return Err("открытие файлов не поддерживается на этой ОС".to_string());
    }
}

/// Сформировать HTML-отчёт по сессии рядом с результатами истории и открыть в браузере.
pub fn write_session_report(json: &SessionJson) -> Option<String> {
    match write_session_report_strict(json) {
        Ok(path) => Some(path),
        Err(e) => {
            eprintln!("PowerBench: {e}");
            None
        }
    }
}

/// То же, но с причиной ошибки: файл всегда создаётся, открытие браузера —
/// best-effort с понятным сообщением при неудаче.
pub fn write_session_report_strict(json: &SessionJson) -> Result<String, String> {
    let html = powerbench_orchestrator::report::build_session_report(json);
    let dir = powerbench_orchestrator::history::results_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("не удалось создать каталог «{}»: {e}", dir.display()))?;
    let start = powerbench_orchestrator::history::session_started_at_ns(json).unwrap_or(0);
    let stamp = powerbench_orchestrator::history::date_time_stamp(start);
    let plan_part = powerbench_orchestrator::history::sanitize(&json.plan_guid);
    let mut candidate = dir.join(format!("PowerBench-Session_{plan_part}_{stamp}.html"));
    let mut suffix = 2u32;
    while candidate.exists() {
        candidate = dir.join(format!(
            "PowerBench-Session_{plan_part}_{stamp}_{suffix}.html"
        ));
        suffix += 1;
    }
    std::fs::write(&candidate, &html)
        .map_err(|e| format!("не удалось записать «{}»: {e}", candidate.display()))?;
    if let Err(e) = open_with_default_app(&candidate) {
        return Err(format!(
            "отчёт сохранён ({}), но браузер открыть не удалось: {e}",
            candidate.display()
        ));
    }
    Ok(candidate.display().to_string())
}

fn empty_finished() -> FinishedPayload {
    FinishedPayload {
        ok: false,
        error: None,
        plan_guid: None,
        cancelled: None,
        early_stopped: false,
        result_path: None,
        report_path: None,
        level: None,
        level_label: None,
        recommended_scheme: None,
        recommended_name: None,
        expected_margin_percent: None,
        winner_margin_percent: None,
    }
}

/// Занять раннер под сессию: проверка «уже выполняется» и установка дескриптора
/// под ОДНОЙ блокировкой.
///
/// Регресс C6/H34: эти две операции раньше шли двумя отдельными взятиями
/// блокировки. Между ними было окно, в которое вторая команда `start` успевала
/// пройти ту же проверку и затереть дескриптор первой: сессий становилось две, а
/// отменялась и дожидалась только вторая — первая продолжала мерить и
/// переключать схему питания уже без возможности её остановить.
///
/// Отравление блокировки игнорируется (регресс H38): после паники в любом
/// владельце интерфейс не должен блокироваться навсегда.
fn claim(
    runner: &Arc<Mutex<Option<RunnerHandle>>>,
    handle: RunnerHandle,
) -> Result<(), String> {
    let mut guard = runner.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return Err("сессия уже выполняется".to_string());
    }
    *guard = Some(handle);
    Ok(())
}

/// Снять дескриптор сессии: раннер снова свободен.
///
/// Срабатывает ВСЕГДА, в том числе при отравленной блокировке. Прежний
/// `if let Ok(..)` здесь означал, что после паники в другом потоке дескриптор
/// остаётся в состоянии «идёт» навсегда: интерфейс больше не мог запустить
/// замер, а пользователь не получал ни ошибки, ни объяснения.
fn release(runner: &Arc<Mutex<Option<RunnerHandle>>>) {
    let mut guard = runner.lock().unwrap_or_else(|e| e.into_inner());
    guard.take();
}

/// Запустить сессию в фоновом потоке. Возвращает plan_guid или причину отказа.
pub fn start(
    app: &AppHandle,
    runner: &Arc<Mutex<Option<RunnerHandle>>>,
    plan: SessionConfig,
) -> Result<String, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(None));
    claim(
        runner,
        RunnerHandle {
            cancel: Arc::clone(&cancel),
            join: None,
            status: Arc::clone(&status),
        },
    )?;
    let pid = plan.plan_guid.clone();
    let app2 = app.clone();
    let cancel2 = Arc::clone(&cancel);
    let runner2 = Arc::clone(runner);
    let join = std::thread::spawn(move || {
        let app_err = app2.clone();
        // Паника в сессии не должна вешать интерфейс в «running» навсегда
        // и оставлять чужую схему: гасим в ошибку и освобождаем раннер.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_test(app2, plan, cancel2, status);
        }));
        if let Err(payload) = outcome {
            let _ = app_err.emit(
                "test-finished",
                FinishedPayload {
                    ok: false,
                    error: Some(format!("паника потока сессии: {}", panic_message(&payload))),
                    ..empty_finished()
                },
            );
        }
        release(&runner2);
    });
    // `join` ставится уже существующему дескриптору: он заведомо на месте,
    // иначе сессия была бы уже занята и `claim` вернул бы ошибку выше.
    if let Some(h) = runner.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        h.join = Some(join);
    }
    Ok(pid)
}

/// Запросить остановку текущей сессии. Возвращает true, если она шла.
pub fn stop(runner: &Arc<Mutex<Option<RunnerHandle>>>) -> bool {
    let mut guard = runner.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_mut() {
        Some(h) => {
            h.cancel.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// Идёт ли сессия сейчас.
pub fn running(runner: &Arc<Mutex<Option<RunnerHandle>>>) -> bool {
    runner.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Глобальное состояние приложения не должны видеть внутренности; лог-хелпер
/// для команд интерфейса (событие `log` + персистентный журнал).
pub fn emit_log(app: &AppHandle, level: &str, text: &str) {
    let msg = LogMsg {
        level: level.to_string(),
        text: text.to_string(),
        ts_ms: now_ms(),
    };
    persist_log(app, level, text);
    let _ = app.emit("log", msg);
}

/// Текст паники для журнала (String / &str / неизвестно).
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else {
        "неизвестная причина".to_string()
    }
}

/// Короткое описание текущей сессии для отчёта и шапки.
///
/// `None` — сессии нет. Иначе строка обновляется на каждой фазе, поэтому
/// отчёт, снятый посреди замера, показывает, на чём всё остановилось.
pub fn session_description(runner: &Arc<Mutex<Option<RunnerHandle>>>) -> Option<String> {
    let guard = runner.lock().unwrap_or_else(|e| e.into_inner());
    let handle = guard.as_ref()?;
    let status = handle
        .status
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    Some(status.unwrap_or_else(|| "идёт (фаза неизвестна)".to_string()))
}

/// Короткая форма хеша конфигурации: полный в отчёте бесполезен, а по
/// префиксу видно, совпадает ли он с прошлой сессией.
fn short_hash(hash: &str) -> String {
    if hash.len() > 12 {
        format!("{}…", &hash[..12])
    } else {
        hash.to_string()
    }
}

/// Дождаться завершения фоновой сессии при закрытии окна.
///
/// Сначала просим сессию остановиться, затем ждём поток, но не дольше
/// [`JOIN_TIMEOUT`]. Раньше закрытие окна ждало `join()` без предела: если
/// движок завис (например, поток ждал ресурс), приложение не закрывалось
/// вообще и выглядело зависшим. По истечении времени мы отходим — сессия
/// запишет контрольную точку, и её можно будет продолстить при следующем
/// запуске (ровно для этого точка и существует).
pub fn join(runner: &Arc<Mutex<Option<RunnerHandle>>>) {
    // Отравление игнорируем (регресс H38): иначе после паники в другом потоке
    // закрытие окно не отменило бы сессию и не дождалось её — фоновый поток
    // продолжал бы мерить и переключать схему питания у закрытого приложения.
    let (cancel, handle) = {
        let mut guard = runner.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_mut() {
            Some(h) => (Some(Arc::clone(&h.cancel)), h.join.take()),
            None => (None, None),
        }
    };
    if let Some(c) = cancel {
        c.store(true, Ordering::Relaxed);
    }
    let Some(h) = handle else { return };
    if h.thread().id() == std::thread::current().id() {
        // Вызвано из самого потока сессии: ждать себя нельзя.
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = h.join();
        let _ = tx.send(());
    });
    if rx
        .recv_timeout(Duration::from_millis(JOIN_TIMEOUT_MS))
        .is_err()
    {
        eprintln!(
            "сессия не завершилась за {} с — закрытие приложения продолжается",
            JOIN_TIMEOUT_MS
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// Пустой раннер для проверок занятости.
    fn empty_runner() -> Arc<Mutex<Option<RunnerHandle>>> {
        Arc::new(Mutex::new(None))
    }

    fn handle() -> RunnerHandle {
        RunnerHandle {
            cancel: Arc::new(AtomicBool::new(false)),
            join: None,
            status: Arc::new(Mutex::new(None)),
        }
    }

    /// Регресс C6/H34: вторая попытка занять раннер обязана отказать, и первый
    /// дескриптор должен уцелеть.
    ///
    /// При прежней схеме (проверка и установка двумя взятиями блокировки) окно
    /// между ними позволяло двум `start` пройти проверку, и второй затирал
    /// дескриптор первого: сессий становилось две, а отменялась только вторая.
    #[test]
    fn a_second_claim_is_refused_and_the_first_survives() {
        let runner = empty_runner();
        let first = handle();
        claim(&runner, first).expect("первая сессия должна занять раннер");

        let err = claim(&runner, handle()).expect_err("вторая сессия не должна занять раннер");
        assert!(
            err.contains("уже выполняется"),
            "пользователю нужна внятная причина отказа: {err}"
        );

        // Дескриптор первой сессии на месте и отменяем.
        let guard = runner.lock().unwrap_or_else(|e| e.into_inner());
        let live = guard.as_ref().expect("дескриптор первой сессии затёрт");
        assert!(!live.cancel.load(Ordering::Relaxed));
    }

    /// Флаг занятости снимается, и раннер снова пригоден.
    #[test]
    fn release_frees_the_runner() {
        let runner = empty_runner();
        claim(&runner, handle()).expect("занято");
        assert!(running(&runner), "после claim раннер должен считаться занятым");
        release(&runner);
        assert!(!running(&runner), "после release раннер должен быть свободен");
        // И занять его можно снова.
        claim(&runner, handle()).expect("раннер не освободился");
    }

    /// Регресс H38: отравленная блокировка не должна блокировать интерфейс
    /// навсегда и оставлять сессию в состоянии «идёт».
    ///
    /// Прежний `if let Ok(mut guard) = runner.lock() { guard.take() }` в потоке
    /// сессии просто пропускал снятие флага: после паники в другом потоке
    /// интерфейс вечно показывал «идёт» и не давал запустить новый замер.
    #[test]
    fn a_poisoned_lock_still_lets_the_ui_recover() {
        let runner = empty_runner();
        // Отравляем блокировку паникой в её владельце.
        let poisoned = Arc::clone(&runner);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().expect("первая блокировка обязана удаться");
            panic!("отравляем блокировку");
        })
        .join();
        assert!(
            runner.lock().is_err(),
            "блокировка не отравлена — тест не проверяет то, ради чего написан"
        );

        // Занятость читается, снимается и выдаётся заново.
        assert!(!running(&runner));
        release(&runner);
        claim(&runner, handle()).expect("отравленная блокировка блокирует интерфейс");
        release(&runner);
        // И `stop` тоже работает.
        claim(&runner, handle()).expect("занято");
        assert!(
            stop(&runner),
            "stop должен видеть сессию на отравленной блокировке"
        );
        release(&runner);
        assert!(
            !stop(&runner),
            "после освобождения stop должен сообщать, что сессии нет"
        );
    }

    /// `join` на отравленной блокировке обязан отменить сессию, а не выйти
    /// молча: иначе закрытие окна оставит фоновый замер со схемой питания.
    #[test]
    fn join_cancels_the_session_even_when_the_lock_is_poisoned() {
        let runner = empty_runner();
        claim(&runner, handle()).expect("занято");
        let poisoned = Arc::clone(&runner);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().expect("первая блокировка обязана удаться");
            panic!("отравляем блокировку");
        })
        .join();

        join(&runner);
        // Сессия получила команду отмены (дескриптор `join` был пустым, поэтому
        // поток ждать не пришлось).
        let guard = runner.lock().unwrap_or_else(|e| e.into_inner());
        let live = guard.as_ref().expect("join снял дескриптор");
        assert!(
            live.cancel.load(Ordering::Relaxed),
            "join не отменил сессию: фоновый поток продолжит мерить"
        );
    }

    /// Регресс H34: `identity_info` не должен гонять самопроверку ядра.
    ///
    /// Самопроверка — это сотни тиков в четырёх фазах на всех ядрах. Команда
    /// `async`, то есть выполняется на пуле tokio, и звалась при каждом
    /// открытии страницы «Схемы»: интерфейс намертво занимал процессор. Для
    /// идентичности она ничего не даёт — нужны только метаданные движка.
    #[test]
    fn identity_info_does_not_run_the_core_self_check() {
        let src = include_str!("bridge.rs");
        let start = src
            .find("pub fn identity_info()")
            .expect("не найдена identity_info");
        let body = &src[start..];
        let end = body
            .find("\n#[tauri::command")
            .unwrap_or(body.len());
        let body = &body[..end];
        assert!(
            !body.contains("self_check()"),
            "identity_info снова гоняет самопроверку ядра — она съедает все ядра \
             на пуле tokio"
        );
        assert!(
            body.contains("Engine::new(None)"),
            "идентичность строится из метаданных движка"
        );
    }
}