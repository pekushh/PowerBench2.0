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
    /// Код ACPI-ограничения системы; 0 — ограничений нет.
    pub policy_reason: u32,
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
            on_ac: p.on_ac,
            unavailable: p.unavailable,
        }
    }

    /// Короткое описание для отчёта; `None`, если ограничений не было.
    pub fn note(&self) -> Option<String> {
        if self.unavailable {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if self.throttled {
            parts.push(format!(
                "троттлинг ({}/{} МГц)",
                self.current_mhz, self.max_mhz
            ));
        }
        if self.thermal_throttle {
            parts.push("термоограничение".to_string());
        }
        if self.policy_reason != 0 {
            parts.push(format!("ACPI-причина {}", self.policy_reason));
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
    /// Первые тики измеряемых фаз (Лёгкая/Тяжёлая/Отклик) — подпись детерминизма.
    pub first_tick_checksums: [u64; crate::config::PHASES_PER_RUN as usize],
    /// Итоговые контрольные суммы измеряемых фаз.
    pub run_checksums: [u64; crate::config::PHASES_PER_RUN as usize],
    /// Статистики измеряемых фаз в порядке «Лёгкая/Тяжёлая/Отклик».
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
}

/// Каталог данных приложения: `%LOCALAPPDATA%\PowerBench\`.
pub fn data_dir() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA") {
        Some(base) => PathBuf::from(base).join(crate::DATA_DIR_NAME),
        None => std::env::temp_dir().join(crate::DATA_DIR_NAME),
    }
}

/// Путь контрольной точки.
pub fn checkpoint_path() -> PathBuf {
    data_dir().join(CHECKPOINT_FILE_NAME)
}

/// Загрузить контрольную точку, если существует и читается.
pub fn load_checkpoint() -> Option<Checkpoint> {
    crate::storage::read_json(&checkpoint_path())
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
                },
                PhaseStats {
                    phase_index: 1,
                    stats: stats(10, 1.0),
                    power: None,
                },
                PhaseStats {
                    phase_index: 2,
                    stats: stats(10, 1.0),
                    power: None,
                },
            ],
            combined: stats(30, 1.0),
            cross_phase_consistency: 99.0,
            burst_retention_percent: 100.0,
            background: Vec::new(),
            spike_windows: 0,
            power: None,
            background_cpu_p50: 0.0,
            background_cpu_p95: 0.0,
            background_sample_seconds: 0,
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
        let _ = clear_checkpoint();
        let cp = Checkpoint::new(plan());
        save_checkpoint(&cp).unwrap();
        assert!(load_checkpoint().is_some());
        clear_checkpoint().unwrap();
        assert!(load_checkpoint().is_none());
        // Повторный вызов на отсутствующем файле — не ошибка.
        clear_checkpoint().unwrap();
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
        });
        assert_eq!(summary.stats.consistency_percent, 99.0);
        assert_eq!(summary.burst_retention_percent, 100.0);
        assert_eq!(summary.determinism.checksums, vec![1, 2, 3, 4]);
        assert_eq!(summary.duration_ms, 1000);
    }
}
