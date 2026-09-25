//! Фоновый запуск сессии из интерфейса: поток-исполнитель, наблюдатель
//! телеметрии (события `log`, `telemetry`, `test-finished`), гарантия ОС
//! (запрет сна, восстановление исходной схемы — внутри orchestator::run_session).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

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
}

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
        }
    }
}

/// Наблюдатель сессии: рассылает события и 10-Гц телеметрию во фронтенд.
struct AppObserver {
    app: AppHandle,
    ctx: Mutex<ObsCtx>,
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
        SessionEvent::RunCompleted { .. }
        | SessionEvent::PhaseFinished { .. }
        | SessionEvent::Restored { .. }
        | SessionEvent::BackgroundClean { .. }
        | SessionEvent::Finished => "success",
        _ => "info",
    }
}

impl TelemetryObserver for AppObserver {
    fn event(&self, e: &SessionEvent) {
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
        let mut ctx = self.ctx.lock().unwrap();
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
    }

    fn tick(&self, snap: &ProgressSnapshot, phase_label: &str, phase_started: &Instant) {
        let ctx = self.ctx.lock().unwrap();
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

fn warn_of(app: &AppHandle, text: &str) {
    let msg = LogMsg {
        level: "warn".into(),
        text: text.to_string(),
        ts_ms: now_ms(),
    };
    persist_log(app, "warn", text);
    let _ = app.emit("log", msg);
}

/// Дублировать запись журнала в персистентный `Logger` (если состояние есть).
fn persist_log(app: &AppHandle, level: &str, text: &str) {
    if let Some(state) = app.try_state::<crate::bridge::AppState>() {
        state.log.append(level, text);
    }
}

/// Тело фонового потока сессии.
fn run_test(app: AppHandle, plan: SessionConfig, cancel: Arc<AtomicBool>) {
    let mut engine = match prepare_engine(plan.worker_count) {
        Ok(e) => e,
        Err(e) => {
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
    let observer = Arc::new(AppObserver {
        app: app.clone(),
        ctx: Mutex::new(ObsCtx::default()),
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
    );
    let saved = match powerbench_orchestrator::history::save_result(&json) {
        Ok(path) => Some(path.display().to_string()),
        Err(e) => {
            warn_of(&app, &format!("не удалось сохранить в историю: {e}"));
            None
        }
    };
    let report_path = match write_session_report(&json) {
        Some(p) => Some(p),
        None => {
            warn_of(&app, "не удалось сформировать HTML-отчёт по сессии");
            None
        }
    };
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

/// Сформировать HTML-отчёт по сессии рядом с результатами истории.
pub fn write_session_report(json: &SessionJson) -> Option<String> {
    let html = powerbench_orchestrator::report::build_session_report(json);
    let dir = powerbench_orchestrator::history::results_dir();
    std::fs::create_dir_all(&dir).ok()?;
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
    std::fs::write(&candidate, html).ok()?;
    Some(candidate.display().to_string())
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

/// Запустить сессию в фоновом потоке. Возвращает plan_guid или причину отказа.
pub fn start(
    app: &AppHandle,
    runner: &Arc<Mutex<Option<RunnerHandle>>>,
    plan: SessionConfig,
) -> Result<String, String> {
    {
        let guard = runner.lock().unwrap();
        if guard.is_some() {
            return Err("сессия уже выполняется".to_string());
        }
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let handle = RunnerHandle {
        cancel: Arc::clone(&cancel),
        join: None,
    };
    {
        let mut guard = runner.lock().unwrap();
        *guard = Some(handle);
    }
    let pid = plan.plan_guid.clone();
    let app2 = app.clone();
    let cancel2 = Arc::clone(&cancel);
    let runner2 = Arc::clone(runner);
    let join = std::thread::spawn(move || {
        let app_err = app2.clone();
        // Паника в сессии не должна вешать интерфейс в «running» навсегда
        // и оставлять чужую схему: гасим в ошибку и освобождаем раннер.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_test(app2, plan, cancel2);
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
        if let Ok(mut guard) = runner2.lock() {
            guard.take();
        }
    });
    if let Some(h) = runner.lock().unwrap().as_mut() {
        h.join = Some(join);
    }
    Ok(pid)
}

/// Запросить остановку текущей сессии. Возвращает true, если она шла.
pub fn stop(runner: &Arc<Mutex<Option<RunnerHandle>>>) -> bool {
    let mut guard = runner.lock().unwrap();
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
    runner.lock().unwrap().is_some()
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

/// Дождаться завершения фоновой сессии (используется при закрытии окна).
pub fn join(runner: &Arc<Mutex<Option<RunnerHandle>>>) {
    let handle = runner
        .lock()
        .ok()
        .and_then(|mut guard| guard.as_mut().and_then(|h| h.join.take()));
    if let Some(h) = handle {
        let _ = h.join();
    }
}
