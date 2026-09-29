//! История завершённых сессий (Этап 6): каталог `Results\`, сохранение
//! полного JSON-результата после завершённой сессии, список и чтение записей
//! (включая старые несовместимые — открываются без паники), экспорт в JSON/CSV
//! со всеми обязательными полями раздела «Хранение», проверка совместимости
//! сигнатур для ранжирования (записи с другой версией/хэшем/seed в сравнение
//! не входят).

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::checkpoint::StoredRun;
use crate::checkpoint::{atomic_write, data_dir};
use crate::result::{IdentityJson, RecommendationJson, SchemeJson, SessionJson};

/// Имя каталога истории завершённых сессий в каталоге данных.
pub const RESULTS_DIR_NAME: &str = "Results";

/// Заголовок CSV-экспорта (одна строка на прогон; все обязательные поля
/// раздела «Хранение» присутствуют в шапке).
pub const CSV_HEADER: &[&str] = &[
    "workload_version",
    "config_hash",
    "seed_hex",
    "worker_count",
    "logical_cpus",
    "timer_hz",
    "cpu_identifier",
    "diagnostics_version",
    "plan_guid",
    "original_scheme_guid",
    "original_restored",
    // Компактная сводка рекомендаций.
    "recommendation_level",
    "recommendation_level_label",
    "recommendation_reason",
    "recommended_scheme",
    "runner_up_scheme",
    "p_best",
    "p_margin_gt_0",
    "p_margin_gt_1pct",
    "expected_margin_percent",
    // Схема и прогон.
    "scheme_id",
    "scheme_name",
    "rejected",
    "runs",
    "mean_average_throughput",
    "run_variation_percent",
    "run_key",
    "round",
    "started_at_ns",
    "started_at_utc",
    "duration_ms",
    "ticks",
    "supercycles",
    "first_tick_checksums",
    "run_checksums",
    "average_throughput",
    "median_throughput",
    "p1_throughput",
    "p01_throughput",
    "average_execution_time_ms",
    "p95_execution_time_ms",
    "p99_execution_time_ms",
    "consistency_percent",
    "jitter_p99_ms",
    "burst_retention_percent",
    "spike_windows",
    "background_correlations",
];

/// Запись истории: имя файла и путь.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub file_name: String,
    pub path: PathBuf,
}

/// Каталог истории (`%LOCALAPPDATA%\PowerBench\Results\`).
pub fn results_dir() -> PathBuf {
    data_dir().join(RESULTS_DIR_NAME)
}

/// Время старта сессии (UTC, нс с эпохи) — начало самого раннего прогона.
pub fn session_started_at_ns(session: &SessionJson) -> Option<u64> {
    session
        .schemes
        .iter()
        .flat_map(|s| s.per_run.iter())
        .map(|r| r.started_at_ns)
        .min()
}

/// Прочитать результат из файла истории. Старые записи с другой версией
/// нагрузки открываются без паники (отсутствующие новые поля опциональны);
/// ошибка формата возвращается как `Err` — вызывающий решает, пропускать ли.
///
/// Имена схем, записанные до исправления декодирования, чинятся на лету:
/// `powercfg` отдавал UTF-8, а прежний код читал его как CP866, из-за чего
/// «Сбалансированная» превращалась в «╨б╨▒╨░╨╗╨░╨╜». Ошибка обратима,
/// поэтому старые сессии показываются читаемо без ручной правки файлов.
pub fn load_result(path: &Path) -> Result<SessionJson, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut session: SessionJson = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    repair_scheme_names(&mut session);
    Ok(session)
}

/// Починить имена схем в загруженной сессии.
fn repair_scheme_names(session: &mut SessionJson) {
    for sch in &mut session.schemes {
        if let Some(name) = sch.name.take() {
            sch.name = Some(
                powerbench_windows::power::repair_mojibake(&name).unwrap_or(name),
            );
        }
        for run in &mut sch.per_run {
            if let Some(name) = run.scheme_name.take() {
                run.scheme_name =
                    Some(powerbench_windows::power::repair_mojibake(&name).unwrap_or(name));
            }
        }
    }
}

/// Совместимы ли две идентичности для совместного агрегирования/ранжирования:
/// версия нагрузки, хэш конфигурации, seed, число воркеров и логических CPU,
/// частота таймера, идентификатор процессора, версия диагностики — все равны.
pub fn same_identity(a: &IdentityJson, b: &IdentityJson) -> bool {
    a.workload_version == b.workload_version
        && a.config_hash == b.config_hash
        && a.seed_hex == b.seed_hex
        && a.worker_count == b.worker_count
        && a.logical_cpus == b.logical_cpus
        && a.timer_hz == b.timer_hz
        && a.cpu_identifier == b.cpu_identifier
        && a.diagnostics_version == b.diagnostics_version
}

/// Записи, совместимые с базовой идентичностью (входят в сравнение).
pub fn rankable_with<'a>(all: &'a [SessionJson], base: &IdentityJson) -> Vec<&'a SessionJson> {
    all.iter()
        .filter(|s| same_identity(&s.identity, base))
        .collect()
}

/// Безопасное имя файла из произвольной строки (GUID/`plan-…`).
pub fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Сохранить завершённую сессию в историю (атомарно). Имя файла:
/// `{UTC-старт}_{plan_guid}.json`; при коллизии добавляется `_2`, `_3`, …
pub fn save_result(session: &SessionJson) -> io::Result<PathBuf> {
    save_result_in(session, &results_dir())
}

/// То же, но в указанный каталог (для тестов).
pub fn save_result_in(session: &SessionJson, dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let base = date_time_stamp(session_started_at_ns(session).unwrap_or_else(now_unix_ns));
    let plan_part = sanitize(&session.plan_guid);
    let mut candidate = dir.join(format!("{base}_{plan_part}.json"));
    let mut suffix = 2u32;
    while candidate.exists() {
        candidate = dir.join(format!("{base}_{plan_part}_{suffix}.json"));
        suffix += 1;
    }
    let bytes = serde_json::to_vec_pretty(session)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    atomic_write(&candidate, &bytes)?;
    Ok(candidate)
}

/// Список записей истории по убыванию времени (по имени файла: метка UTC).
pub fn list_results() -> io::Result<Vec<HistoryEntry>> {
    list_results_in(&results_dir())
}

/// То же, но для указанного каталога (для тестов).
pub fn list_results_in(dir: &Path) -> io::Result<Vec<HistoryEntry>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<HistoryEntry> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".json")
        })
        .map(|e| HistoryEntry {
            file_name: e.file_name().to_string_lossy().into_owned(),
            path: e.path(),
        })
        .collect();
    entries.sort_by(|a, b| b.file_name.cmp(&a.file_name));
    Ok(entries)
}

/// Экспорт результата в JSON (pretty-print, атомарная запись).
pub fn export_json(session: &SessionJson, path: &Path) -> io::Result<()> {
    // У голого имени файла parent() даёт Some("") — фильтруем, иначе упадём.
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(session)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    atomic_write(path, &bytes)
}

/// Экспорт результата в CSV — одна строка на прогон, все обязательные поля.
pub fn export_csv(session: &SessionJson, out: &mut impl Write) -> Result<usize, String> {
    let mut wtr = csv::Writer::from_writer(out);
    wtr.write_record(CSV_HEADER).map_err(|e| e.to_string())?;
    let mut rows = 0usize;
    for scheme in &session.schemes {
        for run in &scheme.per_run {
            let record = csv_row(session, scheme, run);
            wtr.write_record(record).map_err(|e| e.to_string())?;
            rows += 1;
        }
    }
    wtr.flush().map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Экспорт результата в файл CSV.
///
/// Запись атомарная: сначала во временный файл рядом, затем переименование.
/// Раньше файл создавался сразу и заполнялся на месте, поэтому прерванный
/// экспорт оставлял после себя обрезанный CSV, который нельзя отличить от
/// корректного, но уже неполного.
pub fn export_csv_to(session: &SessionJson, path: &Path) -> Result<usize, String> {
    // У голого имени файла parent() даёт Some("") — фильтруем, иначе упадём.
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut buf: Vec<u8> = Vec::new();
    let rows = export_csv(session, &mut buf)?;
    atomic_write(path, &buf).map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Одна CSV-строка (прогон) с полями обязательного набора.
fn csv_row(session: &SessionJson, scheme: &SchemeJson, run: &StoredRun) -> Vec<String> {
    let id = &session.identity;
    let rec = &session.recommendation;
    let combined = &run.combined;
    vec![
        id.workload_version.clone(),
        id.config_hash.clone(),
        id.seed_hex.clone(),
        id.worker_count.to_string(),
        id.logical_cpus.to_string(),
        id.timer_hz.to_string(),
        id.cpu_identifier.clone(),
        id.diagnostics_version.clone(),
        session.plan_guid.clone(),
        session.original_scheme_guid.clone().unwrap_or_default(),
        session.original_restored.to_string(),
        rec.level.clone(),
        rec.level_label.clone(),
        rec.reason.clone(),
        rec.recommended_scheme.clone().unwrap_or_default(),
        rec.runner_up_scheme.clone().unwrap_or_default(),
        probability(rec, 0),
        probability(rec, 1),
        probability(rec, 2),
        rec.expected_margin_percent
            .map(|m| format!("{m:.6}"))
            .unwrap_or_default(),
        scheme.scheme_id.clone(),
        scheme.name.clone().unwrap_or_default(),
        scheme.rejected.to_string(),
        scheme.runs.to_string(),
        scheme.mean_average_throughput.to_string(),
        scheme.run_variation_percent.to_string(),
        run.key.clone(),
        run.round.to_string(),
        run.started_at_ns.to_string(),
        date_time_stamp(run.started_at_ns),
        run.duration_ms.to_string(),
        run.ticks.to_string(),
        run.supercycles.to_string(),
        checksums_hex(run.first_tick_checksums),
        checksums_hex(run.run_checksums),
        combined.average_throughput.to_string(),
        combined.median_throughput.to_string(),
        combined.p1_throughput.to_string(),
        combined.p01_throughput.to_string(),
        combined.average_execution_time_ms.to_string(),
        combined.p95_execution_time_ms.to_string(),
        combined.p99_execution_time_ms.to_string(),
        combined.consistency_percent.to_string(),
        combined.jitter_p99_ms.to_string(),
        run.burst_retention_percent.to_string(),
        run.spike_windows.to_string(),
        run.background.len().to_string(),
    ]
}

fn probability(rec: &RecommendationJson, i: usize) -> String {
    rec.probabilities
        .map(|p| format!("{:.4}", p[i]))
        .unwrap_or_default()
}

fn checksums_hex(a: [u64; crate::config::PHASES_PER_RUN as usize]) -> String {
    a.map(|v| format!("{v:016X}")).join(",")
}

/// UTC-метка `YYYYMMDDTHHMMSSZ.###` из наносекунд эпохи.
pub fn date_time_stamp(ns: u64) -> String {
    let secs = ns / 1_000_000_000;
    let millis = (ns % 1_000_000_000) / 1_000_000;
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let hh = sod / 3600;
    let mi = (sod % 3600) / 60;
    let ss = sod % 60;
    format!("{y:04}{m:02}{d:02}T{hh:02}{mi:02}{ss:02}Z{millis:03}")
}

/// Календарная дата `(год, месяц, день)` по числу дней с эпохи (алгоритм
/// Howard Hinnant `civil_from_days`; простой, без внешних зависимостей).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn now_unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Минимальный результат для тестов.
    fn sample_session(seed: &str) -> SessionJson {
        SessionJson {
            plan_guid: "plan-test".to_string(),
            original_scheme_guid: Some("orig-guid".to_string()),
            original_restored: true,
            identity: IdentityJson {
                workload_version: "GamingCpuV1".to_string(),
                config_hash: "HASH".to_string(),
                seed_hex: seed.to_string(),
                worker_count: 4,
                logical_cpus: 8,
                timer_hz: 10_000_000,
                cpu_identifier: "cpu".to_string(),
                diagnostics_version: "0.1.0".to_string(),
                os_build: String::new(),
                memory_gib: 0.0,
                cpu_brand: String::new(),
            },
            schemes: vec![SchemeJson::from_aggregate(
                "s1".to_string(),
                false,
                None,
                &empty_aggregate(),
                vec![test_run(0, 1_700_000_000_000_000_000)],
            )],
            recommendation: RecommendationJson {
                level: "Probable".to_string(),
                level_label: "Вероятно".to_string(),
                recommended_scheme: Some("s1".to_string()),
                runner_up_scheme: None,
                reason: "тест".to_string(),
                probabilities: Some([0.9, 0.8, 0.7]),
                expected_margin_percent: Some(1.5),
                bootstrap_mode: Some("PairedByRun".to_string()),
                tie_criterion: Some("P1".to_string()),
            },
            warnings: Vec::new(),
            rounds_planned: 1,
            rounds_completed: 1,
            early_stop_reason: None,
            score_weights: crate::result::default_score_weights(),
            reference: None,
            screening: false,
        }
    }

    fn empty_aggregate() -> powerbench_metrics::AggregateResult {
        powerbench_metrics::AggregateResult {
            runs: 1,
            mean_average_throughput: 100.0,
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

    fn test_run(round: u32, started: u64) -> StoredRun {
        let mut s: powerbench_metrics::run::RunStats =
            powerbench_metrics::run::run_stats(&[1.0, 2.0, 3.0, 4.0]).unwrap();
        s.average_throughput = 100.0;
        StoredRun {
            key: format!("{round}:plan-test"),
            round,
            scheme_id: "s1".to_string(),
            scheme_name: Some("План Один".to_string()),
            started_at_ns: started,
            duration_ms: 1000,
            ticks: 100,
            supercycles: 1,
            first_tick_checksums: [1, 2, 3, 4],
            run_checksums: [4, 5, 6, 7],
            phases: Vec::new(),
            combined: s,
            cross_phase_consistency: 95.0,
            burst_retention_percent: 90.0,
            background: Vec::new(),
            spike_windows: 0,
            power: None,
            background_cpu_p50: 0.0,
            background_cpu_p95: 0.0,
            background_sample_seconds: 0,
        }
    }

    /// Семантическое равенство результатов: все обязательные поля совпадают;
    /// безчувственно к последнему биту f64 (json-круг возвращает кратчайшую
    /// форму представления, битовая форма может отличаться на ULP).
    fn same_semantics(a: &SessionJson, b: &SessionJson) -> bool {
        a.plan_guid == b.plan_guid
            && a.original_scheme_guid == b.original_scheme_guid
            && a.original_restored == b.original_restored
            && a.identity == b.identity
            && a.warnings == b.warnings
            && a.recommendation.level == b.recommendation.level
            && a.recommendation.level_label == b.recommendation.level_label
            && a.recommendation.reason == b.recommendation.reason
            && a.recommendation.recommended_scheme == b.recommendation.recommended_scheme
            && a.recommendation.runner_up_scheme == b.recommendation.runner_up_scheme
            && a.recommendation
                .expected_margin_percent
                .map(|v| (v * 1e9) as i64)
                == b.recommendation
                    .expected_margin_percent
                    .map(|v| (v * 1e9) as i64)
            && match (
                &a.recommendation.probabilities,
                &b.recommendation.probabilities,
            ) {
                (Some(x), Some(y)) => (0..3).all(|i| (x[i] - y[i]).abs() < 1e-9),
                (None, None) => true,
                _ => false,
            }
            && a.schemes.len() == b.schemes.len()
            && a.schemes
                .iter()
                .zip(b.schemes.iter())
                .all(|(sa, sb)| same_scheme(sa, sb))
    }

    fn same_scheme(a: &SchemeJson, b: &SchemeJson) -> bool {
        a.scheme_id == b.scheme_id
            && a.name == b.name
            && a.rejected == b.rejected
            && a.rejection_reason == b.rejection_reason
            && a.runs == b.runs
            && (a.mean_average_throughput.is_nan() && b.mean_average_throughput.is_nan()
                || (a.mean_average_throughput - b.mean_average_throughput).abs() < 1e-9)
            && a.per_run.len() == b.per_run.len()
            && a.per_run.iter().zip(b.per_run.iter()).all(|(ra, rb)| {
                ra.key == rb.key
                    && ra.round == rb.round
                    && ra.scheme_id == rb.scheme_id
                    && ra.scheme_name == rb.scheme_name
                    && ra.started_at_ns == rb.started_at_ns
                    && ra.duration_ms == rb.duration_ms
                    && ra.ticks == rb.ticks
                    && ra.supercycles == rb.supercycles
                    && ra.first_tick_checksums == rb.first_tick_checksums
                    && ra.run_checksums == rb.run_checksums
                    && ra.cross_phase_consistency == rb.cross_phase_consistency
                    && ra.burst_retention_percent == rb.burst_retention_percent
                    && ra.spike_windows == rb.spike_windows
            })
    }

    #[test]
    fn civil_days_known_bases() {
        // 1970-01-01, 2000-01-01, 2026-09-08.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10957), (2000, 1, 1));
        assert_eq!(civil_from_days(20704), (2026, 9, 8));
    }

    #[test]
    fn date_time_stamp_format() {
        // 1_700_000_000_000_000_000 нс = 2023-11-14T22:13:20.000Z.
        let stamp = date_time_stamp(1_700_000_000_000_000_000);
        assert_eq!(&stamp[..8], "20231114");
        assert_eq!(&stamp[9..15], "221320");
        assert!(stamp.ends_with("Z000"));
    }

    #[test]
    fn save_then_list_then_load_roundtrip() {
        let dir = std::env::temp_dir().join("powerbench-history-save-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let session = sample_session("DEADBEEF");
        let saved = save_result_in(&session, &dir).unwrap();
        assert!(saved.exists());
        let entries = list_results_in(&dir).unwrap();
        assert!(entries.iter().any(|e| e.path == saved));
        let loaded = load_result(&saved).unwrap();
        assert!(
            same_semantics(&session, &loaded),
            "запись искажена при сериализации"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_incompatible_record_opens_but_is_not_rankable() {
        let a = sample_session("AAAA0001");
        let mut b = sample_session("BBBB0002");
        b.identity.workload_version = "GamingCpuV2".to_string();
        let all = vec![a.clone(), b.clone()];
        // Обе открываются (load_result уже проверен), но в сравнение входят
        // только записи с той же идентичностью.
        let rankable = rankable_with(&all, &a.identity);
        assert_eq!(rankable.len(), 1);
        assert_eq!(rankable[0].identity.seed_hex, "AAAA0001");
    }

    #[test]
    fn malformed_file_returns_err_without_panic() {
        let dir = std::env::temp_dir().join("powerbench-history-bad-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.json");
        std::fs::write(&path, "{ not json ").unwrap();
        assert!(load_result(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csv_export_contains_all_mandatory_columns() {
        let session = sample_session("12345678");
        let mut buf = Vec::new();
        let rows = export_csv(&session, &mut buf).unwrap();
        assert_eq!(rows, 1);
        let text = String::from_utf8(buf).unwrap();
        let header = text.lines().next().unwrap();
        for col in CSV_HEADER {
            assert!(header.contains(col), "в шапке CSV нет столбца {col}");
        }
        // Обязательное содержание присутствует в первой строке данных.
        assert!(text.contains("GamingCpuV1"));
        assert!(text.contains("12345678"));
        assert!(text.contains("0000000000000001,0000000000000002,0000000000000003"));
        assert!(text.contains("0000000000000004,0000000000000005,0000000000000006"));
        assert!(text.contains("Probable"));
        assert!(text.contains("План Один"));
        assert!(text.contains("s1"));
    }

    #[test]
    fn export_json_matches_load() {
        let dir = std::env::temp_dir().join("powerbench-history-exp-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("export.json");
        let session = sample_session("CAFE0001");
        export_json(&session, &path).unwrap();
        let loaded = load_result(&path).unwrap();
        assert!(
            same_semantics(&session, &loaded),
            "JSON-экспорт искажает запись"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
