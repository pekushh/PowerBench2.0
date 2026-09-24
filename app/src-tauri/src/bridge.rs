//! Tauri-команды: схемы питания, настройки, контрольная точка, история,
//! управление сессией и системные гарантии.

use std::sync::{Arc, Mutex};

use powerbench_core::engine::Engine;
use powerbench_orchestrator::appsettings::{self, AppSettings};
use powerbench_orchestrator::checkpoint::{load_checkpoint, Checkpoint};
use powerbench_orchestrator::config::{validate_config, SessionConfig};
use powerbench_orchestrator::history::{self, export_csv_to, export_json, list_results, load_result};
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
    /// Сохранять сырые выборки в результат сессии (новый UI).
    #[serde(default)]
    pub export_raw_samples: bool,
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
    power::is_admin()
}

#[tauri::command]
pub fn ac_power_online() -> Result<bool, String> {
    power::ac_power_online().map_err(|e| format!("{e:?}"))
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

/// Плоский DTO настроек для интерфейса (этап 7).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SettingsDto {
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
    pub background_threshold_percent: f64,
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

#[tauri::command]
pub fn set_settings(mut settings: SettingsDto) -> Result<(), String> {
    let mut s = AppSettings::load();
    s.appearance.theme = std::mem::take(&mut settings.theme);
    s.appearance.mode = std::mem::take(&mut settings.mode);
    s.appearance.reduce_motion = settings.reduce_motion;
    s.appearance.sidebar_collapsed = settings.sidebar_collapsed;
    s.scoring.performance = settings.score_performance;
    s.scoring.stability = settings.score_stability;
    s.scoring.worst_second = settings.score_worst_second;
    s.retention.max_sessions = settings.max_sessions;
    s.favorite_schemes = std::mem::take(&mut settings.favorite_schemes);
    s.excluded_schemes = std::mem::take(&mut settings.excluded_schemes);
    s.benchmark.background_threshold_percent = settings.background_threshold_percent;
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
    state.log.append("info", &format!("запуск сессии: план {}", plan.plan_guid));
    runner::emit_log(&app, "info", &format!("запуск сессии: план {}", plan.plan_guid));
    runner::start(&app, &state.runner, plan)
}

#[tauri::command]
pub fn stop_test(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let running = runner::running(&state.runner);
    let level = if running { "info" } else { "warn" };
    let text = if running { "запрос остановки сессии" } else { "сессия не выполняется" };
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
    if let Some(g) = s.recommendation.recommended_scheme.as_deref() {
        if let Some(sch) = s.schemes.iter().find(|x| x.scheme_id.eq_ignore_ascii_case(g)) {
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
    let score = best
        .filter(|b| b.median_throughput.is_finite() && b.median_throughput > 0.0)
        .map(|b| b.median_throughput);
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
                error: Some(format!("{e}")),
                scheme_name: String::new(),
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

/// HTML-отчёт по одной сессии (сохраняется рядом с результатами).
#[tauri::command]
pub fn session_report(plan_guid: String) -> Result<String, String> {
    let s = find_session(&plan_guid)?;
    crate::runner::write_session_report(&s)
        .ok_or_else(|| "не удалось записать отчёт по сессии".to_string())
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
    std::fs::remove_file(&canon)
        .map_err(|e| format!("не удалось удалить «{file_name}»: {e}"))
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

#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("файл не существует: {path}"));
    }
    open_in_explorer(&path)
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

/// Готовность системы к запуску теста.
#[derive(serde::Serialize)]
pub struct Readiness {
    pub ok: bool,
    pub issues: Vec<String>,
}

#[tauri::command]
pub fn system_ready(requested_schemes: Option<u32>) -> Readiness {
    let requested = requested_schemes.unwrap_or(2).max(1);
    let mut issues: Vec<String> = Vec::new();

    match powercfg::list_schemes() {
        Ok(s) => {
            let available: Vec<&PowerScheme> = s.iter().filter(|x| !x.name.is_empty()).collect();
            if available.len() < requested as usize {
                issues.push(format!(
                    "доступно только {} схем питания, а для теста нужно не менее {}",
                    available.len(),
                    requested
                ));
            } else if available.is_empty() {
                issues.push("не найдено ни одной схемы питания".to_string());
            }
        }
        Err(e) => issues.push(format!("не удалось получить список схем: {}", e.message)),
    }

    match power::ac_power_online() {
        Ok(true) => {}
        Ok(false) => issues.push("ноутбук работает от аккумулятора — тест требует сети 220 В".to_string()),
        Err(e) => issues.push(format!("не удалось проверить питание: {e:?}")),
    }

    let dir = history::results_dir();
    match disk::free_space_bytes(&dir) {
        Ok(free) if free < 250 * 1024 * 1024 => {
            issues.push(format!(
                "мало свободного места на диске: {}",
                disk::format_bytes(free)
            ));
        }
        Ok(_) => {}
        Err(e) => issues.push(format!("не удалось проверить диск: {e}")),
    }

    if !power::is_admin() {
        issues.push("запустите PowerBench от имени администратора для переключения схем".to_string());
    }

    Readiness { ok: issues.is_empty(), issues }
}

// --- Вспомогательные ---

fn find_session(plan_guid: &str) -> Result<SessionJson, String> {
    let entries = list_results().map_err(|e| format!("не удалось прочитать историю: {e}"))?;
    for entry in entries {
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
    std::process::Command::new("explorer")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("не удалось открыть «{path}»: {e}"))
}

fn now_unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}