//! Агрегация нескольких прогонов одной схемы.
//!
//! Запрещено объединять прогоны с разными: типом/версией нагрузки, хэшем
//! конфигурации, seed, числом воркеров, числом логических CPU, частотой
//! таймера, идентификатором процессора, версией диагностики, единицей метрики.
//! Для GamingCpuV1 дополнительно должна совпадать «подпись детерминизма»
//! (контрольные суммы фаз); различие → ошибка «контрольная сумма различается
//! между повторами».

use std::fmt;

use crate::run::median;

/// Порог CV повторов: больше 3% — предупреждение «результат желательно
/// перепроверить».
pub const CV_WARNING_PERCENT: f64 = 3.0;

/// Таблица t95 по степеням свободы df (спецификация, раздел «Агрегация»).
pub fn t95(df: usize) -> f64 {
    match df {
        0 | 1 => 12.706,
        2 => 4.303,
        3 => 3.182,
        4 => 2.776,
        5 => 2.571,
        6 => 2.447,
        7 => 2.365,
        8 => 2.306,
        9 => 2.262,
        10 => 2.228,
        11..=15 => 2.131,
        16..=20 => 2.086,
        21..=30 => 2.042,
        _ => 1.96,
    }
}

/// Сигнатура совместимости: прогоны с разными полями агрегировать нельзя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilitySignature {
    /// Тип/версия нагрузки (например, "GamingCpuV1").
    pub workload_version: String,
    /// Хэш конфигурации нагрузки.
    pub config_hash: String,
    /// Seed генератора случайных чисел.
    pub seed: u64,
    /// Число воркеров пула.
    pub worker_count: usize,
    /// Число логических процессоров.
    pub logical_cpus: usize,
    /// Частота таймера (QPC), Гц.
    pub timer_hz: u64,
    /// Идентификатор процессора.
    pub cpu_identifier: String,
    /// Версия диагностики.
    pub diagnostics_version: String,
}

/// Подпись детерминизма GamingCpuV1: контрольные суммы фаз.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeterminismSignature {
    /// Контрольные суммы фаз (состав задаётся источником данных).
    pub checksums: Vec<u64>,
}

impl DeterminismSignature {
    pub fn new(checksums: Vec<u64>) -> Self {
        Self { checksums }
    }
}

/// Сводка одного завершённого прогона для агрегации.
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub signature: CompatibilitySignature,
    pub determinism: DeterminismSignature,
    pub stats: crate::run::RunStats,
    pub burst_retention_percent: f64,
    /// Чистота фона прогона (%) — 100 при отсутствии коррелированных процессов.
    pub background_purity: Option<f64>,
    /// Время старта прогона (UTC, наносекунды от эпохи) — для минимума.
    pub started_at_ns: u64,
    /// Длительность прогона в мс — для суммы.
    pub duration_ms: u64,
}

/// Ошибка агрегации.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregateError {
    /// Нет ни одного прогона.
    EmptyRuns,
    /// Несовместимые сигнатуры (разные версия/хэш/seed/воркеры/CPU/таймер…).
    SignatureMismatch,
    /// Подписи детерминизма различаются.
    ChecksumMismatch,
}

impl fmt::Display for AggregateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AggregateError::EmptyRuns => write!(f, "нет ни одного прогона для агрегации"),
            AggregateError::SignatureMismatch => {
                write!(
                    f,
                    "несовместимые сигнатуры: запрещено объединять такие прогоны"
                )
            }
            AggregateError::ChecksumMismatch => {
                write!(f, "контрольная сумма различается между повторами")
            }
        }
    }
}

/// Результат агрегации прогонов одной схемы.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateResult {
    /// Число прогонов k.
    pub runs: usize,
    /// Среднее арифметическое средних throughput прогонов.
    pub mean_average_throughput: f64,
    /// sample_std: `sqrt(Σ(x − mean)²/(k−1))`; при k = 1 → 0.
    pub sample_std: f64,
    /// Коэффициент t95(k−1).
    pub t_value: f64,
    /// margin = t95(k−1) * sample_std / √k; при k = 1 → 0.
    pub margin: f64,
    /// ДИ 95% = [max(0, mean − margin); mean + margin].
    pub ci_95: [f64; 2],
    /// RunVariationPercent = sample_std/mean*100.
    pub run_variation_percent: f64,
    /// Предупреждение «результат желательно перепроверить» (> 3%).
    pub cv_warning: bool,
    /// Медианы по прогонам (для всех показателей кроме среднего).
    pub median_throughput: f64,
    pub median_p1_throughput: f64,
    pub median_p01_throughput: f64,
    pub median_p95_execution_time_ms: f64,
    pub median_p99_execution_time_ms: f64,
    pub median_consistency_percent: f64,
    pub median_burst_retention_percent: f64,
    pub median_jitter_p99_ms: f64,
    /// Медиана худших секунд прогонов (минимальный AverageThroughput по окнам 1 с).
    pub median_worst_window_throughput: f64,
    /// Медиана чистоты фона по прогонам (None — данные недоступны).
    pub median_background_purity: Option<f64>,
    /// RunDuration агрегата = сумма длительностей прогонов.
    pub run_duration_ms: u64,
    /// RunStartedAtUtc агрегата = минимальное время старта прогонов.
    pub started_at_min_ns: u64,
}

/// Агрегировать прогоны одной схемы. Возвращает ошибку при несовместимых
/// сигнатурах или расходящихся подписях детерминизма.
pub fn aggregate_runs(runs: &[RunSummary]) -> Result<AggregateResult, AggregateError> {
    if runs.is_empty() {
        return Err(AggregateError::EmptyRuns);
    }

    let first_sig = &runs[0].signature;
    for run in &runs[1..] {
        if &run.signature != first_sig {
            return Err(AggregateError::SignatureMismatch);
        }
    }
    let first_det = &runs[0].determinism;
    for run in &runs[1..] {
        if &run.determinism != first_det {
            return Err(AggregateError::ChecksumMismatch);
        }
    }

    // Средние throughput прогонов, отсортированы по возрастанию.
    let mut averages: Vec<f64> = runs.iter().map(|r| r.stats.average_throughput).collect();
    averages.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let k = runs.len();
    let mean = averages.iter().sum::<f64>() / k as f64;
    let sample_std = if k == 1 {
        0.0
    } else {
        (averages.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (k - 1) as f64).sqrt()
    };
    // При k = 1 степеней свободы нет: доверительного интервала не бывает,
    // margin = 0, и t-множитель тоже не имеет смысла. Раньше туда попадало
    // t95(0) = 12.0 — правдоподобное число, означающее ровно ничего.
    let t_value = if k >= 2 { t95(k - 1) } else { 0.0 };
    let margin = if k == 1 {
        0.0
    } else {
        t_value * sample_std / (k as f64).sqrt()
    };
    let ci_low = (mean - margin).max(0.0);
    let ci_high = mean + margin;
    let run_variation_percent = if mean > 0.0 {
        sample_std / mean * 100.0
    } else {
        0.0
    };

    let med = |f: fn(&crate::run::RunStats) -> f64| -> f64 {
        let values: Vec<f64> = runs.iter().map(|r| f(&r.stats)).collect();
        median(&values)
    };

    let median_burst: Vec<f64> = runs.iter().map(|r| r.burst_retention_percent).collect();
    let median_jitter: Vec<f64> = runs.iter().map(|r| r.stats.jitter_p99_ms).collect();
    let purity: Vec<f64> = runs.iter().filter_map(|r| r.background_purity).collect();
    let median_purity = if purity.is_empty() {
        None
    } else {
        Some(median(&purity))
    };

    Ok(AggregateResult {
        runs: k,
        mean_average_throughput: mean,
        sample_std,
        t_value,
        margin,
        ci_95: [ci_low, ci_high],
        run_variation_percent,
        cv_warning: run_variation_percent > CV_WARNING_PERCENT,
        median_throughput: med(|s| s.median_throughput),
        median_p1_throughput: med(|s| s.p1_throughput),
        median_p01_throughput: med(|s| s.p01_throughput),
        median_p95_execution_time_ms: med(|s| s.p95_execution_time_ms),
        median_p99_execution_time_ms: med(|s| s.p99_execution_time_ms),
        median_consistency_percent: med(|s| s.consistency_percent),
        median_burst_retention_percent: median(&median_burst),
        median_jitter_p99_ms: median(&median_jitter),
        median_worst_window_throughput: med(|s| s.worst_second_throughput),
        median_background_purity: median_purity,
        run_duration_ms: runs.iter().map(|r| r.duration_ms).sum(),
        started_at_min_ns: runs.iter().map(|r| r.started_at_ns).min().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::run_stats;

    fn stats_from(times: &[f64]) -> crate::run::RunStats {
        run_stats(times).expect("валидные сэмплы")
    }

    fn sig(version: &str, hash: &str) -> CompatibilitySignature {
        CompatibilitySignature {
            workload_version: version.to_string(),
            config_hash: hash.to_string(),
            seed: 0xC52A202600000001,
            worker_count: 4,
            logical_cpus: 6,
            timer_hz: 10_000_000,
            cpu_identifier: "test-cpu".to_string(),
            diagnostics_version: "0.1.0".to_string(),
        }
    }

    fn summary(
        avg_throughput: f64,
        duration_ms: u64,
        started_ns: u64,
        sig: CompatibilitySignature,
        dets_sum: u64,
    ) -> RunSummary {
        // Время тика, дающее нужный AverageThroughput при 4 сэмплах:
        // throughput сэмпла = 1000/ms = avg при ms = 1000/avg.
        let active = 1000.0 / avg_throughput;
        let mut times = [active; 4];
        times[3] = active; // все сэмплы одинаковые — стабильность 100.
        RunSummary {
            signature: sig,
            determinism: DeterminismSignature::new(vec![dets_sum, dets_sum + 1]),
            stats: stats_from(&times),
            burst_retention_percent: 100.0,
            background_purity: None,
            started_at_ns: started_ns,
            duration_ms,
        }
    }

    #[test]
    fn empty_runs_is_an_error() {
        let err = aggregate_runs(&[]).unwrap_err();
        assert!(matches!(err, AggregateError::EmptyRuns));
    }

    /// Руками посчитанный набор: три прогона, средние 800/900/1000.
    #[test]
    fn single_run_has_no_t_multiplier() {
        let s = sig("GamingCpuV1", "AAA");
        let runs = [summary(500.0, 1000, 1, s.clone(), 7)];
        let a = aggregate_runs(&runs).unwrap();
        assert_eq!(a.runs, 1);
        assert_eq!(a.margin, 0.0, "при одном прогоне интервала не бывает");
        // t95(0) = 12.0 не имеет смысла при нуле степеней свободы.
        assert_eq!(a.t_value, 0.0, "у одного прогона нет t-множителя");
        assert_eq!(a.sample_std, 0.0);
        assert_eq!(
            a.run_variation_percent, 0.0,
            "CV по одному прогону не определён"
        );
    }

    #[test]
    fn aggregate_math_with_hand_computed_values() {
        let s = sig("GamingCpuV1", "AAA");
        let runs = [
            summary(800.0, 1000, 1, s.clone(), 11),
            summary(900.0, 2000, 2, s.clone(), 11),
            summary(1000.0, 3000, 3, s.clone(), 11),
        ];
        let a = aggregate_runs(&runs).unwrap();

        assert_eq!(a.runs, 3);
        assert_eq!(a.mean_average_throughput, 900.0);
        // sample_std: (100² + 0 + 100²)/2 = 10000 → √10000 = 100.
        assert_eq!(a.sample_std, 100.0);
        // t95(2) = 4.303; margin = 4.303*100/√3 ≈ 248.434.
        assert_eq!(a.t_value, 4.303);
        let sqrt3 = 3.0f64.sqrt();
        let expected_margin = 4.303 * 100.0 / sqrt3;
        assert!((a.margin - expected_margin).abs() < 1e-9);
        assert!((a.ci_95[0] - (900.0 - expected_margin).max(0.0)).abs() < 1e-9);
        assert!((a.ci_95[1] - (900.0 + expected_margin)).abs() < 1e-9);
        // CV повторов = 100/900*100 ≈ 11.111 > 3% → предупреждение.
        assert!((a.run_variation_percent - 100.0 / 900.0 * 100.0).abs() < 1e-9);
        assert!(a.cv_warning);

        // Длительность = сумма, старт = минимум.
        assert_eq!(a.run_duration_ms, 6000);
        assert_eq!(a.started_at_min_ns, 1);
    }

    #[test]
    fn cv_warning_near_threshold() {
        let s = sig("GamingCpuV1", "BBB");
        // 990/1000/1010: mean = 1000, std = √((100+0+100)/2) = 10 → CV 1%.
        let runs = [
            summary(990.0, 100, 1, s.clone(), 7),
            summary(1000.0, 100, 1, s.clone(), 7),
            summary(1010.0, 100, 1, s.clone(), 7),
        ];
        let a = aggregate_runs(&runs).unwrap();
        assert!((a.run_variation_percent - 1.0).abs() < 1e-9);
        assert!(!a.cv_warning);
    }

    #[test]
    fn single_run_has_zero_std_and_margin() {
        let s = sig("GamingCpuV1", "CCC");
        let runs = [summary(1234.0, 500, 9, s, 3)];
        let a = aggregate_runs(&runs).unwrap();
        assert_eq!(a.runs, 1);
        assert_eq!(a.sample_std, 0.0);
        assert_eq!(a.margin, 0.0);
        assert_eq!(a.ci_95, [1234.0, 1234.0]);
        assert_eq!(a.run_variation_percent, 0.0);
        assert!(!a.cv_warning);
    }

    #[test]
    fn median_aggregation_over_run_stats() {
        let s = sig("GamingCpuV1", "DDD");
        // Три прогона с разными статистиками — берётся медиана по прогонам.
        let mk =
            |med: f64, p1: f64, p95: f64, cons: f64, burst: f64, jit: f64, det: u64| RunSummary {
                signature: s.clone(),
                determinism: DeterminismSignature::new(vec![det]),
                stats: crate::run::RunStats {
                    samples: 4,
                    work_units: 4,
                    active_time_ms_total: 0.0,
                    average_throughput: 0.0,
                    average_execution_time_ms: 0.0,
                    median_throughput: med,
                    p1_throughput: p1,
                    p01_throughput: p1 * 0.9,
                    p95_execution_time_ms: p95,
                    p99_execution_time_ms: p95 * 1.01,
                    consistency_percent: cons,
                    jitter_p99_ms: jit,
                    worst_second_throughput: cons * 0.8,
                },
                burst_retention_percent: burst,
                background_purity: None,
                started_at_ns: 1,
                duration_ms: 100,
            };
        let runs = [
            mk(500.0, 10.0, 1.0, 50.0, 100.0, 0.1, 1),
            mk(1000.0, 20.0, 2.0, 70.0, 120.0, 0.2, 1),
            mk(1500.0, 30.0, 3.0, 90.0, 140.0, 0.3, 1),
        ];
        let a = aggregate_runs(&runs).unwrap();
        assert_eq!(a.median_throughput, 1000.0);
        assert_eq!(a.median_p1_throughput, 20.0);
        assert!((a.median_p01_throughput - 18.0).abs() < 1e-9);
        assert_eq!(a.median_p95_execution_time_ms, 2.0);
        assert!((a.median_p99_execution_time_ms - 2.02).abs() < 1e-9);
        assert_eq!(a.median_consistency_percent, 70.0);
        assert_eq!(a.median_burst_retention_percent, 120.0);
        assert_eq!(a.median_jitter_p99_ms, 0.2);
        assert!((a.median_worst_window_throughput - 56.0).abs() < 1e-9);
    }

    #[test]
    fn refuses_incompatible_signatures() {
        let s1 = sig("GamingCpuV1", "AAA");
        let mut s2 = s1.clone();
        s2.config_hash = "BBB".to_string();
        let runs = [
            summary(100.0, 1, 1, s1.clone(), 5),
            summary(100.0, 1, 1, s2, 5),
        ];
        assert!(matches!(
            aggregate_runs(&runs),
            Err(AggregateError::SignatureMismatch)
        ));

        // Число воркеров — тоже часть сигнатуры.
        let mut s3 = s1.clone();
        s3.worker_count = 8;
        let runs = [
            summary(100.0, 1, 1, s1.clone(), 5),
            summary(100.0, 1, 1, s3, 5),
        ];
        assert!(matches!(
            aggregate_runs(&runs),
            Err(AggregateError::SignatureMismatch)
        ));
    }

    #[test]
    fn refuses_divergent_determinism_checksums() {
        let s = sig("GamingCpuV1", "AAA");
        let runs = [
            summary(100.0, 1, 1, s.clone(), 5),
            summary(100.0, 1, 1, s.clone(), 6),
        ];
        let err = aggregate_runs(&runs).unwrap_err();
        assert_eq!(err, AggregateError::ChecksumMismatch);
        assert_eq!(
            err.to_string(),
            "контрольная сумма различается между повторами"
        );
    }
}
