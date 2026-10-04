//! Контрольная точка сессии: атомарная запись на диск после каждого прогона
//! и после каждой браковки; чтение для «Продолжить» после прерывания.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use powerbench_metrics::run::RunStats;
use powerbench_windows::monitor::CorrelatedProcess;

use crate::config::SessionConfig;

/// Имя файла контрольной точки в каталоге данных.
pub const CHECKPOINT_FILE_NAME: &str = "benchmark-checkpoint.json";

/// Контрольная точка сессии.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Checkpoint {
    pub plan: SessionConfig,
    /// Ключи выполненных раундов `"{round}:{plan-guid}"` (строго по спецификации).
    /// Ключ добавляется, когда раунд отработан целиком; конкретные схемы раунда
    /// восстанавливаются из записей `runs` (уникальность по `round` + guid схемы).
    pub completed_keys: Vec<String>,
    pub runs: Vec<StoredRun>,
    /// guid схемы → причина браковки.
    pub rejections: BTreeMap<String, String>,
    /// Исходная активная схема (зафиксирована до первого применения тестовой).
    pub original_scheme_guid: Option<String>,
    /// Признак того, что исходная схема уже восстановлена.
    pub original_restored: bool,
}

impl Checkpoint {
    /// Новый пустой checkpoint под план.
    pub fn new(plan: SessionConfig) -> Self {
        Self {
            plan,
            completed_keys: Vec::new(),
            runs: Vec::new(),
            rejections: BTreeMap::new(),
            original_scheme_guid: None,
            original_restored: false,
        }
    }

    /// Ключ раунда выполнен? Сравнение ординарное, регистронезависимое.
    pub fn is_completed(&self, key: &str) -> bool {
        self.completed_keys
            .iter()
            .any(|k| k.eq_ignore_ascii_case(key))
    }

    /// Схема забракована? Сравнение по guid, регистронезависимое.
    pub fn is_rejected(&self, scheme_id: &str) -> bool {
        self.rejections
            .keys()
            .any(|k| k.eq_ignore_ascii_case(scheme_id))
    }

    /// Причина браковки схемы (регистронезависимый поиск по guid).
    pub fn rejection_reason(&self, scheme_id: &str) -> Option<&String> {
        self.rejections
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(scheme_id))
            .map(|(_, v)| v)
    }

    /// В раунде `round` уже есть запись прогона для схемы (по guid,
    /// регистронезависимо; имена схем для идентификации не используются).
    pub fn has_run(&self, round: u32, scheme_id: &str) -> bool {
        self.runs
            .iter()
            .any(|r| r.round == round && r.scheme_id.eq_ignore_ascii_case(scheme_id))
    }
}

/// Снимок ограничений питания на момент прогона.
///
/// Читается через `CallNtPowerInformation` (`powerbench_windows::power`).
/// Градусов температуры без драйвера не достать, но и не нужно: важнее, что
/// система ограничивала частоту и по какой причине. Нагрев смещает все
/// последующие прогоны сессии, а ротация порядка схем его не компенсирует —
/// поэтому признак пишется в каждый прогон.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub struct PowerSnapshot {
    /// Наибольшая разрешённая частота по ядрам, МГц.
    pub max_mhz: u32,
    /// Частота, которую система выдавала, МГц.
    pub current_mhz: u32,
    /// Частота была ограничена схемой питания или термограницей.
    pub throttled: bool,
    /// Причина ограничения — температура (пассивное охлаждение).
    pub thermal_throttle: bool,
    /// Флаги источника ограничения; 0 — ограничений нет (см.
    /// `powerbench_windows::power::throttle_cause`).
    pub policy_reason: u32,
    /// Сколько система выдержит без сна, минут (0 — неизвестно/без предела).
    #[serde(default)]
    pub max_idle_minutes: u32,
    /// Питание от сети.
    pub on_ac: bool,
    /// Снимок получить не удалось (нет данных, а не «ограничений нет»).
    pub unavailable: bool,
}

impl PowerSnapshot {
    /// Снимок из системного источника.
    pub fn capture() -> Self {
        let p = powerbench_windows::power::power_state();
        Self {
            max_mhz: p.max_mhz,
            current_mhz: p.current_mhz,
            throttled: p.throttled,
            thermal_throttle: p.thermal_throttle,
            policy_reason: p.policy_reason,
            max_idle_minutes: p.max_idle_minutes,
            on_ac: p.on_ac,
            unavailable: p.unavailable,
        }
    }
    /// Заметка о состоянии питания; `None`, если замечаний нет.
    ///
    /// Частота сюда не попадает намеренно. Снимок одной фазы ничего не знает о
    /// том, какой частота была в начале сессии, а сама частота и так видна в
    /// пофазной таблице вместе со стрелкой падения. Если печатать её здесь,
    /// в условиях замера появлялась строка на каждый прогон, и настоящие
    /// замечания — тепловая защита и троттлинг — в них терялись.
    pub fn note(&self) -> Option<String> {
        if self.unavailable {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if self.thermal_throttle {
            parts.push("тепловая защита".to_string());
        }
        // Регресс H22: здесь печаталось «ACPI-ограничение N», где N на самом
        // деле было `MaxIdlenessAllowed` — временем простоя до сна в единицах
        // по 64 с. Отчёт утверждал конкретную причину ограничения, которой
        // система не сообщала. Теперь перечисляются настоящие флаги
        // троттлинга.
        let causes = powerbench_windows::power::throttle_cause::describe(self.policy_reason);
        if !causes.is_empty() {
            parts.push(format!("троттлинг: {causes}"));
        }
        if self.max_idle_minutes > 0 && self.max_idle_minutes < 30 {
            parts.push(format!(
                "система уйдёт в сон после {} мин простоя",
                self.max_idle_minutes
            ));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(", "))
        }
    }
}

/// Статистика одной измеряемой фазы прогона.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PhaseStats {
    /// Индекс фазы: 0 = Лёгкая, 1 = Частичная, 2 = Тяжёлая, 3 = Отклик
    /// (`powerbench_core::config::Phase::index`).
    pub phase_index: u8,
    pub stats: RunStats,
    /// Состояние питания в конце фазы: троттлинг может начаться и закончиться
    /// внутри прогона, и по одному срезу в конце прогона это не поймать.
    #[serde(default)]
    pub power: Option<PowerSnapshot>,
    /// Номинальная длительность фазы, секунды.
    ///
    /// Без неё в отчёте нельзя отличить p0.001, посчитанный по трёмстам
    /// сэмплам, от p0.001 по пятидесяти тысячам: величина зависит от длины
    /// фазы, а длина в отчёте не была.
    #[serde(default)]
    pub seconds: u64,
}

/// Завершённый прогон одной схемы (компактная сводка для checkpoint).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredRun {
    pub key: String,
    pub round: u32,
    pub scheme_id: String,
    /// Отображаемое имя плана/схемы (PlanName). Только для вывода — никогда
    /// не используется для идентификации или матчинга.
    pub scheme_name: Option<String>,
    pub started_at_ns: u64,
    pub duration_ms: u64,
    pub ticks: u64,
    pub supercycles: u64,
    /// Первые тики измеряемых фаз (Лёгкая/Частичная/Тяжёлая/Отклик) — подпись детерминизма.
    pub first_tick_checksums: [u64; crate::config::PHASES_PER_RUN as usize],
    /// Итоговые контрольные суммы измеряемых фаз.
    pub run_checksums: [u64; crate::config::PHASES_PER_RUN as usize],
    /// Статистики измеряемых фаз в порядке «Лёгкая/Частичная/Тяжёлая/Отклик».
    pub phases: Vec<PhaseStats>,
    /// Объединённая статистика прогона (consistency — по фазам).
    pub combined: RunStats,
    /// ConsistencyPercent, посчитанный по группам фаз (взвешенный).
    pub cross_phase_consistency: f64,
    pub burst_retention_percent: f64,
    pub background: Vec<CorrelatedProcess>,
    pub spike_windows: usize,
    /// Снимок питания в конце прогона.
    #[serde(default)]
    pub power: Option<PowerSnapshot>,
    /// Снимок настроек схемы (`powercfg /query <guid>`) на момент прогона.
    ///
    /// Без него через год остаётся только GUID, а по нему не сказать, что
    /// было задано: Windows молча правит планы при обновлениях, OEM-агенты —
    /// постоянно. Дамп снимается один раз на прогон и делает результат
    /// воспроизводимым.
    ///
    /// Регресс M35: дамп писался в КАЖДЫЙ прогон, а чекпоинт переписывается
    /// целиком после каждого прогона. На типовой сессии (десятки схем ×
    /// повторы) это десятки копий одного и того же текста об одних и тех же
    /// настройках — мегабайты, которые переписывались после каждого прогона и
    /// уходили в итоговый JSON истории. Теперь дамп хранится ОДИН РАЗ на
    /// схему: у первого её прогона. Настройки схемы между повторами не меняются
    /// (их меняют между сессиями), так что копии были буквально одинаковыми, а
    /// воспроизводимость результата не пострадала: dump по GUID и раньше брался
    /// один, просто хранился в каждом прогоне.
    #[serde(default)]
    pub scheme_dump: Option<String>,
    /// Медианная фоновая нагрузка за прогон, % одного ядра.
    /// Раньше фон оценивался только пробой 700 мс *перед* прогоном, и весь
    /// прогон считался «чистым» или «грязным» по одному моменту.
    #[serde(default)]
    pub background_cpu_p50: f64,
    /// 95-й перцентиль той же фоновой нагрузки.
    #[serde(default)]
    pub background_cpu_p95: f64,
    /// Сколько секунд фона набралось (раз в секунду).
    #[serde(default)]
    pub background_sample_seconds: u32,
    // --- Условия запуска прогона ---
    //
    // Без них `resume` с другими параметрами перештамповывает старые прогоны
    // подписью ТЕКУЩЕГО движка, и в одном отчёте оказываются числа, измеренные
    // при разном числе воркеров, разной привязке и разной нагрузке. Такая
    // смесь ранжируется как один замер, а воспроизвести его нельзя.
    //
    // Все поля с `default`: контрольные точки, записанные прошлыми версиями,
    // читаются как «условия неизвестны» и отбраковываются на проверке.
    /// Число воркеров, под которыми шёл прогон (0 — неизвестно).
    #[serde(default)]
    pub worker_count: usize,
    /// Режим привязки потоков на момент прогона (пусто — неизвестно).
    #[serde(default)]
    pub affinity_mode: String,
    /// Подпись привязки: какие ядра достались воркерам.
    #[serde(default)]
    pub affinity_signature: String,
    /// Хэш конфигурации движка (профиль нагрузки, объёмы, версия).
    #[serde(default)]
    pub config_hash: String,
}

impl StoredRun {
    /// Условия запуска прогона в виде, пригодном для сравнения и показа.
    pub fn launch_conditions(&self) -> LaunchConditions {
        LaunchConditions {
            worker_count: self.worker_count,
            affinity_mode: self.affinity_mode.clone(),
            affinity_signature: self.affinity_signature.clone(),
            config_hash: self.config_hash.clone(),
        }
    }

    /// Известны ли условия запуска (у точки, записанной прошлой версией — нет).
    pub fn has_launch_conditions(&self) -> bool {
        self.worker_count > 0 || !self.config_hash.is_empty()
    }
}

/// Условия запуска прогона: чем именно он измерялся.
///
/// Отдельный тип нужен, чтобы сравнение подписей было одной операцией с
/// внятным сообщением, а не набором разрозненных проверок в цикле сессии.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchConditions {
    pub worker_count: usize,
    pub affinity_mode: String,
    pub affinity_signature: String,
    pub config_hash: String,
}

impl LaunchConditions {
    /// Первое поле, по которому условия разошлись, и как именно.
    ///
    /// Пустая строка — расхождений нет. Сравнение идёт по полям в порядке
    /// влияния на величину: число воркеров меняет саму работу, привязка —
    /// то, на каких ядрах она шла, конфигурация — что именно считалось.
    pub fn first_difference(&self, other: &LaunchConditions) -> Option<String> {
        // `worker_count == 0` означает «неизвестно» (точка прошлой версии), и
        // сравнивать его с настоящим значением нельзя: иначе любое чтение старой
        // точки превращалось бы в отказ продолжать сессию.
        if self.worker_count > 0 && other.worker_count > 0 && self.worker_count != other.worker_count
        {
            return Some(format!(
                "число воркеров {} против {}",
                self.worker_count, other.worker_count
            ));
        }
        if !self.affinity_mode.is_empty()
            && !other.affinity_mode.is_empty()
            && self.affinity_mode != other.affinity_mode
        {
            return Some(format!(
                "режим привязки «{}» против «{}»",
                self.affinity_mode, other.affinity_mode
            ));
        }
        if !self.affinity_signature.is_empty()
            && !other.affinity_signature.is_empty()
            && self.affinity_signature != other.affinity_signature
        {
            return Some(format!(
                "подпись привязки «{}» против «{}»",
                self.affinity_signature, other.affinity_signature
            ));
        }
        if !self.config_hash.is_empty()
            && !other.config_hash.is_empty()
            && self.config_hash != other.config_hash
        {
            return Some(format!(
                "конфигурация нагрузки {} против {}",
                self.config_hash, other.config_hash
            ));
        }
        None
    }

    /// Человекочитаемое описание для журнала сессии.
    pub fn describe(&self) -> String {
        format!(
            "{} воркер(ов), привязка «{}»/{}",
            self.worker_count, self.affinity_mode, self.affinity_signature
        )
    }
}

/// Каталог данных приложения: `%LOCALAPPDATA%\PowerBench\`.
pub fn data_dir() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA") {
        Some(base) => PathBuf::from(base).join(crate::DATA_DIR_NAME),
        None => std::env::temp_dir().join(crate::DATA_DIR_NAME),
    }
}

/// Общий замок тестов, которые трогают настоящий каталог данных.
///
/// `data_dir()` смотрит в `%LOCALAPPDATA%`, то есть тесты писали в папку
/// реального пользователя. Модули `checkpoint` и `recovery` использовали каждый
/// свой замок, поэтому при параллельном запуске один тест затирал чужую
/// контрольную точку: `already_restored_is_left_as_is` падал без видимой
/// причины, а у пользователя пропадала незавершённая сессия.
#[cfg(test)]
pub(crate) static DATA_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// Путь контрольной точки.
pub fn checkpoint_path() -> PathBuf {
    data_dir().join(CHECKPOINT_FILE_NAME)
}

/// Загрузить контрольную точку.
///
/// Возвращает ошибку, если файл существует, но не читается: прежняя версия
/// отдавала `None` и на «файла нет», и на «файл есть, но он битый», а вызов
/// `resume` молча начинал сессию с нуля, теряя честно отработанные раунды
/// без единого слова пользователю.
pub fn load_checkpoint() -> Result<Option<Checkpoint>, String> {
    load_checkpoint_from(&checkpoint_path())
}

/// То же, но для произвольного пути.
///
/// Отдельная функция нужна отчёту для поддержки: он обязан показать, что
/// контрольная точка не читается, и проверяется это на временном каталоге —
/// настоящий каталог данных пользователя тестами трогать нельзя.
pub fn load_checkpoint_from(path: &std::path::Path) -> Result<Option<Checkpoint>, String> {
    crate::storage::read_json_checked(path)
}

/// Сохранить контрольную точку: атомарная запись (временный файл + перемещение).
pub fn save_checkpoint(checkpoint: &Checkpoint) -> io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let bytes = serde_json::to_vec_pretty(checkpoint)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    crate::storage::atomic_write(&dir.join(CHECKPOINT_FILE_NAME), &bytes)
}

/// Удалить контрольную точку. Отсутствие файла — не ошибка.
///
/// Вызывается после успешного сохранения результата и перед стартом новой
/// сессии: без этого новый план всегда падал бы с `CheckpointPlanMismatch`,
/// потому что на диске лежит точка прошлого плана.
pub fn clear_checkpoint() -> io::Result<()> {
    crate::storage::remove_file_if_exists(&checkpoint_path())
}

impl StoredRun {
    /// Собрать `RunSummary` для агрегации: `stats` с пересчитанной
    /// cross-фазовой стабильностью, burst и сигнатуры из источника.
    pub fn to_summary(
        &self,
        signature: powerbench_metrics::CompatibilitySignature,
    ) -> powerbench_metrics::RunSummary {
        let mut stats = self.combined;
        stats.consistency_percent = self.cross_phase_consistency;
        let background_purity = {
            let correlated: f64 = self
                .background
                .iter()
                .map(|p| p.correlated_spike_windows as f64)
                .sum();
            let total = self.spike_windows as f64;
            if total > 0.0 {
                Some((1.0 - (correlated / total).min(1.0)) * 100.0)
            } else {
                None
            }
        };
        powerbench_metrics::RunSummary {
            signature,
            determinism: powerbench_metrics::DeterminismSignature::new(
                self.first_tick_checksums.to_vec(),
            ),
            stats,
            burst_retention_percent: self.burst_retention_percent,
            background_purity,
            started_at_ns: self.started_at_ns,
            duration_ms: self.duration_ms,
        }
    }
}

/// Атомарная запись файла: пишем во временный файл рядом и перемещаем.
/// Уникальное имя временного файла на запись (см. `storage::atomic_write`).
pub use crate::storage::atomic_write;

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> SessionConfig {
        SessionConfig {
            duration_seconds: 9,
            warmup_seconds: 2,
            cooling_seconds: 1,
            repetitions: 1,
            background_threshold_percent: 5.0,
            accept_dirty_background: false,
            worker_count: None,
            scheme_ids: vec!["s1".to_string()],
            plan_guid: "g1".to_string(),
            reference_scheme_id: None,
        }
    }

    fn stats(samples: usize, avg: f64) -> RunStats {
        let mut s: RunStats = powerbench_metrics::run::run_stats(&[1.0; 4]).unwrap();
        s.samples = samples;
        s.average_throughput = avg;
        s
    }

    fn run(key: &str) -> StoredRun {
        StoredRun {
            key: key.to_string(),
            round: 0,
            scheme_id: "S1".to_string(),
            scheme_name: Some("План One".to_string()),
            started_at_ns: 1,
            duration_ms: 1000,
            ticks: 100,
            supercycles: 0,
            first_tick_checksums: [1, 2, 3, 4],
            run_checksums: [4, 5, 6, 7],
            phases: vec![
                PhaseStats {
                    phase_index: 0,
                    stats: stats(10, 1.0),
                    power: None,
                    seconds: 4,
                },
                PhaseStats {
                    phase_index: 1,
                    stats: stats(10, 1.0),
                    power: None,
                    seconds: 4,
                },
                PhaseStats {
                    phase_index: 2,
                    stats: stats(10, 1.0),
                    power: None,
                    seconds: 4,
                },
            ],
            combined: stats(30, 1.0),
            cross_phase_consistency: 99.0,
            burst_retention_percent: 100.0,
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

    #[test]
    fn checkpoint_roundtrip_via_atomic_write() {
        let dir = std::env::temp_dir().join("powerbench-checkpoint-test");
        let _ = std::fs::remove_dir_all(&dir);
        let json_path = dir.join(CHECKPOINT_FILE_NAME);
        let mut cp = Checkpoint::new(plan());
        cp.completed_keys.push("0:g1".to_string());
        cp.runs.push(run("0:g1"));
        atomic_write(&json_path, &serde_json::to_vec(&cp).unwrap()).unwrap();
        let text = std::fs::read_to_string(&json_path).unwrap();
        let loaded: Checkpoint = serde_json::from_str(&text).unwrap();
        assert_eq!(loaded, cp);
        assert!(loaded.is_completed("0:g1"));
        assert!(!loaded.is_rejected("s1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkpoint_membership_checks() {
        let mut cp = Checkpoint::new(plan());
        assert!(!cp.is_completed("0:g1"));
        assert!(!cp.is_rejected("s1"));
        assert!(!cp.has_run(0, "s1"));
        cp.completed_keys.push("0:g1".to_string());
        cp.rejections.insert(
            "s9".to_string(),
            "нет прогресса более 30 секунд".to_string(),
        );
        cp.runs.push(run("0:g1"));
        assert!(cp.is_completed("0:g1"));
        assert!(cp.is_rejected("s9"));
        assert!(!cp.is_rejected("s1"));
        assert!(cp.has_run(0, "s1"));
        // Сравнение ординарное и регистронезависимое (ключи и guid схемы).
        assert!(cp.is_completed("0:G1"));
        assert!(cp.is_rejected("S9"));
        assert!(cp.has_run(0, "s1"));
        assert!(cp.has_run(0, "S1"));
        assert!(!cp.has_run(1, "s1"));
        assert_eq!(
            cp.rejection_reason("S9").map(String::as_str),
            Some("нет прогресса более 30 секунд")
        );
        assert_eq!(cp.rejection_reason("s1"), None);
        // Имя схемы — только отображение, не участвует в матчинге.
        assert!(cp.has_run(0, "S1"));
    }

    #[test]
    fn clear_checkpoint_is_idempotent() {
        // Регресс: без очистки новый план падал бы с CheckpointPlanMismatch.
        // Пишем в настоящий каталог данных: без общего с recovery замка
        // тесты затирают чужую контрольную точку при параллельном запуске.
        let _guard = DATA_DIR_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = clear_checkpoint();
        let cp = Checkpoint::new(plan());
        save_checkpoint(&cp).unwrap();
        assert!(load_checkpoint().expect("читается").is_some());
        clear_checkpoint().unwrap();
        assert!(load_checkpoint().expect("читается").is_none());
        // Повторный вызов на отсутствующем файле — не ошибка.
        clear_checkpoint().unwrap();
    }

    /// Битый JSON обязан быть ошибкой, а не «сессии не было»: раньше `resume`
    /// на таком файле молча начинал заново и терял честно отработанные раунды.
    #[test]
    fn corrupt_checkpoint_is_an_error_not_absence() {
        let _guard = DATA_DIR_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = clear_checkpoint();
        std::fs::create_dir_all(data_dir()).unwrap();
        std::fs::write(checkpoint_path(), b"{ not json at all").unwrap();
        let err = load_checkpoint().expect_err("битый файл обязан сообщить об ошибке");
        assert!(
            err.contains("checkpoint") || err.contains("JSON") || err.contains("json"),
            "сообщение должно называть файл и проблему, получено: {err}"
        );
        let _ = clear_checkpoint();
    }

    /// Пустой файл — ещё не сессия (обрыв записи в ноль байт), но и не ошибка
    /// чтения: продолжать можно.
    #[test]
    fn empty_checkpoint_is_treated_as_absent() {
        let _guard = DATA_DIR_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = clear_checkpoint();
        std::fs::create_dir_all(data_dir()).unwrap();
        std::fs::write(checkpoint_path(), b"").unwrap();
        assert!(
            load_checkpoint().expect("пустой файл не ошибка").is_none(),
            "пустой файл не должен считаться сессией"
        );
        let _ = clear_checkpoint();
    }

    #[test]
    fn summary_rebuild_overrides_consistency() {
        let r = run("0:g1");
        let summary = r.to_summary(powerbench_metrics::CompatibilitySignature {
            workload_version: "GamingCpuV1".to_string(),
            config_hash: "H".to_string(),
            seed: 1,
            worker_count: 4,
            logical_cpus: 6,
            timer_hz: 10_000_000,
            cpu_identifier: "cpu".to_string(),
            diagnostics_version: "0.1.0".to_string(),
            affinity_mode: "p-only".to_string(),
            affinity_signature: "p-only:test".to_string(),
        });
        assert_eq!(summary.stats.consistency_percent, 99.0);
        assert_eq!(summary.burst_retention_percent, 100.0);
        assert_eq!(summary.determinism.checksums, vec![1, 2, 3, 4]);
        assert_eq!(summary.duration_ms, 1000);
    }
}
