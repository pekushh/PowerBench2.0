//! Tauri-команды: схемы питания, настройки, контрольная точка, история,
//! управление сессией и системные гарантии.

use std::sync::{Arc, Mutex};

use powerbench_core::engine::Engine;
use powerbench_orchestrator::appsettings::{self, AppSettings};
use powerbench_orchestrator::checkpoint::{load_checkpoint, Checkpoint};
use powerbench_orchestrator::config::{validate_config, SessionConfig};
use powerbench_orchestrator::history::{self, export_csv_to, export_json, list_results, load_result};
use powerbench_orchestrator::result::SessionJson;
use powerbench_windows::powercfg::{self, PowerScheme};

use crate::runner::{self, RunnerHandle};

/// Глобальное состояние приложения.
pub struct AppState {
    pub runner: Arc<Mutex<Option<RunnerHandle>>>,
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
        .map(|s| SchemeRow { guid: s.guid.clone(), name: s.name.clone(), active: s.active })
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
}

fn preset_of(name: &str) -> Option<powerbench_orchestrator::config::Preset> {
    match name {
        "quick" => Some(powerbench_orchestrator::config::QUICK_PRESET),
        "detailed" => Some(powerbench_orchestrator::config::DETAILED_PRESET),
        _ => None,
    }
}

fn build_plan(req: &TestRequestDto) -> Result<SessionConfig, String> {
    if req.resume {
        let cp = load_checkpoint()
            .ok_or_else(|| "нет сохранённой контрольной точки (запустите тест сначала)".to_string())?;
        return Ok(cp.plan);
    }
    let p = match preset_of(&req.preset) {
        Some(p) => p,
        None => powerbench_orchestrator::config::DETAILED_PRESET,
    };
    if req.scheme_ids.is_empty() {
        return Err("выберите хотя бы одну схему питания".to_string());
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
    powercfg::list_schemes().map(|s| scheme_rows(&s)).map_err(|e| e.message)
}

#[tauri::command]
pub fn is_admin() -> bool {
    powerbench_windows::power::is_admin()
}

#[tauri::command]
pub fn ac_power_online() -> Result<bool, String> {
    powerbench_windows::power::ac_power_online().map_err(|e| format!("{e:?}"))
}

/// Действия со схемой питания.
#[tauri::command]
pub fn scheme_action(action: String, guid: Option<String>, path: Option<String>) -> Result<Option<String>, String> {
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
            powercfg::import(std::path::Path::new(&p)).map(Some).map_err(|e| e.message)
        }
        "restore_defaults" => {
            powercfg::restore_defaults().map(|_| None).map_err(|e| e.message)
        }
        other => Err(format!("неизвестное действие «{other}»")),
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct SettingsDto {
    pub duration_seconds: u32,
    pub warmup_seconds: u32,
    pub cooling_seconds: u32,
    pub repetitions: u32,
    pub background_threshold_percent: f64,
    pub theme: String,
    pub mode: String,
    pub reduce_motion: bool,
    pub sidebar_collapsed: bool,
    pub favorite_schemes: Vec<String>,
    pub excluded_schemes: Vec<String>,
}

fn settings_to_dto(s: &AppSettings) -> SettingsDto {
    SettingsDto {
        duration_seconds: s.benchmark.duration_seconds,
        warmup_seconds: s.benchmark.warmup_seconds,
        cooling_seconds: s.benchmark.cooling_seconds,
        repetitions: s.benchmark.repetitions,
        background_threshold_percent: s.benchmark.background_threshold_percent,
        theme: s.appearance.theme.clone(),
        mode: s.appearance.mode.clone(),
        reduce_motion: s.appearance.reduce_motion,
        sidebar_collapsed: s.appearance.sidebar_collapsed,
        favorite_schemes: s.favorite_schemes.clone(),
        excluded_schemes: s.excluded_schemes.clone(),
    }
}

#[tauri::command]
pub fn get_settings() -> SettingsDto {
    settings_to_dto(&AppSettings::load())
}

#[tauri::command]
pub fn set_settings(mut settings: SettingsDto) -> Result<(), String> {
    let bench = powerbench_orchestrator::appsettings::BenchmarkSettings {
        duration_seconds: settings.duration_seconds,
        warmup_seconds: settings.warmup_seconds,
        cooling_seconds: settings.cooling_seconds,
        repetitions: settings.repetitions,
        background_threshold_percent: settings.background_threshold_percent,
    };
    let appearance = powerbench_orchestrator::appsettings::AppearanceSettings {
        theme: std::mem::take(&mut settings.theme),
        mode: std::mem::take(&mut settings.mode),
        reduce_motion: settings.reduce_motion,
        sidebar_collapsed: settings.sidebar_collapsed,
    };
    let s = AppSettings {
        benchmark: bench,
        appearance,
        favorite_schemes: std::mem::take(&mut settings.favorite_schemes),
        excluded_schemes: std::mem::take(&mut settings.excluded_schemes),
    };
    s.save().map_err(|e| format!("не удалось сохранить настройки: {e}"))
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
pub fn start_test(app: tauri::AppHandle, state: tauri::State<'_, AppState>, req: TestRequestDto) -> Result<String, String> {
    let plan = build_plan(&req)?;
    runner::start(&app, &state.runner, plan)
}

#[tauri::command]
pub fn stop_test(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let running = runner::running(&state.runner);
    runner::emit_log(
        &app,
        "info",
        if running { "запрос остановки сессии" } else { "сессия не выполняется" },
    );
    Ok(runner::stop(&state.runner))
}

#[tauri::command]
pub fn test_running(state: tauri::State<'_, AppState>) -> bool {
    runner::running(&state.runner)
}

#[derive(serde::Serialize)]
pub struct HistoryRow {
    pub file_name: String,
    pub plan_guid: String,
    pub started_label: String,
    pub schemes: usize,
    pub level: String,
    pub level_label: String,
    pub readable: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub fn history_list() -> Result<Vec<HistoryRow>, String> {
    let entries = list_results().map_err(|e| format!("не удалось прочитать историю: {e}"))?;
    let mut rows = Vec::new();
    for entry in entries {
        match load_result(&entry.path) {
            Ok(s) => {
                let start = history::session_started_at_ns(&s)
                    .map(history::date_time_stamp)
                    .unwrap_or_default();
                rows.push(HistoryRow {
                    file_name: entry.file_name.clone(),
                    plan_guid: s.plan_guid.clone(),
                    started_label: start,
                    schemes: s.schemes.len(),
                    level: s.recommendation.level.clone(),
                    level_label: s.recommendation.level_label.clone(),
                    readable: true,
                    error: None,
                });
            }
            Err(e) => {
                rows.push(HistoryRow {
                    file_name: entry.file_name.clone(),
                    plan_guid: String::new(),
                    started_label: String::new(),
                    schemes: 0,
                    level: String::new(),
                    level_label: "<не читается>".to_string(),
                    readable: false,
                    error: Some(format!("{e}")),
                });
            }
        }
    }
    Ok(rows)
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
pub fn history_export_to(plan_guid: String, format: String, out_dir: String) -> Result<Vec<String>, String> {
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
            Err(e) => last_error = Some(format!("не удалось экспортировать «{}»: {e}", file.display())),
        }
    }
    if !skipped.is_empty() {
        if last_error.is_none() {
            last_error = Some(format!("некоторые записи пропущены:\n{skipped}"));
        }
    }
    if let Some(e) = last_error {
        if written.is_empty() {
            return Err(e);
        }
        written.push(format!("| {e}"));
    }
    Ok(written)
}

#[tauri::command]
pub fn results_dir() -> String {
    history::results_dir().display().to_string()
}

#[tauri::command]
pub fn appsettings_path() -> String {
    appsettings::appsettings_path().display().to_string()
}