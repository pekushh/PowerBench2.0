//! Отчёт для поддержки: один текстовый файл, по которому можно понять,
//! что у пользователя пошло не так, без доступа к его машине.
//!
//! Зачем он нужен. Обычный журнал показывает, *что* произошло, но не
//! показывает контекст: какая машина, какая версия нагрузки, что лежит в
//! каталоге данных, что было в последней сессии и — главное — какие
//! «полуразбитые» состояния остались после сбоя. Половина реальных жалоб
//! выглядит как «ничего не работает», а причина — один оставшийся
//! тестовый маркер, из-за которого схема ушла в постоянный карантин.
//! Отчёт собирает всё это в одном месте и печатает подозрительные состояния
//! отдельным списком.
//!
//! Формат — обычный текст с фиксированными разделами, а не JSON: отчёт
//! читают глазами и пересылают в чат, где JSON с тысячей строк бесполезен.
//! Внутри разделов, где это имеет смысл, остаются машинно-читаемые куски
//! JSON (настройки, схемы) — их можно распарсить при желании.
//!
//! Две гарантии, которые отчёт держит:
//!
//! * **Он ничего не меняет.** Только чтение. Отчёт собирается в том числе
//!   тогда, когда приложение «зависло», поэтому не должен ничего чинить
//!   и не должен создавать файлов в каталоге данных.
//! * **Он всегда получается.** Любой сбой сбора превращается в строку
//!   «не удалось прочитать: …», а не в панику: отчёт, который не
//!   сохранился из-за второстепенной проверки, бесполезен ровно в тот
//!   момент, когда он нужнее всего.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use powerbench_orchestrator::appsettings::AppSettings;
use powerbench_orchestrator::checkpoint::{self, Checkpoint};
use powerbench_orchestrator::history;
use powerbench_orchestrator::quarantine;
use powerbench_orchestrator::result::SessionJson;
use powerbench_windows::disk::{self, format_bytes};
use powerbench_windows::power;
use powerbench_windows::powercfg::{self, PowerScheme};

use crate::logger::LogEntry;

/// Ширина линии-разделителя. Совпадает со всеми остальными разделителями,
/// чтобы отчёт читался как один документ.
const RULE: &str =
    "--------------------------------------------------------------------------------";

/// Сколько последних записей журнала попадает в отчёт целиком.
///
/// Кольцо журнала — 5000 записей; в отчёт столько не нужно, а файл в
/// пол megabyte неудобно пересылать. Все ошибки и предупреждения попадают
/// в отчёт в любом случае, отдельным разделом.
const LOG_TAIL: usize = 1200;

/// Сколько схем питания перечислять в отчёте. На типичной машине их 7-12,
/// лимит страхует от «powercfg вернул тысячу строк».
const MAX_SCHEMES: usize = 40;

/// Замер длительности вызовов `powercfg`: медленный ответ сам по себе
/// диагностический признак (а антивирус — частая причина).
struct Timed<T> {
    value: T,
    millis: u128,
}

impl<T> Timed<T> {
    /// Время в человекочитаемом виде: «0,10 с» или «> 1 с».
    fn text(&self) -> String {
        if self.millis < 1000 {
            format!("{:.2} с", self.millis as f64 / 1000.0)
        } else {
            format!("{:.1} с", self.millis as f64 / 1000.0)
        }
    }
}

/// Запустить замыкание, замерив время; паника внутри не должна ронять отчёт.
fn timed<T, E, F: FnOnce() -> Result<T, E>>(f: F) -> Timed<Result<T, String>>
where
    E: std::fmt::Display,
{
    let start = SystemTime::now();
    // Отчёт собирается по явному действию пользователя, а паника в
    // диагностическом коде означала бы ровно то, чего отчёт не должен
    // допускать: потерю отчёта об ошибке.
    let value = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v.map_err(|e| e.to_string()),
        Err(_) => Err("внутри проверки произошла паника".to_string()),
    };
    // Именно `now - start`, а не `start`: первый вариант печатал в отчёте
    // «powercfg ответил за 1790708776 с» вместо сотых долей секунды.
    let millis = start.elapsed().map(|d| d.as_millis()).unwrap_or(0);
    Timed { value, millis }
}

/// Подозрительные состояния, найденные при сборе.
///
/// Это главная часть отчёта: она отвечает на вопрос «что, скорее всего,
/// сломано» ещё до того, как начнётся чтение остальных разделов.
#[derive(Debug, Default, PartialEq)]
struct Findings {
    items: Vec<(Severity, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    /// Точно требует внимания.
    Problem,
    /// Стоит знать, но работу не блокирует.
    Notice,
}

impl Findings {
    fn problem(&mut self, text: impl Into<String>) {
        self.items.push((Severity::Problem, text.into()));
    }
    fn notice(&mut self, text: impl Into<String>) {
        self.items.push((Severity::Notice, text.into()));
    }
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Всё, что отчёт знает о текущем запуске.
pub struct Snapshot {
    /// Момент запуска процесса — для аптайма и «на чём работало приложение».
    started_at: SystemTime,
    /// Идёт ли сейчас сессия (короткое описание для шапки).
    session: Option<String>,
    /// Журнал целиком (кольцо в памяти).
    log: Vec<LogEntry>,
    /// Куда положили отчёт.
    out_path: PathBuf,
    /// Скрывать ли путь к профилю и имя пользователя.
    redact: bool,
}

impl Snapshot {
    /// Снимок для сохранения. Команда моста не должна знать про поля.
    pub fn new(
        started_at: SystemTime,
        session: Option<String>,
        log: Vec<LogEntry>,
        out_path: PathBuf,
        redact: bool,
    ) -> Self {
        Self {
            started_at,
            session,
            log,
            out_path,
            redact,
        }
    }
}

/// Собранный отчёт: сам текст и то, что о нём стоит знать вызывающему.
pub struct Built {
    pub path: PathBuf,
    pub lines: usize,
    /// Сколько подозрительных состояний найдено: показываем это тостом
    /// пользователю, чтобы он понимал, на что смотреть в первую очередь.
    pub findings: usize,
}

/// Собрать и записать отчёт.
pub fn save(snapshot: &Snapshot) -> Result<Built, String> {
    let (text, findings) = render(snapshot);
    if let Some(parent) = snapshot.out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("не удалось создать «{}»: {e}", parent.display()))?;
    }
    std::fs::write(&snapshot.out_path, text.as_bytes())
        .map_err(|e| format!("не удалось записать «{}»: {e}", snapshot.out_path.display()))?;
    Ok(Built {
        path: snapshot.out_path.clone(),
        lines: text.lines().count(),
        findings,
    })
}

/// Имя файла отчёта по умолчанию: с датой, чтобы отчёты не слипались.
pub fn default_file_name() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "PowerBench-Diagnostics-{}.txt",
        history::date_time_stamp(now * 1_000_000_000)
    )
}

/// Собрать текст отчёта. Чистая функция от всех внешних источников, кроме
/// журнала, — её удобно тестировать. Вместе с текстом отдаёт число
/// найденных проблем.
fn render(s: &Snapshot) -> (String, usize) {
    let mut findings = Findings::default();

    let env = collect_environment(&mut findings);
    let paths = DataPaths::real();
    let data = collect_data(&paths, &mut findings);
    let schemes = collect_schemes(&mut findings);
    let last_session = collect_last_session(&paths, &mut findings);
    let log_section = collect_log(s, &mut findings);

    let mut out = String::with_capacity(64 * 1024);

    // --- Шапка: что это, когда и откуда сохранено. ---
    let generated = SystemTime::now();
    let _ = writeln!(out, "{RULE}");
    let _ = writeln!(out, "PowerBench — отчёт для поддержки");
    let _ = writeln!(out, "{RULE}");
    let _ = writeln!(out, "Собран:        {}", stamp(generated));
    let _ = writeln!(out, "Версия:        {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        out,
        "Сборка:        {} / {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        std::env::consts::ARCH
    );
    let _ = writeln!(out, "Запущено:      {}", stamp(s.started_at));
    let _ = writeln!(
        out,
        "Работает:      {}",
        s.session.as_deref().unwrap_or("сессия не идёт")
    );
    let _ = writeln!(out, "Записей в журнале: {}", s.log.len());
    let _ = writeln!(
        out,
        "Имя пользователя и путь к профилю: {}",
        if s.redact {
            "скрыты"
        } else {
            "как есть"
        }
    );
    if s.redact {
        let _ = writeln!(
            out,
            "  (в тексте ниже «{USER_PLACEHOLDER}» вместо вашего пути и «{NAME_PLACEHOLDER}» вместо вашего имени)"
        );
    }
    let _ = writeln!(out);

    // --- Сводка: главный раздел. ---
    section(&mut out, "1. СВОДКА: ЧТО ПОХОЖЕ НА ПРИЧИНУ");
    let errors = count_level(&s.log, "error");
    // Считаем ровно то же, что попадёт в раздел 7: иначе сводка и раздел
    // ошибок показывают разные числа, и оба выглядят неправильными.
    let warns = s
        .log
        .iter()
        .filter(|e| is_problem_level(&e.level) && e.level != "error")
        .count();
    let _ = writeln!(
        out,
        "Ошибок в журнале: {errors}   Предупреждений: {warns}   Записей всего: {}",
        s.log.len()
    );
    match last_entry(&s.log, &["error"]) {
        Some(e) => {
            let _ = writeln!(out, "Последняя ошибка:  {} — {}", time_of(e.ts_ms), e.text);
        }
        None => {
            let _ = writeln!(out, "Последняя ошибка:  в журнале ошибок нет");
        }
    }
    if findings.is_empty() {
        let _ = writeln!(out, "Явно сломанных состояний не найдено.");
    } else {
        let _ = writeln!(out, "Найденные проблемы:");
        for (sev, text) in &findings.items {
            let _ = writeln!(out, "  [{}] {text}", sev.tag());
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Разделы: 2 окружение · 3 идентичность замера · 4 состояние данных · \
         5 схемы питания · 6 последняя сессия · 7 ошибки и предупреждения · \
         8 журнал · 9 что проверить самому"
    );
    let _ = writeln!(out);

    // --- Окружение. ---
    section(&mut out, "2. ОКРУЖЕНИЕ");
    for (k, v) in &env.facts {
        let _ = writeln!(out, "{:<24} {}", format!("{k}:"), v);
    }
    let _ = writeln!(out);

    // --- Идентичность замера. ---
    section(&mut out, "3. ИДЕНТИЧНОСТЬ ЗАМЕРА");
    for (k, v) in &data.identity {
        let _ = writeln!(out, "{:<24} {}", format!("{k}:"), v);
    }
    if let Some(note) = &data.identity_note {
        let _ = writeln!(out, "  {note}");
    }
    let _ = writeln!(out);

    // --- Данные. ---
    section(&mut out, "4. СОСТОЯНИЕ ДАННЫХ");
    for (k, v) in &data.state {
        let _ = writeln!(out, "{:<24} {}", format!("{k}:"), v);
    }
    if let Some(json) = &data.settings_json {
        let _ = writeln!(out, "  Настройки (appsettings.json):");
        for line in json.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
    let _ = writeln!(out);

    // --- Схемы. ---
    section(&mut out, "5. СХЕМЫ ПИТАНИЯ");
    for (k, v) in &schemes.facts {
        let _ = writeln!(out, "{:<24} {}", format!("{k}:"), v);
    }
    for line in &schemes.lines {
        let _ = writeln!(out, "  {line}");
    }
    let _ = writeln!(out);

    // --- Последняя сессия. ---
    section(&mut out, "6. ПОСЛЕДНЯЯ СЕССИЯ");
    if let Some(text) = &last_session {
        for line in text.lines() {
            let _ = writeln!(out, "  {line}");
        }
    }
    let _ = writeln!(out);

    // --- Ошибки и предупреждения. ---
    section(&mut out, "7. ОШИБКИ И ПРЕДУПРЕЖДЕНИЯ");
    let problems: Vec<&LogEntry> = s
        .log
        .iter()
        .filter(|e| is_problem_level(&e.level))
        .collect();
    if problems.is_empty() {
        let _ = writeln!(out, "  Ошибок и предупреждений в журнале нет.");
    } else {
        for e in &problems {
            let _ = writeln!(out, "  {} [{:<7}] {}", time_of(e.ts_ms), e.level, e.text);
        }
    }
    let _ = writeln!(out);

    // --- Журнал. ---
    section(&mut out, &log_section.title);
    for line in &log_section.lines {
        let _ = writeln!(out, "  {line}");
    }
    let _ = writeln!(out);

    // --- Подсказки пользователю. ---
    section(&mut out, "9. ЧТО ПРОВЕРИТЬ САМОМУ");
    let hints = self_check_hints(&findings);
    if hints.is_empty() {
        let _ = writeln!(
            out,
            "  Ничего конкретного: попробуйте ещё раз и приложите новый отчёт."
        );
    } else {
        for h in hints {
            let _ = writeln!(out, "  - {h}");
        }
    }
    let _ = writeln!(out);

    if s.redact {
        redact_paths(&mut out);
    }
    (out, findings.items.len())
}

impl Severity {
    fn tag(&self) -> &'static str {
        match self {
            Severity::Problem => "ПРОБЛЕМА",
            Severity::Notice => "ВНИМАНИЕ",
        }
    }
}

fn section(out: &mut String, title: &str) {
    let _ = writeln!(out, "{RULE}");
    let _ = writeln!(out, "{title}");
    let _ = writeln!(out, "{RULE}");
}

/// Что заменить при редактировании путей.
const USER_PLACEHOLDER: &str = "%ПРОФИЛЬ%";
const NAME_PLACEHOLDER: &str = "%ИМЯ%";

/// Скрыть имя пользователя и путь к профилю.
///
/// Отчёт заведомо уходит вовне: в чужой стране или в Issues. Логин и путь
/// `C:\Users\ivanov` для диагностики ничего не дают, но много говорят о
/// пользователе, поэтому по умолчанию заменяем их на placeholders.
/// Плейсхолдеры содержат кириллицу, поэтому при последующем
/// редактировании файла «поиск по имени» не сработает.
fn redact_paths(text: &mut String) {
    // Порядок важен: сначала длинные пути, потом голое имя пользователя.
    // Обратный порядок давал `C:\Users\%ИМЯ%` вместо `%ПРОФИЛЬ%`, потому что
    // после подстановки имени полный путь уже не совпадал с тем, что в тексте.
    for var in ["USERPROFILE", "LOCALAPPDATA"] {
        let Ok(dir) = std::env::var(var) else {
            continue;
        };
        if dir.is_empty() {
            continue;
        }
        // Хвост пути (`Users\ivanov` → `Users\%ИМЯ%`) закрывает случай, когда
        // в отчёте остался только фрагмент профиля, а переменная окружения
        // отличается от того, что записала ОС при создании профиля.
        let masked = match std::path::Path::new(&dir)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| n.len() > 2)
        {
            Some(short) => dir.replace(&short, NAME_PLACEHOLDER),
            None => dir.clone(),
        };
        *text = text.replace(&dir, USER_PLACEHOLDER);
        *text = text.replace(&masked, USER_PLACEHOLDER);
    }
    if let Ok(name) = std::env::var("USERNAME")
        && name.len() > 2
    {
        *text = text.replace(&name, NAME_PLACEHOLDER);
    }
}

// --- Сбор окружения -------------------------------------------------------

struct EnvInfo {
    facts: Vec<(String, String)>,
}

fn collect_environment(findings: &mut Findings) -> EnvInfo {
    let mut facts: Vec<(String, String)> = Vec::new();
    facts.push(("Сборка Windows".into(), power::os_build()));
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    facts.push(("Логических ядер".into(), cpus.to_string()));
    // Воркеры по умолчанию — те самые `logical_cpus - 2`; если число
    // изменилось, фаза «Частичная» считает уже другую долю нагрузки, и
    // без этой строки сравнение снимков бессмысленно.
    facts.push((
        "Воркеров по умолчанию".into(),
        cpus.saturating_sub(2).max(1).to_string(),
    ));
    facts.push(("Память".into(), format!("{:.1} ГиБ", power::memory_gib())));
    facts.push(("CPU".into(), {
        let brand = power::cpu_brand();
        if brand.trim().is_empty() {
            "не определён".to_string()
        } else {
            brand
        }
    }));
    facts.push((
        "Права администратора".into(),
        yes_no(power::is_admin()).to_string(),
    ));
    facts.push((
        "Питание от сети".into(),
        match power::ac_power_online() {
            Ok(true) => "да".to_string(),
            Ok(false) => "НЕТ (только батарея)".to_string(),
            Err(e) => format!("не удалось определить: {e:?}"),
        },
    ));
    // Состояние питания: `Debug` здесь бесполезен (`PowerState { max_mhz: … }`),
    // а именно по нему видно, держит ли система частоту — из-за этого
    // результат занижается, и об этом должна знать и поддержка, и пользователь.
    let ps = power::power_state();
    facts.push((
        "Частота CPU".into(),
        if ps.max_mhz == 0 {
            "Windows не сообщает".to_string()
        } else if ps.throttled {
            format!(
                "медленное ядро {} МГц при потолке {} МГц — СИСТЕМА ДЕРЖИТ ЧАСТОТУ",
                ps.current_mhz, ps.max_mhz
            )
        } else {
            format!(
                "медленное ядро {} МГц, потолок {} МГц",
                ps.current_mhz, ps.max_mhz
            )
        },
    ));
    facts.push((
        "Ограничение частоты".into(),
        if ps.unavailable {
            "Windows не отдаёт состояние".to_string()
        } else if ps.thermal_throttle {
            format!("да, тепловой (ACPI-код {})", ps.policy_reason)
        } else if ps.throttled {
            format!("да, ACPI-код {}", ps.policy_reason)
        } else {
            // Важная оговорка, без которой строка вводит в заблуждение:
            // «нет» означает «система не держит процессор ниже разрешённого
            // потолка». Потолок этот — с учётом буста, а базовая частота здесь
            // не измеряется, поэтому застрять на ней эта проверка не может.
            "нет (ниже разрешённого потолка не держит)".to_string()
        },
    ));
    if ps.throttled || ps.thermal_throttle {
        findings.problem(
            "Windows удерживала частоту CPU ниже разрешённого потолка во время сбора \
             данных — результаты занижены, и это не дефект PowerBench. Проверьте \
             температуру и сторонние программы, ограничивающие частоту",
        );
    }
    facts.push((
        "Частота QPC".into(),
        format!("{} Гц", power::qpc_frequency()),
    ));
    facts.push(("Идентификатор CPU".into(), {
        let id = power::cpu_identifier();
        if id.is_empty() {
            "не определён".to_string()
        } else {
            id
        }
    }));
    let data_dir = checkpoint::data_dir();
    facts.push(("Каталог данных".into(), data_dir.display().to_string()));
    let free = disk::free_space_bytes(&data_dir);
    let total = disk::volume_bytes(&data_dir);
    facts.push((
        "Свободно на диске".into(),
        match (&free, &total) {
            (Ok(f), Ok(t)) => format!("{} из {}", format_bytes(*f), format_bytes(*t)),
            (Ok(f), Err(_)) => format_bytes(*f),
            (Err(e), _) => format!("не удалось определить: {e}"),
        },
    ));
    if let Err(e) = &free {
        // Ровно этот случай превращался в «Свободно 0 Б» и ложное
        // предупреждение «место заканчивается» на экране результатов.
        findings.problem(format!("не читается объём диска: {e}"));
    }
    if !power::is_admin() {
        findings.problem(
            "приложение запущено без прав администратора — смена схем питания не сработает",
        );
    }
    if !facts.is_empty() {
        facts.push((
            "Версия диагностики".into(),
            power::diagnostics_version().to_string(),
        ));
    }
    EnvInfo { facts }
}

/// Откуда читать состояние данных.
///
/// Путь передаётся явно, а не берётся из окружения: отчёт обязан собираться
/// из произвольного каталога, иначе его нельзя проверить тестом, не затрагивая
/// настоящие данные пользователя.
#[derive(Clone)]
struct DataPaths {
    dir: PathBuf,
}

impl DataPaths {
    fn real() -> Self {
        Self {
            dir: checkpoint::data_dir(),
        }
    }
    fn checkpoint(&self) -> PathBuf {
        self.dir.join(checkpoint::CHECKPOINT_FILE_NAME)
    }
    fn marker(&self) -> PathBuf {
        self.dir.join(quarantine::TESTING_MARKER_FILE_NAME)
    }
    fn quarantine(&self) -> PathBuf {
        self.dir.join(quarantine::QUARANTINE_FILE_NAME)
    }
    fn settings(&self) -> PathBuf {
        self.dir.join("appsettings.json")
    }
    fn results(&self) -> PathBuf {
        self.dir.join("Results")
    }
}

/// Прочитать тестовый маркер из указанного файла.
///
/// Свой путь вместо `quarantine::load_testing_marker`: отчёт проверяется
/// тестом на временном каталоге, а тот по необходимости читает настоящий
/// каталог данных пользователя.
fn load_marker(path: &Path) -> Option<quarantine::TestingMarker> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Прочитать карантин из указанного файла.
fn load_quarantine_file(path: &Path) -> Vec<quarantine::QuarantineEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

// --- Сбор состояния данных ------------------------------------------------

struct DataInfo {
    identity: Vec<(String, String)>,
    identity_note: Option<String>,
    state: Vec<(String, String)>,
    settings_json: Option<String>,
}

fn collect_data(paths: &DataPaths, findings: &mut Findings) -> DataInfo {
    let mut identity: Vec<(String, String)> = Vec::new();
    let mut identity_note: Option<String> = None;
    let mut state: Vec<(String, String)> = Vec::new();
    let mut settings_json: Option<String> = None;

    // Идентичность текущей сборки — без `Engine`: создание движка
    // выделяет мегабайты буферов, а отчёт собирается по кнопке и не должен
    // подвисать.
    let hash = powerbench_core::config::config_hash().to_string();
    identity.push((
        "Версия нагрузки".into(),
        powerbench_core::config::VERSION.to_string(),
    ));
    identity.push(("Хеш конфигурации".into(), hash.clone()));
    identity.push((
        "Seed".into(),
        format!("{:016X}", powerbench_core::config::SEED),
    ));
    identity.push((
        "Версия диагностики".into(),
        power::diagnostics_version().to_string(),
    ));

    // Контрольная точка. Ошибку чтения показываем дословно: раньше битый
    // файл выглядел как «сессии нет», и это стоило пользователю раундов.
    let cp_result = checkpoint::load_checkpoint_from(&paths.checkpoint());
    match &cp_result {
        Ok(Some(cp)) => {
            let done = completed_rounds(cp);
            state.push((
                "Контрольная точка".into(),
                format!(
                    "есть — план {}, раундов выполнено {done} из {}, схем {}",
                    cp.plan.plan_guid,
                    cp.plan.repetitions,
                    cp.plan.scheme_ids.len()
                ),
            ));
            if done < cp.plan.repetitions as usize {
                findings.notice(format!(
                    "есть незавершённая сессия ({} из {} раундов) — её можно продолжить",
                    done, cp.plan.repetitions
                ));
            }
            if !cp.original_restored {
                findings.problem(
                    "исходная схема питания ещё не восстановлена — возможно, сессия \
                     прервалась; запустите любой замер или закройте приложение, чтобы \
                     вернуть схему",
                );
            }
        }
        Ok(None) => {
            state.push(("Контрольная точка".into(), "нет".into()));
        }
        Err(e) => {
            state.push(("Контрольная точка".into(), format!("НЕ ЧИТАЕТСЯ: {e}")));
            findings.problem(format!(
                "контрольная точка не читается ({e}); продолжить прерванную сессию нельзя, \
                 файл можно удалить вручную: {CHECKPOINT_FILE}"
            ));
        }
    }

    // Тестовый маркер. Именно он объясняет «схема ни с чего ушла в карантин».
    match load_marker(&paths.marker()) {
        Some(marker) => {
            state.push((
                "Тестовый маркер".into(),
                format!(
                    "ОСТАЛСЯ: схема {} ({}), замер начат {}",
                    marker.scheme_name.as_deref().unwrap_or("без имени"),
                    marker.scheme_id,
                    history::date_time_stamp(marker.started_at_ns)
                ),
            ));
            findings.problem(format!(
                "остался тестовый маркер замера: {} — при следующем запуске эта схема \
                 попадёт в постоянный карантин как «зависание», хотя замер мог быть \
                 прерван обычным образом (перезагрузка, сон, закрытие программы)",
                marker.scheme_name.as_deref().unwrap_or(&marker.scheme_id)
            ));
        }
        None => {
            state.push(("Тестовый маркер".into(), "нет".into()));
        }
    }

    // Карантин.
    let quarantine = load_quarantine_file(&paths.quarantine());
    state.push((
        "Карантин".into(),
        if quarantine.is_empty() {
            "пуст".to_string()
        } else {
            format!("{} схем", quarantine.len())
        },
    ));
    for entry in &quarantine {
        state.push((
            format!("  карантин {}", short_guid(&entry.scheme_id)),
            format!(
                "{} — {}",
                entry.scheme_name.as_deref().unwrap_or("без имени"),
                entry.reason
            ),
        ));
    }
    if let Err(e) = std::fs::metadata(paths.quarantine())
        && e.kind() != std::io::ErrorKind::NotFound
    {
        findings.notice(format!("файл карантина не читается: {e}"));
    }

    // История: количество и объём без чтения всех файлов.
    let entries = timed(history::list_results);
    match entries.value {
        Ok(list) => {
            state.push((
                "Записей в истории".into(),
                format!(
                    "{} (каталог {})",
                    list.len(),
                    history::results_dir().display()
                ),
            ));
        }
        Err(e) => {
            state.push(("Записей в истории".into(), format!("не читаются: {e}")));
            findings.problem(format!("история результатов не читается: {e}"));
        }
    }

    // Настройки: показываем как есть, но если файл не читается — это
    // отдельная находка, потому что следующая же правка затрёт его
    // значениями по умолчанию.
    let settings_path = paths.settings();
    match std::fs::read_to_string(&settings_path) {
        Ok(text) => match serde_json::from_str::<AppSettings>(&text) {
            Ok(_) => {
                state.push((
                    "Настройки".into(),
                    format!("читаются ({})", settings_path.display()),
                ));
                settings_json = Some(text);
            }
            Err(e) => {
                state.push(("Настройки".into(), format!("НЕ РАЗОБИРАЮТСЯ: {e}")));
                findings.problem(format!(
                    "appsettings.json не разбирается ({e}); интерфейс покажет значения по \
                     умолчанию, а первая же правка перезапишет файл и ваши настройки \
                     потеряются"
                ));
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            state.push((
                "Настройки".into(),
                "нет файла (используются значения по умолчанию)".into(),
            ));
        }
        Err(e) => {
            state.push(("Настройки".into(), format!("не читаются: {e}")));
            findings.problem(format!("appsettings.json не читается: {e}"));
        }
    }

    // Сверка хеша конфигурации с последней сессией: расхождение объясняет
    // «контрольная сумма различается между повторами» без правок кода.
    if let Some(sess) = load_last_session(paths)
        && sess.identity.config_hash != hash
    {
        identity_note = Some(format!(
            "ВНИМАНИЕ: последняя сессия измерена при ДРУГОЙ конфигурации нагрузки \
             (хеш {}), а приложение сейчас собрано с {}. Результаты такой сессии нельзя \
             сравнивать с новыми, и расхождение контрольных сумм тут ни при чём.",
            short_hash(&sess.identity.config_hash),
            short_hash(&hash)
        ));
        findings.problem("конфигурация нагрузки изменилась после последней сессии");
    }

    DataInfo {
        identity,
        identity_note,
        state,
        settings_json,
    }
}

/// Сколько раундов плана уже выполнено по контрольной точке.
fn completed_rounds(cp: &Checkpoint) -> usize {
    if cp.plan.scheme_ids.is_empty() {
        return cp.completed_keys.len();
    }
    // `completed_keys` содержит по записи на схему на раунд, поэтому это
    // число нельзя показывать пользователю как «раунды».
    cp.completed_keys.len() / cp.plan.scheme_ids.len().max(1)
}

// --- Схемы питания --------------------------------------------------------

struct SchemeInfo {
    facts: Vec<(String, String)>,
    lines: Vec<String>,
}

fn collect_schemes(findings: &mut Findings) -> SchemeInfo {
    let mut facts: Vec<(String, String)> = Vec::new();
    let mut lines: Vec<String> = Vec::new();

    let listed = timed(|| powercfg::list_schemes().map_err(|e| e.message));
    match &listed.value {
        Ok(list) => {
            facts.push((
                "Всего схем".into(),
                format!("{} (powercfg ответил за {})", list.len(), listed.text()),
            ));
            let active_now = list.iter().find(|s| s.active).map(|s| s.active_name());
            facts.push((
                "Активная сейчас".into(),
                active_now.unwrap_or_else(|| "не определена".to_string()),
            ));
            for sch in list.iter().take(MAX_SCHEMES) {
                lines.push(scheme_line(sch));
            }
            if list.len() > MAX_SCHEMES {
                lines.push(format!("…и ещё {} схем", list.len() - MAX_SCHEMES));
            }
        }
        Err(e) => {
            facts.push(("Всего схем".into(), format!("powercfg НЕ ОТВЕТИЛ: {e}")));
            findings.problem(format!(
                "powercfg не отвечает ({e}); без него приложение не может ни показать \
                 схемы, ни переключить их — скорее всего, причина именно в этом"
            ));
        }
    }

    // Активная схема по отдельному запросу: расхождение с флагом из
    // `/list` само по себе признак того, что схему переключили извне.
    let active = timed(|| powercfg::active_scheme().map_err(|e| e.message));
    match &active.value {
        Ok(guid) => {
            facts.push((
                "Активная (/getactive)".into(),
                format!("{guid} (ответ за {})", active.text()),
            ));
            if let Ok(l) = &listed.value
                && let Some(marked) = l.iter().find(|s| s.active)
                && !marked.guid.eq_ignore_ascii_case(guid)
            {
                findings.notice(
                    "активная схема по /getactivescheme не совпадает с отмеченной \
                     в /list — возможно, её переключили из другой программы",
                );
            }
        }
        Err(e) => {
            facts.push((
                "Активная (/getactive)".into(),
                format!("не удалось определить: {e}"),
            ));
        }
    }

    SchemeInfo { facts, lines }
}

fn scheme_line(sch: &PowerScheme) -> String {
    let mark = if sch.active { "*" } else { " " };
    format!("{mark} {}  {}", sch.guid, sch.name)
}

trait ActiveName {
    fn active_name(&self) -> String;
}

impl ActiveName for PowerScheme {
    fn active_name(&self) -> String {
        format!("{} ({})", self.name, self.guid)
    }
}

// --- Последняя сессия -----------------------------------------------------

fn collect_last_session(paths: &DataPaths, findings: &mut Findings) -> Option<String> {
    // `list_results` сортирует по имени файла по убыванию, а имя начинается
    // с метки времени, поэтому первый элемент — самая свежая сессия.
    let list = history::list_results_in(&paths.results()).ok()?;
    let newest = list.first()?;
    let sess = match load_result_file(&newest.path) {
        Ok(s) => s,
        Err(e) => {
            findings.problem(format!(
                "последний файл результата {} не читается: {e}",
                newest.file_name
            ));
            return Some(format!(
                "Файл {} не читается: {e}\nЭто может быть и причиной жалобы.",
                newest.file_name
            ));
        }
    };
    Some(describe_session(&sess))
}

fn load_last_session(paths: &DataPaths) -> Option<SessionJson> {
    let list = history::list_results_in(&paths.results()).ok()?;
    let newest = list.first()?;
    load_result_file(&newest.path).ok()
}

fn load_result_file(path: &Path) -> Result<SessionJson, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("не удалось прочитать {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| {
        format!(
            "{} не разбирается как результат сессии: {e}",
            path.display()
        )
    })
}

/// Текстовое описание последней сессии: строки для раздела 6.
fn describe_session(sess: &SessionJson) -> String {
    let mut out = String::new();
    let started = history::session_started_at_ns(sess).unwrap_or(0);
    if started > 0 {
        // Локальное время, как и в журнале: метка `20260929T154824Z689`
        // годится для машинной сверки, но не для человека.
        let _ = writeln!(
            out,
            "Начата:           {}",
            power::local_time_string(started / 1_000_000_000)
        );
    } else {
        let _ = writeln!(out, "Начата:           неизвестно (нет ни одного прогона)");
    }
    let _ = writeln!(out, "План:             {}", sess.plan_guid);
    let _ = writeln!(
        out,
        "Вердикт:          {} — {}",
        sess.recommendation.level_label, sess.recommendation.reason
    );
    if let Some(r) = &sess.recommendation.recommended_scheme {
        let name = sess
            .schemes
            .iter()
            .find(|s| &s.scheme_id == r)
            .and_then(|s| s.name.clone())
            .unwrap_or_else(|| r.clone());
        let _ = writeln!(out, "Рекомендована:    {name} [{r}]");
    }
    let _ = writeln!(
        out,
        "Раундов:          {} из {}",
        sess.rounds_completed, sess.rounds_planned
    );
    if let Some(reason) = &sess.early_stop_reason {
        let _ = writeln!(out, "Остановлена рано:  {reason}");
    }
    let _ = writeln!(out);
    // Ширины колонок шапки собраны тем же форматом, что и строки данных:
    // иначе колонки «съезжают» и отчёт приходится выравнивать глазами.
    let header = format!(
        "{:<26} {:>7} {:>9} {:>8} {:>5} %  {}",
        "Схема", "раундов", "медиана", "P1", "стаб.", "статус"
    );
    let _ = writeln!(out, "  {header}");
    for sch in &sess.schemes {
        let name = sch.name.as_deref().unwrap_or(sch.scheme_id.as_str());
        let status = if sch.rejected {
            format!("БРАК: {}", sch.rejection_reason.as_deref().unwrap_or(""))
        } else if sch.runs == 0 {
            "НЕ ИЗМЕРЕНА".to_string()
        } else {
            "допущена".to_string()
        };
        let _ = writeln!(
            out,
            "  {:<26} {:>7} {:>9.0} {:>8.0} {:>5.0} %  {}",
            truncate(name, 26),
            sch.runs,
            sch.median_throughput,
            sch.median_p1_throughput,
            sch.median_consistency_percent,
            status
        );
    }
    // Пофазовая разбивка: по сводной медиане не видно, какая именно фаза
    // просела, а это ровно то, о чём чаще всего спрашивают при разборе
    // «результаты странные».
    for sch in &sess.schemes {
        if sch.phases.is_empty() {
            continue;
        }
        let name = sch.name.as_deref().unwrap_or(sch.scheme_id.as_str());
        let parts: Vec<String> = sch
            .phases
            .iter()
            .map(|p| {
                format!(
                    "{} {:.0}/{:.0}",
                    truncate(&p.name, 10),
                    p.median_throughput,
                    p.p1_throughput
                )
            })
            .collect();
        let _ = writeln!(out, "    {}: {}", truncate(name, 26), parts.join("  "));
    }
    if !sess.warnings.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Предупреждения замера:");
        for w in &sess.warnings {
            let _ = writeln!(out, "  - {w}");
        }
    }
    if let Some(r) = &sess.reference {
        let _ = writeln!(out);
        let _ = writeln!(out, "Дрейф машины по опорной схеме: {}", r.note());
    }
    out
}

// --- Журнал ---------------------------------------------------------------

struct LogSection {
    title: String,
    lines: Vec<String>,
}

fn collect_log(s: &Snapshot, findings: &mut Findings) -> LogSection {
    let total = s.log.len();
    let start = total.saturating_sub(LOG_TAIL);
    let mut lines = Vec::new();
    if start > 0 {
        lines.push(format!(
            "… первые {start} записей пропущены (показаны последние {LOG_TAIL} из {total}). \
             Все ошибки и предупреждения приведены выше, в разделе 7."
        ));
    }
    for e in &s.log[start..] {
        lines.push(format!("{} [{:<7}] {}", time_of(e.ts_ms), e.level, e.text));
    }
    // Если в отчёте не осталось ни одной ошибки, но пользователь её видел,
    // она почти наверняка была в кольце, которое уже переполнено.
    let has_problem = s.log.iter().any(|e| is_problem_level(&e.level));
    if !has_problem && total >= LOG_TAIL {
        findings.problem(
            "в сохранённом журнале нет ни одной ошибки, хотя кольцо журнала переполнено — \
             нужная запись, вероятно, вытеснена более поздними",
        );
    }
    LogSection {
        title: if start == 0 {
            format!("8. ЖУРНАЛ (все {total})")
        } else {
            format!("8. ЖУРНАЛ (последние {LOG_TAIL} из {total})")
        },
        lines,
    }
}

// --- Подсказки ------------------------------------------------------------

/// Что пользователь может проверить сам, не дожидаясь ответа.
///
/// Список строится только по реально найденным проблемам: общие советы
/// «проверьте антивирус» в каждом отчёте только шумят.
fn self_check_hints(findings: &Findings) -> Vec<&'static str> {
    let mut hints = Vec::new();
    let has = |needle: &str| findings.items.iter().any(|(_, text)| text.contains(needle));
    if has("powercfg") {
        hints.push(
            "powercfg не отвечает: проверьте антивирус и корпоративные политики — они \
             часто блокируют запуск системных утилит и создание процессов",
        );
        hints.push(
            "попробуйте вручную выполнить в командной строке от администратора: \
             powercfg /list — если команда не выводит список, дело точно не в PowerBench",
        );
    }
    if has("администратора") {
        hints.push(
            "запускайте приложение «от имени администратора»: без прав смена схемы невозможна",
        );
    }
    if has("маркер") {
        hints.push(
            "если замер прерывали перезагрузкой, сном или закрытием программы — так и \
             должно быть; просто начните новый замер, а тот карантин снимите вручную на \
             странице «Схемы»",
        );
    }
    if has("исходная схема") {
        hints.push(
            "исходную схему питания можно вернуть вручную: «Схемы» → нужная схема → \
             «Сделать активной»",
        );
    }
    if has("контрольная точка") {
        hints.push(
            "файл с испорченной контрольной точкой лежит в каталоге данных, его видно в \
             шапке раздела 4; сохраните его себе и удалите, чтобы начать новый замер",
        );
    }
    if has("настройки") {
        hints.push(
            "если настройки не разбираются, скопируйте appsettings.json в сторону: без \
             этого любая правка настроек перезапишет файл пустыми значениями",
        );
    }
    if has("диск") {
        hints.push("проверьте свободное место на системном диске: история растёт сама, очистка не реализована");
    }
    if has("конфигурация нагрузки") {
        hints.push(
            "сравнивать результаты до и после обновления программы нельзя: нагрузка \
             изменилась, поэтому перезапустите серию замеров целиком",
        );
    }
    if has("кольцо журнала") {
        hints.push(
            "журнал переполняется: записи об ошибке могли вытесниться. Собирайте отчёт \
             сразу после ошибки",
        );
    }
    if has("не определил") || has("питание") {
        hints.push("на некоторых ноутбуках и в виртуальных машинах состояние питания не определяется — это не ошибка PowerBench");
    }
    hints
}

// --- Мелкие помощники -----------------------------------------------------

fn is_problem_level(level: &str) -> bool {
    matches!(level, "error" | "warn" | "warning")
}

fn count_level(log: &[LogEntry], level: &str) -> usize {
    log.iter().filter(|e| e.level == level).count()
}

fn last_entry<'a>(log: &'a [LogEntry], levels: &[&str]) -> Option<&'a LogEntry> {
    log.iter()
        .rev()
        .find(|e| levels.contains(&e.level.as_str()))
}

fn yes_no(v: bool) -> &'static str {
    if v { "да" } else { "нет" }
}

fn short_guid(guid: &str) -> String {
    guid.chars().take(8).collect()
}

fn short_hash(hash: &str) -> String {
    if hash.len() > 12 {
        format!("{}…", &hash[..12])
    } else {
        hash.to_string()
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Время записи журнала в местном виде: метка в отчёте обязана читаться так
/// же, как в интерфейсе (там время выводит JavaScript, то есть местное).
/// Иначе строку из отчёта невозможно сопоставить со скриншотом журнала.
fn time_of(ts_ms: u64) -> String {
    power::local_time_string(ts_ms / 1000)
}

/// Дата в человекочитаемом виде по секундам epoch, местная зона.
fn stamp(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Зона в скобках: без неё метку из отчёта невозможно сопоставить с
    // местным временем пользователя, если он в другой зоне.
    format!(
        "{} ({})",
        power::local_time_string(secs),
        power::local_offset_label(secs)
    )
}

/// Перевод дней от Unix-эпохи в дату. Своя реализация, а не `chrono`:
/// зависимость ради одной функции не оправдана, а местное время всё равно
/// считает `power::local_time_string`.
#[allow(dead_code)]
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const CHECKPOINT_FILE: &str = "benchmark-checkpoint.json";

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_log() -> Vec<LogEntry> {
        vec![
            LogEntry {
                ts_ms: 1_000,
                level: "info".into(),
                text: "сессия начата".into(),
            },
            LogEntry {
                ts_ms: 2_000,
                level: "warn".into(),
                text: "фон высокий".into(),
            },
            LogEntry {
                ts_ms: 3_000,
                level: "error".into(),
                text: "powercfg не ответил за 10 с".into(),
            },
        ]
    }

    /// Отчёт обязан собираться на машине без схем и без истории: именно
    /// такой случай и приходит от пользователя с ошибкой.
    #[test]
    fn render_never_fails_and_has_all_sections() {
        let tmp = std::env::temp_dir().join(format!("pb-diag-{}", std::process::id()));
        let s = Snapshot {
            started_at: SystemTime::now(),
            session: None,
            log: fake_log(),
            out_path: tmp.join("r.txt"),
            redact: false,
        };
        let (text, _) = render(&s);
        for title in [
            "1. СВОДКА",
            "2. ОКРУЖЕНИЕ",
            "3. ИДЕНТИЧНОСТЬ ЗАМЕРА",
            "4. СОСТОЯНИЕ ДАННЫХ",
            "5. СХЕМЫ ПИТАНИЯ",
            "6. ПОСЛЕДНЯЯ СЕССИЯ",
            "7. ОШИБКИ И ПРЕДУПРЕЖДЕНИЯ",
            "8. ЖУРНАЛ",
            "9. ЧТО ПРОВЕРИТЬ САМОМУ",
        ] {
            assert!(text.contains(title), "в отчёте нет раздела «{title}»");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Главный раздел отчёта обязан называть последнюю ошибку: по нему
    /// видно, что именно прислал пользователь.
    #[test]
    fn summary_names_the_last_error() {
        let tmp = std::env::temp_dir().join(format!("pb-diag-e-{}", std::process::id()));
        let s = Snapshot {
            started_at: SystemTime::now(),
            session: Some("прогон 1/3".into()),
            log: fake_log(),
            out_path: tmp.join("r.txt"),
            redact: false,
        };
        let (text, _) = render(&s);
        assert!(
            text.contains("Последняя ошибка") && text.contains("powercfg не ответил"),
            "сводка должна называть последнюю ошибку:\n{text}"
        );
        assert!(
            text.contains("прогон 1/3"),
            "шапка должна говорить, что идёт сессия"
        );
        // Раздел ошибок обязан содержать и предупреждение, а не только error.
        let problems = text
            .split("7. ОШИБКИ И ПРЕДУПРЕЖДЕНИЯ")
            .nth(1)
            .expect("нет раздела 7");
        assert!(problems.contains("фон высокий"), "предупреждение потеряно");
        assert!(problems.contains("powercfg не ответил"), "ошибка потеряна");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Хвост журнала обрезается, но ошибки из начала остаются: иначе
    /// отчёт может не содержать самой нужной записи.
    #[test]
    fn log_tail_is_capped_but_problems_are_kept() {
        let log: Vec<LogEntry> = (0..LOG_TAIL + 50)
            .map(|i| LogEntry {
                ts_ms: i as u64 * 1000,
                level: if i == 0 {
                    "error".into()
                } else {
                    "info".into()
                },
                text: format!("запись {i}"),
            })
            .collect();
        let tmp = std::env::temp_dir().join(format!("pb-diag-t-{}", std::process::id()));
        let s = Snapshot {
            started_at: SystemTime::now(),
            session: None,
            log,
            out_path: tmp.join("r.txt"),
            redact: false,
        };
        let (text, _) = render(&s);
        assert!(
            text.contains("запись 0"),
            "ошибка из начала кольца обязана попасть в раздел ошибок"
        );
        let journal = text.split("8. ЖУРНАЛ").nth(1).expect("нет раздела 8");
        assert!(
            !journal.contains("запись 1\n"),
            "в хвост журнала не должен попасть самый хвост кольца без обрезки"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Даты в отчёте и даты в приложении обязаны считаться одинаково, иначе
    /// строку из журнала невозможно сопоставить с шапкой отчёта. Сверяемся с
    /// собственной функцией приложения, а не с зашитым числом.
    #[test]
    fn dates_agree_with_the_app() {
        // 2026-09-29T18:56:46Z — момент последнего живого замера.
        let ns = 1_790_708_206_000_000_000u64;
        assert_eq!(
            powerbench_orchestrator::history::date_time_stamp(ns),
            "20260929T185646Z000"
        );
        // В отчёте время местное, как в интерфейсе: сдвиг зоны применяется
        // один раз, иначе метка из отчёта разошлась бы с журналом на часы.
        let secs = (ns / 1_000_000_000) as i64;
        let local = power::local_time_string(secs as u64);
        let off = power::local_offset_label(secs as u64);
        assert_eq!(
            time_of(ns / 1_000_000),
            local,
            "метка в отчёте не равна местному времени"
        );
        assert!(
            off.starts_with("UTC+") || off.starts_with("UTC-"),
            "зона не определена: {off}"
        );
        // Эпоха: раньше на таких сессиях отчёт печатал 01.01.1970, и строка
        // выглядела как мусор, но время должно быть осмысленным.
        let epoch_label = time_of(0);
        assert!(
            epoch_label.starts_with("01.01.1970 "),
            "неожиданная метка эпохи: {epoch_label}"
        );
    }

    /// Редактирование убирает путь к профилю и имя пользователя, но не
    /// трогает посторонние пути: иначе в отчёте пропадёт половина
    /// диагностики (например, каталог установки в `Program Files`).
    #[test]
    fn redaction_hides_profile_and_user_name_only() {
        let profile = std::env::var("USERPROFILE").expect("USERPROFILE");
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let user = std::env::var("USERNAME").unwrap_or_default();
        // Имя отдельно от пути: в отчёте оно может встретиться и без пути
        // (например, в тексте сообщения об ошибке от Windows).
        let mut text = format!(
            "Каталог данных: {local}\\PowerBench\nПрофиль: {profile}\n\
             Установлено пользователем: {user}\n\
             Установка: C:\\Program Files\\PowerBench\\powerbench-app.exe\n"
        );
        redact_paths(&mut text);
        assert!(
            !text.contains(&profile),
            "путь к профилю остался в отчёте: {text}"
        );
        assert!(
            text.contains(USER_PLACEHOLDER),
            "нет плейсхолдера профиля: {text}"
        );
        assert!(
            text.contains("C:\\Program Files\\PowerBench"),
            "редактирование не должно трогать посторонние пути: {text}"
        );
        if user.len() > 2 {
            assert!(
                !text.contains(&user),
                "имя пользователя осталось в отчёте: {text}"
            );
            assert!(
                text.contains(NAME_PLACEHOLDER),
                "нет плейсхолдера имени: {text}"
            );
        }
    }

    /// Главная ценность отчёта — распознавать «полуразбитые» состояния.
    /// Проверяем на настоящих файлах во временном каталоге: битая контрольная
    /// точка и оставшийся тестовый маркер должны попасть в сводку и в
    /// подсказки, а не тихо остаться в файлах.
    #[test]
    fn broken_states_are_detected_and_explained() {
        let dir = std::env::temp_dir().join(format!("pb-diag-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Results")).unwrap();
        let paths = DataPaths { dir: dir.clone() };

        // Битая контрольная точка: раньше это выглядело как «сессии нет».
        std::fs::write(paths.checkpoint(), b"{ not json").unwrap();
        // Маркер от прерванного замера: именно он объясняет ложный карантин.
        std::fs::write(
            paths.marker(),
            r#"{"scheme_id":"aaaa-bbbb","scheme_name":"Мой план","plan_guid":"plan-x","started_at_ns":1790708206000000000}"#
                .as_bytes(),
        )
        .unwrap();

        let mut findings = Findings::default();
        let data = collect_data(&paths, &mut findings);
        let text: Vec<String> = findings.items.iter().map(|(_, t)| t.clone()).collect();
        let joined = text.join("\n");
        assert!(
            joined.contains("маркер"),
            "оставшийся маркер не распознан: {joined}"
        );
        assert!(
            joined.contains("контрольная точка не читается"),
            "битая контрольная точка не распознана: {joined}"
        );
        let state: Vec<String> = data.state.iter().map(|(_, v)| v.clone()).collect();
        assert!(
            state.iter().any(|s| s.contains("НЕ ЧИТАЕТСЯ")),
            "в состоянии данных должно быть видно, что точка не читается: {state:?}"
        );
        assert!(
            state.iter().any(|s| s.contains("ОСТАЛСЯ")),
            "в состоянии данных должно быть видно, что маркер остался: {state:?}"
        );
        // Подсказка про штатное прерывание обязана появиться: именно её
        // пользователь чаще всего и не знает.
        let hints = self_check_hints(&findings);
        assert!(
            hints.iter().any(|h| h.contains("перезагрузкой")),
            "для маркера нужна подсказка про штатное прерывание: {hints:?}"
        );
        assert!(
            hints.iter().any(|h| h.contains("контрольной точкой")),
            "для битой точки нужна подсказка, что с ней делать: {hints:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Карантин читается из указанного файла: отчёт не должен молчать, если
    /// файл есть, а список в нём другой.
    #[test]
    fn quarantine_is_read_from_the_given_file() {
        let dir = std::env::temp_dir().join(format!("pb-diag-q-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = DataPaths { dir: dir.clone() };
        assert!(load_quarantine_file(&paths.quarantine()).is_empty());

        let entries = r#"[{"scheme_id":"1111-2222","scheme_name":"Плохой","kind":"HardFreeze","reason":"зависание","at_ns":1,"plan_guid":"p"}]"#;
        std::fs::write(paths.quarantine(), entries.as_bytes()).unwrap();
        let loaded = load_quarantine_file(&paths.quarantine());
        assert_eq!(loaded.len(), 1, "карантин не прочитан: {loaded:?}");
        assert_eq!(loaded[0].scheme_id, "1111-2222");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Настройки, которые не разбираются, — отдельная находка: следующая
    /// же правка в интерфейсе перезапишет файл пустыми значениями.
    #[test]
    fn unparsable_settings_are_flagged() {
        let dir = std::env::temp_dir().join(format!("pb-diag-s-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = DataPaths { dir: dir.clone() };
        std::fs::write(paths.settings(), b"{ broken").unwrap();
        let mut findings = Findings::default();
        let data = collect_data(&paths, &mut findings);
        let joined = findings
            .items
            .iter()
            .map(|(_, t)| t.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("appsettings.json"), "нет находки: {joined}");
        assert!(
            data.state.iter().any(|(k, _)| k == "Настройки"),
            "в состоянии данных нет строки про настройки"
        );
        assert!(
            data.settings_json.is_none(),
            "битые настройки нельзя вставлять в отчёт как есть"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Отчёт без сессий и без схем обязан собираться: ровно этот случай
    /// приходит от нового пользователя, у которого ничего ещё не запускалось.
    #[test]
    fn report_builds_on_an_empty_data_dir() {
        let dir = std::env::temp_dir().join(format!("pb-diag-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = DataPaths { dir };
        let mut findings = Findings::default();
        let data = collect_data(&paths, &mut findings);
        assert!(
            data.state
                .iter()
                .any(|(k, v)| k == "Контрольная точка" && v == "нет"),
            "на пустом каталоге должно быть «контрольная точка: нет»: {:?}",
            data.state
        );
        let last = collect_last_session(&paths, &mut findings);
        assert!(
            last.is_none(),
            "на пустом каталоге последней сессии быть не может"
        );
    }

    /// Подсказки выдаются только под найденные проблемы, а не всем
    /// подряд: иначе пользователь игнорирует весь блок.
    #[test]
    fn hints_follow_found_problems() {
        let mut f = Findings::default();
        assert!(
            self_check_hints(&f).is_empty(),
            "без проблем подсказок быть не должно"
        );
        f.problem("powercfg не отвечает (таймаут)");
        let hints = self_check_hints(&f);
        assert!(hints.iter().any(|h| h.contains("powercfg /list")));
        f.problem("остался тестовый маркер замера");
        assert!(
            self_check_hints(&f)
                .iter()
                .any(|h| h.contains("перезагрузкой")),
            "для маркера должна быть подсказка про штатное прерывание"
        );
    }

    /// Дата в отчёте и дата в интерфейсе должны считаться одинаково,
    /// иначе записи из лога невозможно сопоставить с шапкой.
    #[test]
    fn time_of_handles_epoch_edge() {
        let epoch_label = time_of(1_000);
        assert!(
            epoch_label.starts_with("01.01.1970 0"),
            "метка эпохи должна быть разумной, получено: {epoch_label}"
        );
    }

    /// Имя файла отчёта должно быть уникальным на каждый вызов и безопасным
    /// для имени файла: без двоеточия из времени.
    #[test]
    fn default_file_name_has_no_colon() {
        let name = default_file_name();
        assert!(name.starts_with("PowerBench-Diagnostics-"));
        assert!(
            !name.contains(':'),
            "двоеточие недопустимо в имени файла: {name}"
        );
        assert!(name.ends_with(".txt"));
    }

    #[test]
    fn truncate_and_helpers_do_not_panic_on_odd_input() {
        assert_eq!(truncate("abcd", 10), "abcd");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(
            short_guid("381b4222-f694-41f0-9685-ff5bb260df2e"),
            "381b4222"
        );
        assert_eq!(short_hash("short"), "short");
        assert_eq!(short_hash(&"a".repeat(64)), format!("{}…", "a".repeat(12)));
    }
}
