//! Tauri-команды: схемы питания, настройки, контрольная точка, история,
//! управление сессией и системные гарантии.

use std::sync::{Arc, Mutex};

use powerbench_core::engine::Engine;
use powerbench_orchestrator::appsettings::{self, AppSettings};
use powerbench_orchestrator::checkpoint::{Checkpoint, load_checkpoint};
use powerbench_orchestrator::config::{SessionConfig, validate_config};
use powerbench_orchestrator::history::{
    self, export_csv_to, export_json, list_results, load_result,
};
use powerbench_orchestrator::quarantine::{self, QuarantineEntry};
use powerbench_orchestrator::report::build_report;
use powerbench_orchestrator::result::{SchemeJson, SessionJson};
use powerbench_windows::disk;
use powerbench_windows::power;
use powerbench_windows::powercfg::{self, PowerScheme};

use crate::logger::Logger;
use crate::runner::{self, RunnerHandle};

/// Глобальное состояние приложения.
pub struct AppState {
    pub runner: Arc<Mutex<Option<RunnerHandle>>>,
    pub log: Arc<Logger>,
}

#[derive(serde::Serialize)]
pub struct SchemeRow {
    pub guid: String,
    pub name: String,
    pub active: bool,
}

pub fn scheme_rows(schemes: &[PowerScheme]) -> Vec<SchemeRow> {
    schemes
        .iter()
        .map(|s| SchemeRow {
            guid: s.guid.clone(),
            name: s.name.clone(),
            active: s.active,
        })
        .collect()
}

/// Запрос на запуск сессии из интерфейса.
#[derive(serde::Deserialize)]
pub struct TestRequestDto {
    pub preset: String,
    pub duration_seconds: Option<u64>,
    pub warmup_seconds: Option<u64>,
    pub cooling_seconds: Option<u64>,
    pub repetitions: Option<u32>,
    pub background_threshold_percent: Option<f64>,
    pub worker_count: Option<usize>,
    pub scheme_ids: Vec<String>,
    /// Продолжить текущую контрольную точку (план берётся из неё).
    pub resume: bool,
    /// Сохранять сырые выборки в результат сессии (новый UI).
    /// Поле контракта API: читается внешним фронтендом, бэкенд пока игнорирует.
    #[serde(default)]
    #[allow(dead_code)]
    pub export_raw_samples: bool,
}

/// Имя пресета, как его знает интерфейс. `custom` — параметры заданы вручную,
/// пресет при этом только база для незаданных полей.
const PRESET_CUSTOM: &str = "custom";

fn preset_of(name: &str) -> Option<powerbench_orchestrator::config::Preset> {
    match name {
        "quick" => Some(powerbench_orchestrator::config::QUICK_PRESET),
        "detailed" => Some(powerbench_orchestrator::config::DETAILED_PRESET),
        _ => None,
    }
}

/// Базовые значения для незаданных полей.
///
/// Для `custom` это `DETAILED_PRESET`, но интерфейс всегда присылает все четыре
/// параметра явно, поэтому база фактически не влияет на результат. Раньше
/// неизвестное имя молча трактовалось как «детальный режим», из-за чего в
/// истории сессии мог сохраниться чужой пресет.
fn preset_base(name: &str) -> powerbench_orchestrator::config::Preset {
    preset_of(name).unwrap_or(powerbench_orchestrator::config::DETAILED_PRESET)
}

fn build_plan(req: &TestRequestDto) -> Result<SessionConfig, String> {
    if req.resume {
        let cp = load_checkpoint().ok_or_else(|| {
            "нет сохранённой контрольной точки (запустите тест сначала)".to_string()
        })?;
        return Ok(cp.plan);
    }
    let p = preset_base(&req.preset);
    if req.scheme_ids.is_empty() {
        return Err("выберите хотя бы одну схему питания".to_string());
    }
    // `custom` без единого заданного параметра — это опечатка, а не выбор
    // режима: сообщаем, вместо того чтобы молча запустить детальный режим.
    if req.preset == PRESET_CUSTOM
        && req.duration_seconds.is_none()
        && req.warmup_seconds.is_none()
        && req.cooling_seconds.is_none()
        && req.repetitions.is_none()
    {
        return Err(
            "выбран пользовательский набор, но ни один параметр не задан — проверьте настройки теста"
                .to_string(),
        );
    }
    let plan = SessionConfig {
        duration_seconds: req.duration_seconds.unwrap_or(p.duration_seconds),
        warmup_seconds: req.warmup_seconds.unwrap_or(p.warmup_seconds),
        cooling_seconds: req.cooling_seconds.unwrap_or(p.cooling_seconds),
        repetitions: req.repetitions.unwrap_or(p.repetitions),
        background_threshold_percent: req
            .background_threshold_percent
            .unwrap_or(powerbench_orchestrator::config::DEFAULT_BACKGROUND_PERCENT),
        worker_count: req.worker_count,
        scheme_ids: req.scheme_ids.clone(),
        plan_guid: crate::new_plan_guid(),
    };
    match validate_config(&plan) {
        Some(reason) => Err(reason),
        None => Ok(plan),
    }
}

// --- Команды ---

#[tauri::command]
pub fn list_schemes() -> Result<Vec<SchemeRow>, String> {
    powercfg::list_schemes()
        .map(|s| scheme_rows(&s))
        .map_err(|e| e.message)
}

#[tauri::command]
pub fn is_admin() -> bool {
    power::is_admin()
}

#[tauri::command]
pub fn ac_power_online() -> Result<bool, String> {
    power::ac_power_online().map_err(|e| format!("{e:?}"))
}

/// Действия со схемой питания.
#[tauri::command]
pub fn scheme_action(
    action: String,
    guid: Option<String>,
    path: Option<String>,
) -> Result<Option<String>, String> {
    match action.as_str() {
        "activate" => {
            let g = guid.ok_or_else(|| "activate требует guid".to_string())?;
            powercfg::activate(&g).map(|_| None).map_err(|e| e.message)
        }
        "duplicate" => {
            let g = guid.ok_or_else(|| "duplicate требует guid".to_string())?;
            powercfg::duplicate(&g).map(Some).map_err(|e| e.message)
        }
        "delete" => {
            let g = guid.ok_or_else(|| "delete требует guid".to_string())?;
            powercfg::delete(&g).map(|_| None).map_err(|e| e.message)
        }
        "import" => {
            let p = path.ok_or_else(|| "import требует путь к .pow".to_string())?;
            powercfg::import(std::path::Path::new(&p))
                .map(Some)
                .map_err(|e| e.message)
        }
        "restore_defaults" => powercfg::restore_defaults()
            .map(|_| None)
            .map_err(|e| e.message),
        other => Err(format!("неизвестное действие «{other}»")),
    }
}

/// Плоский DTO настроек для интерфейса (этап 7).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SettingsDto {
    // Длительность/разогрев/охлаждение/повторы убраны: это параметры пресета
    // режима (`test_presets`), а не настройка. Раньше они жили в DTO, и
    // интерфейс подставлял их в мастер вместо пресета — карточка режима
    // обещала одни числа, а запуск шёл с другими.
    pub background_threshold_percent: f64,
    pub theme: String,
    pub mode: String,
    pub reduce_motion: bool,
    pub sidebar_collapsed: bool,
    pub score_performance: f64,
    pub score_stability: f64,
    pub score_worst_second: f64,
    pub max_sessions: u32,
    pub favorite_schemes: Vec<String>,
    pub excluded_schemes: Vec<String>,
}

fn settings_to_dto(s: &AppSettings) -> SettingsDto {
    SettingsDto {
        theme: s.appearance.theme.clone(),
        mode: s.appearance.mode.clone(),
        reduce_motion: s.appearance.reduce_motion,
        sidebar_collapsed: s.appearance.sidebar_collapsed,
        score_performance: s.scoring.performance,
        score_stability: s.scoring.stability,
        score_worst_second: s.scoring.worst_second,
        max_sessions: s.retention.max_sessions,
        favorite_schemes: s.favorite_schemes.clone(),
        excluded_schemes: s.excluded_schemes.clone(),
        background_threshold_percent: s.benchmark.background_threshold_percent,
    }
}

#[tauri::command]
pub fn get_settings() -> SettingsDto {
    settings_to_dto(&AppSettings::load())
}

/// Записать настройки из интерфейса.
///
/// Правка идёт **под блокировкой файла** и поверх актуального содержимого:
/// команды Tauri выполняются на пуле потоков, поэтому «прочитать DTO → дописать
/// своё → записать» без блокировки теряло параллельные изменения (например,
/// отметку «избранное» для схемы, поставленную другим вызовом).
#[tauri::command]
pub fn set_settings(settings: SettingsDto) -> Result<(), String> {
    let mut s = settings;
    // Значения приходят из интерфейса, поэтому проверяем их до записи: ноль в
    // длительности или NaN в весах уронили бы планировщик или нормализацию
    // весов уже во время сессии, а пользователь увидел бы это слишком поздно.
    if !(s.background_threshold_percent.is_finite() && s.background_threshold_percent >= 0.0) {
        return Err("порог фоновой нагрузки должен быть неотрицательным числом".to_string());
    }
    let weights_ok = s.score_performance.is_finite()
        && s.score_stability.is_finite()
        && s.score_worst_second.is_finite()
        && s.score_performance >= 0.0
        && s.score_stability >= 0.0
        && s.score_worst_second >= 0.0;
    if !weights_ok {
        return Err("веса оценки должны быть неотрицательными числами".to_string());
    }
    AppSettings::update_locked(|cur: &mut AppSettings| {
        cur.benchmark.background_threshold_percent = s.background_threshold_percent;
        cur.appearance.theme = std::mem::take(&mut s.theme);
        cur.appearance.mode = std::mem::take(&mut s.mode);
        cur.appearance.reduce_motion = s.reduce_motion;
        cur.appearance.sidebar_collapsed = s.sidebar_collapsed;
        cur.scoring.performance = s.score_performance;
        cur.scoring.stability = s.score_stability;
        cur.scoring.worst_second = s.score_worst_second;
        cur.retention.max_sessions = s.max_sessions;
        cur.favorite_schemes = std::mem::take(&mut s.favorite_schemes);
        cur.excluded_schemes = std::mem::take(&mut s.excluded_schemes);
    })
    .map_err(|e| format!("не удалось сохранить настройки: {e}"))
}

#[derive(serde::Serialize)]
pub struct CheckpointDto {
    pub plan_guid: String,
    pub scheme_ids: Vec<String>,
    pub repetitions: u32,
    pub duration_seconds: u64,
    pub completed_keys: Vec<String>,
    pub original_scheme_guid: Option<String>,
    pub original_restored: bool,
    pub has_runs: bool,
}

impl CheckpointDto {
    fn from_cp(cp: &Checkpoint) -> Self {
        Self {
            plan_guid: cp.plan.plan_guid.clone(),
            scheme_ids: cp.plan.scheme_ids.clone(),
            repetitions: cp.plan.repetitions,
            duration_seconds: cp.plan.duration_seconds,
            completed_keys: cp.completed_keys.clone(),
            original_scheme_guid: cp.original_scheme_guid.clone(),
            original_restored: cp.original_restored,
            has_runs: !cp.runs.is_empty(),
        }
    }
}

#[tauri::command]
pub fn checkpoint_status() -> Option<CheckpointDto> {
    load_checkpoint().map(|cp| CheckpointDto::from_cp(&cp))
}

#[tauri::command]
pub fn identity_info() -> Result<serde_json::Value, String> {
    let mut engine = Engine::new(None);
    engine
        .self_check()
        .map_err(|e| format!("самопроверка ядра не прошла: {e:?}"))?;
    serde_json::to_value(runner::identity_of(&engine)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn start_test(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    req: TestRequestDto,
) -> Result<String, String> {
    // Сначала план: если он невалиден, точку прошлой сессии трогать нельзя —
    // иначе пользователь потеряет возможность продолжить её.
    let plan = build_plan(&req)?;
    // Запуск второй сессии недопустим: восстановление ниже переключило бы
    // схему питания посреди измерений.
    if runner::running(&state.runner) {
        return Err("сессия уже выполняется — сначала остановите её".to_string());
    }
    // Новый запуск обязан выбросить точку прошлого плана, иначе run_session
    // отвергнет план как «чужой». Исходная схема при этом возвращается.
    if !req.resume {
        discard_stale_checkpoint(&app, &state);
    }
    state
        .log
        .append("info", &format!("запуск сессии: план {}", plan.plan_guid));
    runner::emit_log(
        &app,
        "info",
        &format!("запуск сессии: план {}", plan.plan_guid),
    );
    runner::start(&app, &state.runner, plan)
}

/// Убрать контрольную точку чужого плана, вернув исходную схему питания.
/// Точка удаляется всегда (иначе новый план не запустится), но схема
/// восстанавливается гарантированно — это требование безопасности ОС.
fn discard_stale_checkpoint(app: &tauri::AppHandle, state: &AppState) {
    use powerbench_orchestrator::checkpoint::{clear_checkpoint, load_checkpoint};
    use powerbench_orchestrator::recovery::recover_interrupted_session;
    use powerbench_orchestrator::session::RealSchemeDriver;

    let Some(cp) = load_checkpoint() else {
        return;
    };
    // Восстановление исходной схемы (если её не вернули) — до удаления точки:
    // после удаления теряется original_scheme_guid.
    if !cp.original_restored {
        let outcome = recover_interrupted_session(&RealSchemeDriver);
        if let Some(cause) = outcome.error {
            runner::emit_log(
                app,
                "warn",
                &format!("при сбросе прошлой сессии: {cause}"),
            );
        }
    }
    match clear_checkpoint() {
        Ok(()) => {
            let text = "прошлая сессия сброшена: контрольная точка очищена";
            state.log.append("info", text);
            runner::emit_log(app, "info", text);
        }
        Err(e) => {
            let text = format!("не удалось очистить контрольную точку: {e}");
            state.log.append("warn", &text);
            runner::emit_log(app, "warn", &text);
        }
    }
}

/// Забыть прерванную сессию: вернуть исходную схему и удалить точку.
#[tauri::command]
pub fn checkpoint_discard(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    use powerbench_orchestrator::checkpoint::clear_checkpoint;
    use powerbench_orchestrator::recovery::recover_interrupted_session;
    use powerbench_orchestrator::session::RealSchemeDriver;

    let outcome = recover_interrupted_session(&RealSchemeDriver);
    if let Some(cause) = outcome.error.as_deref() {
        // Замечание не теряем: оно объясняет, почему схема могла остаться.
        let text = format!("точка удалена, но есть замечание: {cause}");
        state.log.append("warn", &text);
        runner::emit_log(&app, "warn", &text);
        return Err(text);
    }
    clear_checkpoint().map_err(|e| format!("не удалось удалить контрольную точку: {e}"))?;
    state.log.append("info", "прерванная сессия забыта");
    runner::emit_log(&app, "info", "прерванная сессия забыта");
    Ok(())
}

#[tauri::command]
pub fn stop_test(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let running = runner::running(&state.runner);
    let level = if running { "info" } else { "warn" };
    let text = if running {
        "запрос остановки сессии"
    } else {
        "сессия не выполняется"
    };
    state.log.append(level, text);
    runner::emit_log(&app, level, text);
    Ok(runner::stop(&state.runner))
}

#[tauri::command]
pub fn test_running(state: tauri::State<'_, AppState>) -> bool {
    runner::running(&state.runner)
}

/// Одна точка графика сравнения (схема в сессии).
#[derive(serde::Serialize)]
pub struct SeriesPoint {
    pub scheme_id: String,
    pub scheme_name: String,
    pub score: Option<f64>,
    pub margin: Option<f64>,
}

#[derive(serde::Serialize)]
pub struct HistoryRow {
    pub file_name: String,
    pub plan_guid: String,
    pub started_label: String,
    pub started_at_ns: u64,
    pub schemes: usize,
    pub level: String,
    pub level_label: String,
    pub readable: bool,
    pub error: Option<String>,
    pub scheme_name: String,
    /// Медианный throughput лидера сессии, тик/с (НЕ балл).
    pub throughput: Option<f64>,
    /// Балл лидера 0..=100 по весам из настроек (настоящая метрика).
    pub score: Option<f64>,
    pub margin: Option<f64>,
    pub stability: Option<f64>,
    pub early_stopped: bool,
    pub rounds_planned: u32,
    pub rounds_completed: u32,
    pub series: Vec<SeriesPoint>,
}

/// Лучшая схема сессии для лида строки: рекомендованная, иначе лучшая
/// среди допущенных, иначе первая из схем.
fn best_scheme(s: &SessionJson) -> Option<&SchemeJson> {
    // MSRV 1.85: схлопывание через let-цепочки требует Rust 1.88+.
    #[allow(clippy::collapsible_if)]
    if let Some(g) = s.recommendation.recommended_scheme.as_deref() {
        if let Some(sch) = s
            .schemes
            .iter()
            .find(|x| x.scheme_id.eq_ignore_ascii_case(g))
        {
            return Some(sch);
        }
    }
    let mut best: Option<&SchemeJson> = None;
    let mut best_v = f64::MIN;
    for sch in &s.schemes {
        if sch.rejected {
            continue;
        }
        let v = if sch.median_throughput.is_finite() {
            sch.median_throughput
        } else {
            f64::MIN
        };
        if v > best_v {
            best_v = v;
            best = Some(sch);
        }
    }
    best.or_else(|| s.schemes.first())
}

fn history_row(entry: &history::HistoryEntry, s: &SessionJson) -> HistoryRow {
    let start = history::session_started_at_ns(s).unwrap_or(0);
    let best = best_scheme(s);
    let throughput = best
        .filter(|b| b.median_throughput.is_finite() && b.median_throughput > 0.0)
        .map(|b| b.median_throughput);
    // Настоящий балл 0..=100 по весам из настроек: `throughput` (тик/с)
    // для сравнения между схемами, `score` — для сравнения сессий.
    let settings = AppSettings::load();
    let weights = powerbench_orchestrator::score::ScoreWeights {
        performance: settings.scoring.performance,
        stability: settings.scoring.stability,
        worst_second: settings.scoring.worst_second,
    };
    let score = powerbench_orchestrator::score::score_leader(
        &powerbench_orchestrator::score::score_schemes(&s.schemes, &weights),
    )
    .map(|l| l.score)
    .filter(|v| v.is_finite());
    let margin = best.filter(|b| b.margin.is_finite()).map(|b| b.margin);
    let stability = best
        .filter(|b| b.median_consistency_percent.is_finite())
        .map(|b| b.median_consistency_percent);
    let scheme_name = best
        .map(|b| b.name.clone().unwrap_or_else(|| b.scheme_id.clone()))
        .unwrap_or_else(|| "-".to_string());
    let early_stopped = s.early_stop_reason.is_some()
        || (s.rounds_planned > 0 && s.rounds_completed < s.rounds_planned);
    let series = s
        .schemes
        .iter()
        .filter(|x| !x.rejected && x.median_throughput.is_finite() && x.median_throughput > 0.0)
        .map(|x| SeriesPoint {
            scheme_id: x.scheme_id.clone(),
            scheme_name: x.name.clone().unwrap_or_else(|| x.scheme_id.clone()),
            score: Some(x.median_throughput),
            margin: x.margin.is_finite().then_some(x.margin),
        })
        .collect();
    HistoryRow {
        file_name: entry.file_name.clone(),
        plan_guid: s.plan_guid.clone(),
        started_label: history::date_time_stamp(start),
        started_at_ns: start,
        schemes: s.schemes.len(),
        level: s.recommendation.level.clone(),
        level_label: s.recommendation.level_label.clone(),
        readable: true,
        error: None,
        scheme_name,
        throughput,
        score,
        margin,
        stability,
        early_stopped,
        rounds_planned: s.rounds_planned,
        rounds_completed: s.rounds_completed,
        series,
    }
}

fn handled_history_rows() -> Result<Vec<HistoryRow>, String> {
    let entries = list_results().map_err(|e| format!("не удалось прочитать историю: {e}"))?;
    let mut rows = Vec::new();
    for entry in entries {
        match load_result(&entry.path) {
            Ok(s) => rows.push(history_row(&entry, &s)),
            Err(e) => rows.push(HistoryRow {
                file_name: entry.file_name.clone(),
                plan_guid: String::new(),
                started_label: String::new(),
                started_at_ns: 0,
                schemes: 0,
                level: String::new(),
                level_label: String::new(),
                readable: false,
                error: Some(e.to_string()),
                scheme_name: String::new(),
                throughput: None,
                score: None,
                margin: None,
                stability: None,
                early_stopped: false,
                rounds_planned: 0,
                rounds_completed: 0,
                series: Vec::new(),
            }),
        }
    }
    Ok(rows)
}

#[tauri::command]
pub fn history_list() -> Result<Vec<HistoryRow>, String> {
    handled_history_rows()
}

#[tauri::command]
pub fn history_open(plan_guid: String) -> Result<SessionJson, String> {
    let entries = list_results().map_err(|e| format!("не удалось прочитать историю: {e}"))?;
    let mut last_error = String::new();
    let mut found = false;
    for entry in entries {
        let s = match load_result(&entry.path) {
            Ok(s) => s,
            Err(e) => {
                last_error = format!("«{}» не читается: {e}", entry.file_name);
                continue;
            }
        };
        if s.plan_guid.eq_ignore_ascii_case(&plan_guid) {
            return Ok(s);
        }
        found = true;
    }
    if found {
        Err(format!("запись {plan_guid} не найдена"))
    } else {
        Err(if last_error.is_empty() {
            format!("запись {plan_guid} не найдена")
        } else {
            last_error
        })
    }
}

/// Экспорт одной записи или всей истории в выбранный пользователем каталог.
/// Возвращает пути записанных файлов.
#[tauri::command]
pub fn history_export_to(
    plan_guid: String,
    format: String,
    out_dir: String,
) -> Result<Vec<String>, String> {
    let format = if format == "csv" { "csv" } else { "json" };
    if format == "json" || format == "csv" {
        // допустимо
    } else {
        return Err(format!("неизвестный формат «{format}»"));
    }
    let dir = std::path::PathBuf::from(&out_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("не удалось создать «{}»: {e}", dir.display()))?;
    let entries = list_results().map_err(|e| format!("не удалось прочитать историю: {e}"))?;
    let mut written = Vec::new();
    let mut skipped = String::new();
    let mut last_error: Option<String> = None;
    for entry in entries {
        let s = match load_result(&entry.path) {
            Ok(s) => s,
            Err(e) => {
                skipped.push_str(&format!("  «{}» не читается: {e}\n", entry.file_name));
                continue;
            }
        };
        if plan_guid != "all" && !s.plan_guid.eq_ignore_ascii_case(&plan_guid) {
            continue;
        }
        let file = dir.join(format!("{}.{format}", s.plan_guid));
        let r: Result<(), String> = if format == "json" {
            export_json(&s, &file).map_err(|e| e.to_string())
        } else {
            export_csv_to(&s, &file).map(|_| ())
        };
        match r {
            Ok(()) => written.push(file.display().to_string()),
            Err(e) => {
                last_error = Some(format!(
                    "не удалось экспортировать «{}»: {e}",
                    file.display()
                ))
            }
        }
    }
    if !skipped.is_empty() && last_error.is_none() {
        last_error = Some(format!("некоторые записи пропущены:\n{skipped}"));
    }
    // MSRV 1.85: схлопывание через let-цепочки требует Rust 1.88+.
    #[allow(clippy::collapsible_if)]
    if let Some(e) = last_error {
        if written.is_empty() {
            return Err(e);
        }
        written.push(format!("| {e}"));
    }
    Ok(written)
}

/// Сводный HTML-отчёт по всей истории в выбранный каталог.
#[tauri::command]
pub fn history_report(out_dir: String) -> Result<String, String> {
    let dir = std::path::PathBuf::from(&out_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("не удалось создать «{}»: {e}", dir.display()))?;
    let entries = list_results().map_err(|e| format!("не удалось прочитать историю: {e}"))?;
    let mut sessions: Vec<SessionJson> = Vec::new();
    for entry in entries {
        if let Ok(s) = load_result(&entry.path) {
            sessions.push(s);
        }
    }
    let html = build_report(&sessions);
    let stamp = history::date_time_stamp(now_unix_ns());
    let name = format!("PowerBench-Report-{stamp}.html");
    let path = unique_path(&dir, &name);
    std::fs::write(&path, html)
        .map_err(|e| format!("не удалось записать отчёт «{}»: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Список схем в карантине (браковка: зависания, нестабильность, деградация).
#[tauri::command]
pub fn quarantine_list() -> Vec<QuarantineEntry> {
    quarantine::load_quarantine()
}

/// Оценка длительности сессии: и готовая строка, и сырые секунды.
///
/// Считает на бэкенде из тех же констант, что использует планировщик, поэтому
/// цифра в интерфейсе не расходится с фактом (включая паузу после схемы,
/// стабилизации, проверку фона и охлаждение). Сырые секунды нужны, чтобы
/// интерфейс считал «осталось» без разбора отформатированной строки.
#[derive(serde::Serialize)]
pub struct SessionEstimate {
    pub seconds: f64,
    pub label: String,
}

#[tauri::command]
pub fn estimate_session(
    duration_seconds: u64,
    warmup_seconds: u64,
    cooling_seconds: u64,
    repetitions: u32,
    scheme_count: usize,
) -> SessionEstimate {
    let seconds = powerbench_orchestrator::config::estimate_run_seconds(
        duration_seconds,
        warmup_seconds,
        cooling_seconds,
        repetitions,
        scheme_count,
    );
    SessionEstimate {
        seconds,
        label: powerbench_orchestrator::config::format_estimate(seconds),
    }
}

/// Параметры режима теста для интерфейса (зеркало `config::Preset`).
///
/// Нужны, потому что карточки режимов обещают конкретные «повторы» и «точность»,
/// и мастер применяет именно эти значения. Раньше интерфейс держал свою копию
/// чисел: правка пресета в Rust тихо расходилась с тем, что показывала и
/// запускала карточка, и расхождение замечали уже по отчёту.
#[derive(serde::Serialize)]
pub struct PresetDto {
    /// Ключ режима: `quick` или `detailed`.
    pub key: String,
    pub duration_seconds: u64,
    pub warmup_seconds: u64,
    pub cooling_seconds: u64,
    pub repetitions: u32,
}

/// Значения обоих пресетов одним вызовом.
#[tauri::command]
pub fn test_presets() -> Vec<PresetDto> {
    use powerbench_orchestrator::config::{DETAILED_PRESET, Preset, QUICK_PRESET};
    let one = |key: &str, p: Preset| PresetDto {
        key: key.to_string(),
        duration_seconds: p.duration_seconds,
        warmup_seconds: p.warmup_seconds,
        cooling_seconds: p.cooling_seconds,
        repetitions: p.repetitions,
    };
    vec![one("quick", QUICK_PRESET), one("detailed", DETAILED_PRESET)]
}

/// Вернуть схему из карантина в бенчмарк. `Ok(true)` — запись была и удалена.
#[tauri::command]
pub fn quarantine_clear(scheme_id: String) -> Result<bool, String> {
    quarantine::quarantine_remove(&scheme_id)
        .map_err(|e| format!("не удалось убрать из карантина: {e}"))
}

/// HTML-отчёт по одной сессии (сохраняется рядом с результатами и
/// открывается в браузере по умолчанию).
#[tauri::command]
pub fn session_report(plan_guid: String) -> Result<String, String> {
    let s = find_session(&plan_guid)?;
    crate::runner::write_session_report_strict(&s)
}

/// Удалить запись истории по имени файла.
#[tauri::command]
pub fn history_delete(file_name: String) -> Result<(), String> {
    if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
        return Err("недопустимое имя файла".to_string());
    }
    let dir = history::results_dir();
    let target = dir.join(&file_name);
    let canon_dir = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    let canon = target
        .canonicalize()
        .map_err(|e| format!("не удалось найти «{file_name}»: {e}"))?;
    if !canon.starts_with(&canon_dir) {
        return Err("недопустимое имя файла".to_string());
    }
    std::fs::remove_file(&canon).map_err(|e| format!("не удалось удалить «{file_name}»: {e}"))
}

/// Сведения о доступном месте и размере истории.
#[derive(serde::Serialize)]
pub struct StorageStats {
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub history_bytes: u64,
    pub max_sessions: u32,
}

#[tauri::command]
pub fn storage_stats() -> StorageStats {
    let dir = history::results_dir();
    let free_bytes = disk::free_space_bytes(&dir).unwrap_or(0);
    let total_bytes = disk::volume_bytes(&dir).unwrap_or(0);
    let history_bytes = dir_size(&dir);
    let max_sessions = AppSettings::load().retention.max_sessions;
    StorageStats {
        free_bytes,
        total_bytes,
        history_bytes,
        max_sessions,
    }
}

#[tauri::command]
pub fn history_open_folder() -> Result<(), String> {
    let dir = history::results_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("не удалось создать «{}»: {e}", dir.display()))?;
    open_in_explorer(&dir.display().to_string())
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.is_dir() {
        return Err(format!("каталог не существует: {path}"));
    }
    open_in_explorer(&path)
}

/// Открыть файл приложением по умолчанию (HTML — в браузере).
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("файл не существует: {path}"));
    }
    crate::runner::open_with_default_app(&p)
}

#[tauri::command]
pub fn results_dir() -> String {
    history::results_dir().display().to_string()
}

#[tauri::command]
pub fn appsettings_path() -> String {
    appsettings::appsettings_path().display().to_string()
}

/// Журнал событий приложения (команда `log_history`).
#[tauri::command]
pub fn log_history(state: tauri::State<'_, AppState>) -> Vec<crate::logger::LogEntry> {
    state.log.snapshot()
}

/// Команда «сохранить журнал сейчас».
///
/// Фоновая запись и так устроена, что свежие строки доезжают до диска за
/// секунду, но перед выходом из приложения и перед снятием отчёта полезно
/// дождаться файла явно — так лог не зависит от тайминга.
#[tauri::command]
pub fn log_flush(state: tauri::State<'_, AppState>) {
    state.log.flush();
}

/// Готовность системы к запуску теста.
#[derive(serde::Serialize)]
pub struct Readiness {
    pub ok: bool,
    pub issues: Vec<String>,
}

#[tauri::command]
pub fn system_ready(requested_schemes: Option<u32>) -> Readiness {
    // Единая реализация — в orchestrator::diagnostics (там же юнит-тесты).
    let report = powerbench_orchestrator::diagnostics::system_ready(
        &powerbench_orchestrator::session::RealSchemeDriver,
        requested_schemes.unwrap_or(2).max(1) as usize,
    );
    Readiness {
        ok: report.ok,
        issues: report.issues,
    }
}

// --- Вспомогательные ---

fn find_session(plan_guid: &str) -> Result<SessionJson, String> {
    let entries = list_results().map_err(|e| format!("не удалось прочитать историю: {e}"))?;
    for entry in entries {
        // MSRV 1.85: схлопывание через let-цепочки требует Rust 1.88+.
        #[allow(clippy::collapsible_if)]
        if let Ok(s) = load_result(&entry.path) {
            if s.plan_guid.eq_ignore_ascii_case(plan_guid) {
                return Ok(s);
            }
        }
    }
    Err(format!("запись {plan_guid} не найдена"))
}

/// Путь, которого ещё нет в каталоге (добавляет `_2`, `_3`, …).
fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let mut candidate = dir.join(name);
    let mut suffix = 2u32;
    while candidate.exists() {
        let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
        let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        candidate = dir.join(format!("{stem}_{suffix}.{ext}"));
        suffix += 1;
    }
    candidate
}

/// Суммарный размер каталога (байты).
fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            let p = e.path();
            match e.metadata() {
                Ok(md) if md.is_dir() => total += dir_size(&p),
                Ok(md) => total += md.len(),
                _ => {}
            }
        }
    }
    total
}

fn open_in_explorer(path: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new("explorer");
    cmd.arg(path);
    runner::hide_console(&mut cmd);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("не удалось открыть «{path}»: {e}"))
}

fn now_unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
