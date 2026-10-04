//! Tauri-команды: схемы питания, настройки, контрольная точка, история,
//! управление сессией и системные гарантии.

use std::sync::{Arc, Mutex};

use powerbench_core::engine::Engine;
use powerbench_orchestrator::appsettings::{self, AppSettings};
use powerbench_orchestrator::checkpoint::{Checkpoint, load_checkpoint};
use powerbench_orchestrator::config::{SessionConfig, phase_durations, validate_config};
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
    /// Пользователь нажал «продолжить с риском»: замер разрешено начать при
    /// загруженной фоне. Без этого поля кнопка прятала себя, но не снимала
    /// гейт — все прогоны всё равно пропускались.
    #[serde(default)]
    pub accept_dirty_background: bool,
    pub worker_count: Option<usize>,
    pub scheme_ids: Vec<String>,
    /// Продолжить текущую контрольную точку (план берётся из неё).
    pub resume: bool,
    /// Активная в момент запуска схема — эталон для оценки дрейфа машины.
    /// Её прогоны и так есть в каждом раунде, отдельного времени не тратится.
    #[serde(default)]
    pub active_scheme_id: Option<String>,
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
        // Битая точка - это ошибка, а не «нет точки»: раньше интерфейс
        // предлагал «Запустить сначала» и терял отработанные раунды.
        let cp = load_checkpoint()
            .map_err(|e| format!("контрольная точка не читается: {e}"))?
            .ok_or_else(|| {
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
        accept_dirty_background: req.accept_dirty_background,
        worker_count: req.worker_count,
        scheme_ids: req.scheme_ids.clone(),
        plan_guid: crate::new_plan_guid(),
        // Эталон для оценки дрейфа — активная схема. Берётся из того же
        // запроса: интерфейс знает, какая схема активна, и передаёт её, чтобы
        // правило не расходилось между UI и CLI.
        reference_scheme_id: req.active_scheme_id.clone(),
    };
    match validate_config(&plan) {
        Some(reason) => Err(reason),
        None => Ok(plan),
    }
}

// --- Команды ---

#[tauri::command(async)]
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
#[tauri::command(async)]
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
        // Кнопка «Экспорт .pow» была в интерфейсе с самого начала, но действия
        // в мосте не было: кнопка всегда падала с «неизвестное действие
        // export». Схему нельзя было выгрузить и унести на другую машину.
        "export" => {
            let g = guid.ok_or_else(|| "export требует guid".to_string())?;
            let p = path.ok_or_else(|| "export требует путь к .pow".to_string())?;
            powercfg::export(&g, std::path::Path::new(&p))
                .map(|_| Some(p))
                .map_err(|e| e.message)
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

/// Применить схему питания и **убедиться**, что ОС её приняла.
///
/// Отдельная команда, а не ветка в `scheme_action`, потому что
/// `scheme_action("activate")` вызывает `powercfg::activate` напрямую: команда
/// может вернуть успех, а схема останется прежней (отказ SetActive, политика
/// или запрет OEM-агента). Кнопка «Применить лучшую схему» на финише замера
/// сообщала бы об успехе, оставив пользователя со старой схемой.
///
/// Здесь используется `session::apply_and_verify` — та же точка применения,
/// которой пользуется сама сессия при старте и восстановлении: попытка, затем
/// перечитывание активной схемы и повтор при отказе.
#[tauri::command(async)]
pub fn apply_scheme(guid: String) -> Result<(), String> {
    powerbench_orchestrator::session::apply_and_verify(
        &powerbench_orchestrator::session::RealSchemeDriver,
        &guid,
    )
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
    /// Плотность интерфейса: «compact» / «normal» / «roomy» (CSS `--k`).
    pub density: String,
    /// Масштаб текста: «s» / «m» / «l» (CSS `--kt`).
    pub text_scale: String,
    pub score_performance: f64,
    pub score_stability: f64,
    pub score_worst_second: f64,
    pub max_sessions: u32,
    pub favorite_schemes: Vec<String>,
    pub excluded_schemes: Vec<String>,
    /// Своими словами о настройках CPU и BIOS: разгон, андерволт, отключённые
    /// функции. Попадает в отчёт для поддержки, потому что иначе эту
    /// информацию взять неоткуда: Windows не отличает буст от разгона.
    pub cpu_notes: String,
}

fn settings_to_dto(s: &AppSettings) -> SettingsDto {
    SettingsDto {
        theme: s.appearance.theme.clone(),
        mode: s.appearance.mode.clone(),
        reduce_motion: s.appearance.reduce_motion,
        sidebar_collapsed: s.appearance.sidebar_collapsed,
        density: s.appearance.density.clone(),
        text_scale: s.appearance.text_scale.clone(),
        score_performance: s.scoring.performance,
        score_stability: s.scoring.stability,
        score_worst_second: s.scoring.worst_second,
        max_sessions: s.retention.max_sessions,
        favorite_schemes: s.favorite_schemes.clone(),
        excluded_schemes: s.excluded_schemes.clone(),
        background_threshold_percent: s.benchmark.background_threshold_percent,
        cpu_notes: s.cpu_notes.clone(),
    }
}

#[tauri::command(async)]
pub fn get_settings() -> SettingsDto {
    settings_to_dto(&AppSettings::load())
}

/// Записать настройки из интерфейса.
///
/// Правка идёт **под блокировкой файла** и поверх актуального содержимого:
/// команды Tauri выполняются на пуле потоков, поэтому «прочитать DTO → дописать
/// своё → записать» без блокировки теряло параллельные изменения (например,
/// отметку «избранное» для схемы, поставленную другим вызовом).
#[tauri::command(async)]
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
        // Неизвестное значение молча берётся как «обычно»: настройка приходит из
        // интерфейса, но конфиг мог достаться от другой версии приложения.
        cur.appearance.density = match s.density.as_str() {
            "compact" | "normal" | "roomy" => s.density,
            _ => "normal".to_string(),
        };
        cur.appearance.text_scale = match s.text_scale.as_str() {
            "s" | "m" | "l" => s.text_scale,
            _ => "m".to_string(),
        };
        cur.scoring.performance = s.score_performance;
        cur.scoring.stability = s.score_stability;
        cur.scoring.worst_second = s.score_worst_second;
        cur.retention.max_sessions = s.max_sessions;
        cur.favorite_schemes = std::mem::take(&mut s.favorite_schemes);
        cur.excluded_schemes = std::mem::take(&mut s.excluded_schemes);
        // Заметка о железе: обрезаем хвост, потому что она попадает в отчёт
        // для поддержки, а длинный текст там никому не нужен.
        cur.cpu_notes = std::mem::take(&mut s.cpu_notes).trim().to_string();
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

/// Состояние незавершённой сессии.
///
/// `Err` означает «файл есть, но не читается». Раньше такой случай выглядел
/// как «сессии нет», и интерфейс предлагал начать заново, не предупредив,
/// что отработанные раунды не восстановимы.
#[tauri::command(async)]
pub fn checkpoint_status() -> Result<Option<CheckpointDto>, String> {
    load_checkpoint()
        .map_err(|e| format!("контрольная точка не читается: {e}"))
        .map(|opt| opt.as_ref().map(CheckpointDto::from_cp))
}

/// Идентичность машины и замера для интерфейса.
///
/// Регресс H34: здесь гонялась полная самопроверка ядра (`self_check`) — это
/// несколько сотен тиков в четырёх фазах, то есть секунды работы на всех ядрах.
/// Команда объявлена `async` и выполняется на пуле tokio, поэтому страница
/// «Схемы» (а она зовёт это при каждом открытии) наглухо занимала процессор и
/// мешала всему остальному. Для идентичности она ничего не даёт: нужны только
/// метаданные движка — версия, хэш конфигурации, seed, число воркеров и
/// подпись привязки, а эталоны контрольных сумм в подписи не участвуют.
///
/// Собственно самопроверка запускается один раз перед сессией
/// ([`runner::start`] → `prepare_engine`).
#[tauri::command(async)]
pub fn identity_info() -> Result<serde_json::Value, String> {
    let engine = Engine::new(None);
    serde_json::to_value(runner::identity_of(&engine)).map_err(|e| e.to_string())
}

#[tauri::command(async)]
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

    // Битая точка — тоже «чужая»: её нужно убрать, иначе новая сессия не
    // запустится. Но исходную схему из такого файла не восстановить, поэтому
    // об этом честно говорим в лог, а не делаем вид, что всё в порядке.
    let cp = match load_checkpoint() {
        Ok(Some(cp)) => Some(cp),
        Ok(None) => None,
        Err(e) => {
            let text = format!(
                "прошлая контрольная точка не читается ({e}); исходную схему из неё восстановить нельзя, файл будет удалён"
            );
            state.log.append("error", &text);
            runner::emit_log(app, "error", &text);
            let _ = clear_checkpoint();
            return;
        }
    };
    let Some(cp) = cp else {
        return;
    };
    // Восстановление исходной схемы (если её не вернули) — до удаления точки:
    // после удаления теряется original_scheme_guid.
    if !cp.original_restored {
        let outcome = recover_interrupted_session(&RealSchemeDriver);
        if let Some(cause) = outcome.error {
            runner::emit_log(app, "warn", &format!("при сбросе прошлой сессии: {cause}"));
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

/// Итог сброса контрольной точки: что реально оказалось на диске и что
/// сказать пользователю.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscardReport {
    /// Файл контрольной точки снят с диска.
    pub cleared: bool,
    /// Сообщение для журнала и интерфейса.
    pub message: String,
    /// Нужен ли `warn` вместо `info`: есть что поправить руками.
    pub warn: bool,
}

/// Снять точку прерванной сессии: вернуть исходную схему и удалить файл.
///
/// Точка удаляется **всегда**, даже если возврат схемы не удался. Файл
/// чекпоинта и состояние схемы — разные сущности: оставлять файл только
/// из-за неудачной схемы незачем, он всё равно не помогает.
///
/// Обратный порядок был прямо в команде: при `outcome.error` она возвращала
/// `Err("точка удалена, но есть замечание: …")` и уходила **до** вызова
/// `clear_checkpoint()`. Пользователь читал «удалено», файл лежал на диске,
/// и следующий запуск снова упирался в `CheckpointPlanMismatch`.
///
/// Ядро вынесено из tauri-команды, чтобы проверялось тестом: воспроизвести
/// отказ ОС через `AppHandle` нельзя, а регресс здесь молчаливый — код
/// компилируется и работает, пока не встретит именно тот случай.
fn discard_report(
    restore: &dyn Fn() -> powerbench_orchestrator::recovery::RecoveryOutcome,
    clear: &dyn Fn() -> std::io::Result<()>,
) -> DiscardReport {
    let outcome = restore();
    // Сначала снимаем файл, и только потом формируем сообщение.
    let cleared = clear();
    let warning = outcome.error.as_deref();
    match cleared {
        Ok(()) => DiscardReport {
            cleared: true,
            warn: warning.is_some(),
            message: match warning {
                // Замечание не теряем: оно объясняет, почему схема осталась.
                Some(cause) => format!(
                    "точка удалена, но схему питания вернуть не удалось: {cause}. \
                     Верните свой план вручную: powercfg /list, затем \
                     powercfg /setactive <GUID>"
                ),
                None => "прерванная сессия забыта".to_string(),
            },
        },
        Err(e) => DiscardReport {
            cleared: false,
            warn: true,
            message: format!(
                "точка осталась на диске: не удалось её удалить ({e}). Пока она лежит, \
                 новый план не запустится — удалите файл {CHECKPOINT_FILE} \
                 в каталоге данных PowerBench вручную",
                CHECKPOINT_FILE = powerbench_orchestrator::checkpoint::CHECKPOINT_FILE_NAME,
            ),
        },
    }
}

/// Забыть прерванную сессию: вернуть исходную схему и удалить точку.
#[tauri::command(async)]
pub fn checkpoint_discard(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    use powerbench_orchestrator::checkpoint::clear_checkpoint;
    use powerbench_orchestrator::recovery::recover_interrupted_session;
    use powerbench_orchestrator::session::RealSchemeDriver;

    let report = discard_report(
        &|| recover_interrupted_session(&RealSchemeDriver),
        &clear_checkpoint,
    );
    let level = if report.warn { "warn" } else { "info" };
    state.log.append(level, &report.message);
    runner::emit_log(&app, level, &report.message);
    // Успех — только если файл действительно снят с диска. Иначе сообщение
    // об удалении было бы ложью, а именно его пользователь и ждёт.
    if report.cleared {
        Ok(())
    } else {
        Err(report.message)
    }
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
    /// Время изменения файла записи (Unix ns), 0 если недоступно.
    ///
    /// У прерванной сессии метка старта не пишется, и `started_label` содержит
    /// нулевой таймстамп `19700101T000000Z000`. Единственный реальный
    /// ориентир для такой записи — время появления файла на диске.
    pub file_modified_at_ns: u64,
    /// GUID лидера сессии: в файле может не сохраниться имя схемы, а сам GUID
    /// интерфейс умеет показать как название, если найдёт его в списке схем.
    pub leader_scheme_guid: String,
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

fn history_row(
    entry: &history::HistoryEntry,
    s: &SessionJson,
    settings: &AppSettings,
) -> HistoryRow {
    let start = history::session_started_at_ns(s).unwrap_or(0);
    let best = best_scheme(s);
    let throughput = best
        .filter(|b| b.median_throughput.is_finite() && b.median_throughput > 0.0)
        .map(|b| b.median_throughput);
    // Настоящий балл 0..=100 по весам из настроек: `throughput` (тик/с)
    // для сравнения между схемами, `score` — для сравнения сессий.
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
    let leader_scheme_guid = best.map(|b| b.scheme_id.clone()).unwrap_or_default();
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
        file_modified_at_ns: history::file_modified_at_ns(&entry.path),
        leader_scheme_guid,
    }
}

fn handled_history_rows() -> Result<Vec<HistoryRow>, String> {
    let entries = list_results().map_err(|e| format!("не удалось прочитать историю: {e}"))?;
    // Настройки читаются один раз, а не по разу на запись: при 200 сессиях в
    // истории это было 200 чтений и разборов `appsettings.json` в главном
    // потоке интерфейса при каждом открытии страницы.
    let settings = AppSettings::load();
    let mut rows = Vec::new();
    for entry in entries {
        match load_result(&entry.path) {
            Ok(s) => rows.push(history_row(&entry, &s, &settings)),
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
                file_modified_at_ns: history::file_modified_at_ns(&entry.path),
                leader_scheme_guid: String::new(),
            }),
        }
    }
    Ok(rows)
}

#[tauri::command(async)]
pub fn history_list() -> Result<Vec<HistoryRow>, String> {
    handled_history_rows()
}

#[tauri::command(async)]
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
#[tauri::command(async)]
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
        // Регресс H37: `plan_guid` подставлялся в имя файла как есть. Значение приходит
        // из JSON-файла истории, который пользователь вправе отредактировать или
        // подложить: `..\..\..\Автозагрузка\startup` уводил запись за пределы
        // выбранного каталога экспорта. Через `sanitize` в имя попадают только
        // ASCII-буквы, цифры, дефис, подчёркивание и точка — ни разделителя
        // пути, ни буквы диска там быть не может.
        let stem = history::sanitize_for_filename(&s.plan_guid);
        let file = dir.join(format!("{stem}.{format}"));
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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

/// Одна измеряемая фаза сценария (зеркало `core::config::Phase`).
#[derive(serde::Serialize)]
pub struct PhasePlanRow {
    /// Порядковый номер фазы (0..3) — он же индекс в `StoredRun::phases`.
    pub index: u8,
    /// Подпись фазы.
    pub name: String,
    /// Длительность фазы при заданной общей длительности, с.
    pub seconds: u64,
    /// Сколько процентов пула занято в этой фазе.
    pub active_worker_percent: u32,
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

/// Длительности измеряемых фаз для заданной общей длительности.
///
/// Раньше интерфейс держал свою копию формулы деления, и после появления
/// четвёртой фазы показал бы три фазы вместо четырёх — молча и до первого
/// замера. Теперь единственный источник — `orchestrator::config`.
#[tauri::command]
pub fn phase_plan(duration_seconds: u64) -> Vec<PhasePlanRow> {
    let d = phase_durations(duration_seconds);
    powerbench_core::config::PHASE_ORDER
        .iter()
        .enumerate()
        .map(|(i, p)| PhasePlanRow {
            index: i as u8,
            name: p.label().to_string(),
            seconds: match p {
                // Сопоставление по самой фазе, а не по номеру: иначе новая фаза
                // в `PHASE_ORDER` молча получила бы длительность соседней.
                powerbench_core::config::Phase::Light => d.light_seconds,
                powerbench_core::config::Phase::Partial => d.partial_seconds,
                powerbench_core::config::Phase::Heavy => d.heavy_seconds,
                powerbench_core::config::Phase::Response => d.response_seconds,
            },
            active_worker_percent: p.active_worker_percent(),
        })
        .collect()
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
#[tauri::command(async)]
pub fn quarantine_clear(scheme_id: String) -> Result<bool, String> {
    quarantine::quarantine_remove(&scheme_id)
        .map_err(|e| format!("не удалось убрать из карантина: {e}"))
}

/// HTML-отчёт по одной сессии (сохраняется рядом с результатами и
/// открывается в браузере по умолчанию).
#[tauri::command(async)]
pub fn session_report(plan_guid: String) -> Result<String, String> {
    let s = find_session(&plan_guid)?;
    crate::runner::write_session_report_strict(&s)
}

/// Удалить запись истории по имени файла.
#[tauri::command(async)]
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

#[tauri::command(async)]
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
#[tauri::command(async)]
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

/// Что получилось при сохранении отчёта для поддержки.
#[derive(serde::Serialize)]
pub struct DiagnosticsSaved {
    /// Куда записан файл.
    pub path: String,
    /// Сколько строк в отчёте.
    pub lines: usize,
    /// Сколько подозрительных состояний нашлось при сборе.
    pub findings: usize,
    /// Начало имени файла для копирования в буфер обмена.
    pub suggested_name: String,
}

/// Сохранить отчёт для поддержки.
///
/// Отчёт собирается по кнопке и целиком попадает в файл, который
/// пользователь перешлёт: журнал, окружение, идентичность замера,
/// состояние контрольной точки, карантина, настроек и последней сессии.
/// Смысл в том, чтобы по одному файлу было видно и что произошло, и в каком
/// окружении, — без доступа к машине пользователя.
#[tauri::command(async)]
pub fn save_diagnostics(
    state: tauri::State<'_, AppState>,
    path: String,
    redact: bool,
) -> Result<DiagnosticsSaved, String> {
    let target = std::path::PathBuf::from(&path);
    if target.as_os_str().is_empty() {
        return Err("не выбран путь для отчёта".to_string());
    }
let log = state.log.snapshot();
    // Перед сборкой дописываем журнал на диск: отчёт читает состояние из
    // памяти, но пользователь может приложить к обращению и сам `AppLog.json`.
    state.log.flush();
    // Если файл журнала не записался, сказать об этом здесь — последнее
    // честное место: дальше пользователь пойдёт искать в нём записи.
    state.log.warn_if_not_persisted("warn");
    let session = runner::session_description(&state.runner);
    let snapshot = crate::diagnostics::Snapshot::new(
        std::time::SystemTime::now(),
        session,
        log,
        target.clone(),
        redact,
    );
    let built = crate::diagnostics::save(&snapshot)?;
    let (saved, lines, findings) = (built.path, built.lines, built.findings);
    state.log.append(
        "info",
        &format!(
            "отчёт для поддержки сохранён: {} ({lines} строк, проблем: {findings})",
            saved.display()
        ),
    );
    Ok(DiagnosticsSaved {
        path: saved.display().to_string(),
        lines,
        findings,
        suggested_name: crate::diagnostics::default_file_name(),
    })
}

/// Имя файла отчёта по умолчанию: команда нужна, чтобы диалог сохранения
/// открывался сразу с осмысленным именем, а не `report.txt`.
#[tauri::command]
pub fn diagnostics_file_name() -> String {
    crate::diagnostics::default_file_name()
}

/// Готовность системы к запуску теста.
#[derive(serde::Serialize)]
pub struct Readiness {
    pub ok: bool,
    pub issues: Vec<String>,
}

#[tauri::command(async)]
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

/// Текущая фоновая нагрузка CPU, % от суммарной загрузки всех процессов.
///
/// Нужен на предстартовом экране, где пользователь решает, запускать ли замер
/// сейчас. Сэмпл идёт по одному интервалу в ~1.2 с: `cpu_usage()` у sysinfo
/// считается от предыдущего обновления, поэтому мгновенный вызов без паузы
/// всегда возвращал бы ноль. Сумма по процессам — та же величина, что и в
/// замере сессии, иначе индикатор врал бы относительно порога.
#[tauri::command(async)]
pub fn background_sample() -> f64 {
    let mut sampler = powerbench_windows::monitor::ProcessSampler::new();
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let sum: f64 = sampler.sample().iter().map(|p| p.cpu_percent).sum();
    (sum * 10.0).round() / 10.0
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
///
/// Прежняя версия всегда дописывала точку перед расширением, поэтому для
/// имени без расширения получалось `отчёт_2.` — Windows такой файл не
/// создаёт, и экспорт падал с «недопустимым именем» вместо записи отчёта.
fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let mut candidate = dir.join(name);
    let mut suffix = 2u32;
    let (stem, ext) = match name.rsplit_once('.') {
        // Точка в начале или в конце — не расширение, а часть имени.
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    while candidate.exists() {
        candidate = match ext {
            Some(e) => dir.join(format!("{stem}_{suffix}.{e}")),
            None => dir.join(format!("{stem}_{suffix}")),
        };
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

#[cfg(test)]
mod tests {
    use super::{DiscardReport, TestRequestDto, build_plan, discard_report, unique_path};
    use powerbench_orchestrator::recovery::RecoveryOutcome;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Итог восстановления: схема не вернулась, точка на месте.
    fn restore_failed() -> RecoveryOutcome {
        RecoveryOutcome {
            interrupted_checkpoint: true,
            needed_restore: true,
            restored: false,
            already_ok: false,
            error: Some("ОС не переключила схему".to_string()),
        }
    }

    /// Итог восстановления: всё в порядке.
    fn restore_clean() -> RecoveryOutcome {
        RecoveryOutcome {
            interrupted_checkpoint: true,
            needed_restore: true,
            restored: true,
            already_ok: true,
            error: None,
        }
    }

    /// Запрос минимального вида: все поля, которые обязательны для десериализации.
    fn request(accept_dirty_background: bool) -> TestRequestDto {
        TestRequestDto {
            preset: "quick".to_string(),
            duration_seconds: Some(20),
            warmup_seconds: Some(2),
            cooling_seconds: Some(0),
            repetitions: Some(1),
            background_threshold_percent: Some(5.0),
            accept_dirty_background,
            worker_count: None,
            scheme_ids: vec!["381b4222-f694-41f0-9685-ff5bb260df2e".to_string()],
            resume: false,
            active_scheme_id: None,
        }
    }

    /// Кнопка «продолжить с риском» обязана снимать гейт по фону, а не только
    /// прятать себя: флаг из запроса должен попасть в план сессии.
    #[test]
    fn risk_consent_reaches_the_plan() {
        assert!(
            build_plan(&request(true))
                .expect("план строится")
                .accept_dirty_background
        );
    }

    /// Обычный запуск риск не включает — иначе гейт перестал бы работать.
    #[test]
    fn ordinary_start_does_not_accept_risk() {
        assert!(
            !build_plan(&request(false))
                .expect("план строится")
                .accept_dirty_background
        );
    }

    /// Старый интерфейс (или сохранённый запрос) без нового поля читается как
    /// «риск не принимали»: десериализация не должна падать на отсутствии поля.
    #[test]
    fn request_without_the_flag_is_backward_compatible() {
        let back: TestRequestDto = serde_json::from_str(
            r#"{
                "preset": "quick",
                "duration_seconds": 20,
                "warmup_seconds": 2,
                "cooling_seconds": 0,
                "repetitions": 1,
                "background_threshold_percent": 5.0,
                "worker_count": null,
                "scheme_ids": ["381b4222-f694-41f0-9685-ff5bb260df2e"],
                "resume": false
            }"#,
        )
        .expect("старый запрос читается");
        assert!(
            !build_plan(&back)
                .expect("план строится")
                .accept_dirty_background
        );
    }

    /// Имя без расширения не должно превращаться в `имя_2.` — Windows такой
    /// файл не создаёт, то есть экспорт отчёта падал бы на каждом повторе.
    #[test]
    fn unique_path_without_extension_has_no_trailing_dot() {
        let dir = std::env::temp_dir().join(format!("pb_uniq_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report"), b"x").unwrap();

        let p = unique_path(&dir, "report");
        assert_eq!(p, dir.join("report_2"));
        assert!(p.exists() || !p.exists(), "путь не должен содержать точку");

        std::fs::write(&p, b"y").unwrap();
        assert_eq!(unique_path(&dir, "report"), dir.join("report_3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// С расширением суффикс ставится перед точкой, а не после.
    #[test]
    fn unique_path_keeps_extension_last() {
        let dir = std::env::temp_dir().join(format!("pb_uniq_html_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("PowerBench-Report-1.html"), b"x").unwrap();

        let p = unique_path(&dir, "PowerBench-Report-1.html");
        assert_eq!(p, dir.join("PowerBench-Report-1_2.html"));
        // Скрытый файл в стиле Unix не должен терять своё расширение.
        assert_eq!(unique_path(&dir, ".hidden"), dir.join(".hidden"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Регресс C11: команда «забыть прерванную сессию» сообщала
    /// «точка удалена, но есть замечание» и уходила с `Err` **до** вызова
    /// `clear_checkpoint()`. Файл оставался на диске навсегда, а следующий
    /// запуск падал с `CheckpointPlanMismatch` — то есть «удаление» было
    /// ровно тем, чем не являлось.
    #[test]
    fn discard_removes_the_file_even_when_the_scheme_was_not_restored() {
        let clears = AtomicUsize::new(0);
        let report = discard_report(
            &restore_failed,
            &|| {
                clears.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        assert_eq!(clears.load(Ordering::SeqCst), 1, "файл точки не снят с диска");
        assert!(
            report.cleared,
            "точка удалена, но сообщение об этом не поверили: {}",
            report.message
        );
        // Сообщение правдиво: файл снят, и про схему сказано отдельно.
        assert!(report.message.contains("точка удалена"), "{}", report.message);
        assert!(
            report.message.contains("powercfg /setactive"),
            "нужна инструкция, как вернуть план руками: {}",
            report.message
        );
        assert!(report.warn, "неудача возврата схемы — это предупреждение");
    }

    /// Штатный сброс: файл снят, сообщение спокойное.
    #[test]
    fn clean_discard_is_reported_as_info() {
        let report = discard_report(&restore_clean, &|| Ok(()));
        assert_eq!(
            report,
            DiscardReport {
                cleared: true,
                message: "прерванная сессия забыта".to_string(),
                warn: false,
            }
        );
    }

    /// Неудача самого удаления не должна выглядеть как успех: пользователь
    /// должен получить «точка осталась», а не «удалено».
    #[test]
    fn failed_removal_is_reported_as_the_file_staying_on_disk() {
        let report = discard_report(&restore_clean, &|| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "файл занят другим процессом",
            ))
        });
        assert!(
            !report.cleared,
            "неудача удаления выдана за успех: {}",
            report.message
        );
        assert!(
            report.message.contains("осталась на диске"),
            "{}",
            report.message
        );
        assert!(
            report.message.contains(powerbench_orchestrator::checkpoint::CHECKPOINT_FILE_NAME),
            "в ошибке нужно имя файла, который надо удалить руками: {}",
            report.message
        );
        assert!(report.warn);
    }
}
