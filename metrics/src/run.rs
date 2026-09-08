//! Статистика одного прогона (набора сэмплов времени тика).
//!
//! Каждый завершённый тик даёт один сэмпл: `CompletedWorkUnits = 1`,
//! `ActiveTimeMs = время тика в мс`, throughput сэмпла = `1000/ActiveTimeMs`.
//! Невалидные сэмплы (неположительное или не конечное время) исключаются.

use crate::percentile::{percentile, percentile_sorted};

/// Исключить невалидные сэмплы: время должно быть положительным и конечным.
pub fn filter_valid_times(times_ms: &[f64]) -> Vec<f64> {
    times_ms
        .iter()
        .copied()
        .filter(|t| t.is_finite() && *t > 0.0)
        .collect()
}

/// Среднее арифметическое (0.0 для пустого набора).
pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// Стандартное отклонение генеральной совокупности:
/// `sqrt( Σ(x − mean)² / n )`.
pub fn population_std(values: &[f64]) -> f64 {
    let n = values.len();
    if n == 0 {
        return 0.0;
    }
    let m = mean(values);
    (values.iter().map(|x| (x - m).powi(2)).sum::<f64>() / n as f64).sqrt()
}

/// Медиана (перцентиль 0.50).
pub fn median(values: &[f64]) -> f64 {
    percentile(values, 0.50)
}

/// Score стабильности одной группы сэмплов (фразы спецификации):
/// `variation = population_std(throughput)/mean(throughput)*100`,
/// `score = clamp(100 − variation, 0, 100)`.
pub fn consistency_score(throughput: &[f64]) -> f64 {
    if throughput.is_empty() {
        return 0.0;
    }
    let m = mean(throughput);
    if m == 0.0 {
        return 0.0;
    }
    let variation = population_std(throughput) / m * 100.0;
    (100.0 - variation).clamp(0.0, 100.0)
}

/// ConsistencyPercent прогона с группами по фазам: в каждой фазе считается
/// score стабильности, итог — взвешенное среднее score по числу сэмплов фаз.
///
/// Группа: `(идентификатор фазы, времена тиков в мс)`. Сэмплы каждой группы
/// приводятся к throughput `1000/ms`.
pub fn consistency_percent(times_by_phase: &[(u32, &[f64])]) -> f64 {
    let mut weighted = 0.0;
    let mut total_weight = 0.0;
    for &(_, times) in times_by_phase {
        let throughput: Vec<f64> = times.iter().map(|ms| 1000.0 / ms).collect();
        let n = throughput.len() as f64;
        weighted += n * consistency_score(&throughput);
        total_weight += n;
    }
    if total_weight == 0.0 {
        0.0
    } else {
        weighted / total_weight
    }
}

/// TickJitterP99Ms: перцентиль 0.99 попарных разностей времён соседних тиков.
/// Если данных нет (меньше двух сэмплов) — 0 (спецификация).
pub fn jitter_p99_ms(times_ms: &[f64]) -> f64 {
    if times_ms.len() < 2 {
        return 0.0;
    }
    let diffs: Vec<f64> = times_ms
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .collect();
    percentile(&diffs, 0.99)
}

/// BurstRetentionPercent = AverageThroughput(Heavy)/AverageThroughput(Light)*100
/// (0, если AverageThroughput(Light) = 0).
pub fn burst_retention_percent(average_heavy: f64, average_light: f64) -> f64 {
    if average_light == 0.0 {
        0.0
    } else {
        average_heavy / average_light * 100.0
    }
}

/// Статистика одного прогона фазы.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunStats {
    /// Число валидных сэмплов.
    pub samples: usize,
    /// Сумма единиц работы (CompletedWorkUnits = 1 на тик).
    pub work_units: u64,
    /// Сумма активных времён тиков в мс.
    pub active_time_ms_total: f64,
    /// AverageThroughput = (Σ work) * 1000 / (Σ activeMs) — взвешенное
    /// среднее, НЕ среднее по-сэмпловых throughput.
    pub average_throughput: f64,
    /// AverageExecutionTimeMs = (Σ activeMs) / (Σ work).
    pub average_execution_time_ms: f64,
    /// MedianThroughput = перцентиль 0.50 от throughput сэмплов.
    pub median_throughput: f64,
    /// P1Throughput = перцентиль 0.01.
    pub p1_throughput: f64,
    /// P01Throughput = перцентиль 0.001.
    pub p01_throughput: f64,
    /// P95ExecutionTimeMs = перцентиль 0.95 от времён тиков.
    pub p95_execution_time_ms: f64,
    /// P99ExecutionTimeMs = перцентиль 0.99 от времён тиков.
    pub p99_execution_time_ms: f64,
    /// ConsistencyPercent прогона (сэмплы одной фазы = одна группа).
    pub consistency_percent: f64,
    /// TickJitterP99Ms.
    pub jitter_p99_ms: f64,
}

/// Статистика прогона по временам тиков. `None`, если валидных сэмплов нет.
pub fn run_stats(times_ms: &[f64]) -> Option<RunStats> {
    let times = filter_valid_times(times_ms);
    if times.is_empty() {
        return None;
    }
    let samples = times.len();
    let work_units = samples as u64;
    let active_time_ms_total = times.iter().sum::<f64>();
    let average_throughput = work_units as f64 * 1000.0 / active_time_ms_total;
    let average_execution_time_ms = active_time_ms_total / work_units as f64;

    let mut throughput: Vec<f64> = times.iter().map(|ms| 1000.0 / ms).collect();
    throughput.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let mut times_sorted = times.clone();
    times_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    Some(RunStats {
        samples,
        work_units,
        active_time_ms_total,
        average_throughput,
        average_execution_time_ms,
        median_throughput: percentile_sorted(&throughput, 0.50),
        p1_throughput: percentile_sorted(&throughput, 0.01),
        p01_throughput: percentile_sorted(&throughput, 0.001),
        p95_execution_time_ms: percentile_sorted(&times_sorted, 0.95),
        p99_execution_time_ms: percentile_sorted(&times_sorted, 0.99),
        consistency_percent: consistency_percent(&[(0, &times)]),
        jitter_p99_ms: jitter_p99_ms(&times),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_samples_are_excluded() {
        let samples = [0.0, -5.0, f64::NAN, f64::INFINITY, 5.0];
        let valid = filter_valid_times(&samples);
        assert_eq!(valid, vec![5.0]);
    }

    #[test]
    fn average_throughput_is_weighted_not_sample_mean() {
        // 2, 2, 4 мс: work = 3, ΣactiveMs = 8 → 3*1000/8 = 375.
        let r = run_stats(&[2.0, 2.0, 4.0]).unwrap();
        assert_eq!(r.average_throughput, 375.0);
        assert!((r.average_execution_time_ms - 8.0 / 3.0).abs() < 1e-12);
        assert_eq!(r.samples, 3);
        assert_eq!(r.work_units, 3);
        // Среднее по-сэмпловых throughput = (500+500+250)/3 ≈ 416.7 ≠ 375.
    }

    #[test]
    fn empty_and_invalid_inputs_produce_none() {
        assert!(run_stats(&[]).is_none());
        assert!(run_stats(&[0.0, f64::NAN, f64::NEG_INFINITY]).is_none());
    }

    /// Полный набор руками посчитанных формул на временах [1, 1, 1, 2] мс.
    #[test]
    fn run_stats_honours_all_formulas() {
        let r = run_stats(&[1.0, 1.0, 1.0, 2.0]).unwrap();
        assert_eq!(r.samples, 4);
        assert_eq!(r.work_units, 4);
        assert_eq!(r.active_time_ms_total, 5.0);
        // AverageThroughput = 4*1000/5 = 800.
        assert_eq!(r.average_throughput, 800.0);
        assert_eq!(r.average_execution_time_ms, 1.25);

        // throughput сэмплов: [500, 1000, 1000, 1000].
        // median (p0.5): pos = 1.5 → 1000.
        assert_eq!(r.median_throughput, 1000.0);
        // p1: pos = 0.01*3 = 0.03 → 500 + 500*0.03 = 515.
        let p1 = 500.0 + 500.0 * 0.03;
        assert!((r.p1_throughput - p1).abs() < 1e-9);
        // p01: pos = 0.001*3 = 0.003 → 500 + 500*0.003 = 501.5.
        let p01 = 500.0 + 500.0 * 0.003;
        assert!((r.p01_throughput - p01).abs() < 1e-9);

        // времена [1,1,1,2]: p95 pos = 0.95*3 = 2.85 → 1 + 1*0.85 = 1.85.
        assert!((r.p95_execution_time_ms - 1.85).abs() < 1e-12);
        // p99 pos = 0.99*3 = 2.97 → 1 + 1*0.97 = 1.97.
        assert!((r.p99_execution_time_ms - 1.97).abs() < 1e-12);

        // Стабильность: population_std([500,1000,1000,1000]) = 216.50635094610965,
        // mean = 875 → score = clamp(100 − 24.74358298…, 0, 100) ≈ 75.256417.
        assert!((r.consistency_percent - 75.256417).abs() < 1e-3);

        // jitter: разности [0, 0, 1]; p99 pos = 0.99*2 = 1.98 → 0 + 1*0.98.
        assert!((r.jitter_p99_ms - 0.98).abs() < 1e-12);
    }

    #[test]
    fn consistency_percent_is_weighted_across_phases() {
        // Фаза A: [1,1] → throughput [1000,1000], variation 0, score 100.
        // Фаза B: [1,3] → throughput [1000, 333.33…], variation 50, score 50.
        // Веса 2 и 2 → (2*100 + 2*50)/4 = 75.
        let a = [1.0, 1.0];
        let b = [1.0, 3.0];
        let score = consistency_percent(&[(0, &a), (1, &b)]);
        assert!((score - 75.0).abs() < 1e-9);
        // Пустые группы → 0.
        assert_eq!(consistency_percent(&[]), 0.0);
    }

    #[test]
    fn burst_retention_and_jitter_edges() {
        assert!((burst_retention_percent(900.0, 800.0) - 112.5).abs() < 1e-9);
        assert_eq!(burst_retention_percent(900.0, 0.0), 0.0);
        // Меньше двух сэмплов — jitter 0.
        assert_eq!(jitter_p99_ms(&[1.0]), 0.0);
        assert_eq!(jitter_p99_ms(&[]), 0.0);
    }
}