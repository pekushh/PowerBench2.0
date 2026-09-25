//! Bootstrap-оценка уверенности (8192 итераций, фиксированный seed).
//!
//! Поверх средних throughput компактных прогонов. Если у всех кандидатов
//! одинаковое число валидных прогонов (≥ 2) и оно совпадает с ожидаемым —
//! режим PairedByRun: один пересэмплированный индекс раунда применяется ко
//! всем кандидатам сразу (сохраняет общий дрейф раунда). Иначе — явный режим
//! Independent. Точные равенства делят «кредит лучшего» поровну. Вход должен
//! быть отсортирован до сэмплирования (см. `recommend`), чтобы порядок
//! коллекции не влиял на результат. Сохраняются ровно три вероятности.

use crate::SchemeAggregate;

/// Число итераций bootstrap (фиксировано спецификацией).
pub const BOOTSTRAP_ITERATIONS: usize = 8192;
/// Фиксированный seed: вход перед сэмплированием сортируется, поэтому
/// порядок коллекции не влияет на результат.
const BOOTSTRAP_SEED: u64 = 0x9E3779B97F4A7C15;
/// Порог «перевес > 1%» (относительно).
const MARGIN_1PCT: f64 = 0.01;

/// Режим bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    /// Общий пересэмплированный индекс раунда для всех кандидатов.
    PairedByRun,
    /// Каждый кандидат пересэмплируется независимо.
    Independent,
}

/// Три сохраняемые вероятности.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BootstrapProbabilities {
    /// P(кандидат — лучший).
    pub p_best: f64,
    /// P(перевес > 0).
    pub p_margin_gt_0: f64,
    /// P(перевес > 1%).
    pub p_margin_gt_1pct: f64,
}

/// Детерминированный SplitMix64 (bootstrap не требует криптостойкости).
#[derive(Debug, Clone, Copy)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    /// Равномерное в [0, 1).
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Равномерное в [0, n).
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        (self.next_u64() % n as u64) as usize
    }
}

/// Режим bootstrap по числу прогонов кандидатов.
pub fn bootstrap_mode(candidates: &[&SchemeAggregate], expected_runs: usize) -> BootMode {
    let counts: Vec<usize> = candidates.iter().map(|c| c.runs.len()).collect();
    let paired = !counts.is_empty()
        && counts.iter().min() == counts.iter().max()
        && counts[0] >= 2
        && counts[0] == expected_runs;
    if paired {
        BootMode::PairedByRun
    } else {
        BootMode::Independent
    }
}

/// Bootstrap трёх вероятностей. `winner`/`runner` — индексы в `candidates`
/// (должны быть отсортированы перед вызовом).
pub fn bootstrap_probabilities(
    candidates: &[&SchemeAggregate],
    mode: BootMode,
    winner: usize,
    runner: Option<usize>,
) -> BootstrapProbabilities {
    let k = candidates.len();
    let mut rng = SplitMix64::new(BOOTSTRAP_SEED);

    let mut means = vec![0.0; k];
    let mut idx: Vec<usize> = Vec::with_capacity(64);

    let mut best_n = 0.0f64; // «кредит лучшего» для победителя
    let mut margin_gt_0 = 0.0f64;
    let mut margin_gt_1pct = 0.0f64;

    let n_for = |c: &SchemeAggregate| c.runs.len().max(1);

    for _ in 0..BOOTSTRAP_ITERATIONS {
        idx.clear();
        for (c, mean) in means.iter_mut().enumerate() {
            match mode {
                BootMode::PairedByRun => {
                    let n = n_for(candidates[0]);
                    if idx.is_empty() {
                        for _ in 0..n {
                            idx.push(rng.below(n));
                        }
                    }
                    let runs = &candidates[c].runs;
                    let s: f64 = idx.iter().map(|&j| runs[j].average_throughput).sum();
                    *mean = s / n as f64;
                }
                BootMode::Independent => {
                    let n = n_for(candidates[c]);
                    let runs = &candidates[c].runs;
                    let mut s = 0.0f64;
                    for _ in 0..n {
                        s += runs[rng.below(runs.len().max(1))].average_throughput;
                    }
                    *mean = s / n as f64;
                }
            }
        }

        // Точные равенства делят «кредит лучшего» поровну.
        let best = means.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let tie_count = means.iter().filter(|&&m| m == best).count().max(1) as f64;
        if means[winner] == best {
            best_n += 1.0 / tie_count;
        }

        if let Some(r) = runner {
            let dw = means[winner] - means[r];
            if dw > 0.0 {
                margin_gt_0 += 1.0;
            }
            let rel = if means[r] > 0.0 { dw / means[r] } else { 0.0 };
            if rel > MARGIN_1PCT {
                margin_gt_1pct += 1.0;
            }
        }
    }

    let n = BOOTSTRAP_ITERATIONS as f64;
    BootstrapProbabilities {
        p_best: best_n / n,
        p_margin_gt_0: margin_gt_0 / n,
        p_margin_gt_1pct: margin_gt_1pct / n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunCompact;
    use powerbench_metrics::{AggregateResult, CompatibilitySignature, DeterminismSignature};

    fn sig() -> CompatibilitySignature {
        CompatibilitySignature {
            workload_version: "GamingCpuV1".to_string(),
            config_hash: "AAAA".to_string(),
            seed: 0xC52A202600000001,
            worker_count: 4,
            logical_cpus: 6,
            timer_hz: 10_000_000,
            cpu_identifier: "test-cpu".to_string(),
            diagnostics_version: "0.1.0".to_string(),
        }
    }

    fn dummy_aggregate() -> AggregateResult {
        AggregateResult {
            runs: 0,
            mean_average_throughput: 0.0,
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

    fn dummy_run(avg: f64) -> RunCompact {
        RunCompact {
            average_throughput: avg,
            median_throughput: avg,
            p1_throughput: avg * 0.99,
            p01_throughput: avg * 0.97,
            consistency_percent: 95.0,
            samples: 20_000,
        }
    }

    fn mk(id: &str, run_avgs: &[f64]) -> SchemeAggregate {
        SchemeAggregate {
            scheme_id: id.to_string(),
            rejected: false,
            signature: sig(),
            determinism: DeterminismSignature::new(vec![1]),
            aggregate: dummy_aggregate(),
            runs: run_avgs.iter().map(|&a| dummy_run(a)).collect(),
            is_original: false,
            is_active: false,
        }
    }

    #[test]
    fn splitmix_is_deterministic_and_uniformish() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut mean = 0.0f64;
        let mut rng = SplitMix64::new(1);
        for _ in 0..100_000 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v));
            mean += v;
        }
        let avg = mean / 100_000.0;
        assert!((avg - 0.5).abs() < 0.01, "неравномерный rng: {avg}");
    }

    #[test]
    fn mode_detection() {
        let two = [mk("A", &[1.0, 1.0, 1.0]), mk("B", &[1.0, 1.0, 1.0])];
        // Равное число (3) и совпадает с ожидаемым (3) → PairedByRun.
        assert_eq!(
            bootstrap_mode(&[&two[0], &two[1]], 3),
            BootMode::PairedByRun
        );
        // Не совпадает с ожидаемым → Independent.
        assert_eq!(
            bootstrap_mode(&[&two[0], &two[1]], 5),
            BootMode::Independent
        );
        // Разное число прогонов → Independent.
        let different = [mk("A", &[1.0, 1.0, 1.0]), mk("B", &[1.0, 1.0, 1.0, 1.0])];
        assert_eq!(
            bootstrap_mode(&[&different[0], &different[1]], 3),
            BootMode::Independent
        );
    }

    #[test]
    fn equal_schemes_split_best_credit() {
        let a = mk("A", &[1.0, 1.0, 1.0]);
        let b = mk("B", &[1.0, 1.0, 1.0]);
        let p = bootstrap_probabilities(&[&a, &b], BootMode::PairedByRun, 0, Some(1));
        // Все средние равны → кредит лучшего делится поровну.
        assert!((p.p_best - 0.5).abs() < 1e-9);
        assert_eq!(p.p_margin_gt_0, 0.0);
        assert_eq!(p.p_margin_gt_1pct, 0.0);
    }

    #[test]
    fn refuses_order_dependence_via_sorted_input() {
        // Распределение устойчиво к порядку кандидатов при отсортированном входе:
        // вероятности зависят только от данных и фиксированного seed.
        let a = mk("A", &[990.0, 1000.0, 1010.0]);
        let b = mk("B", &[950.0, 960.0, 970.0]);
        let p1 = bootstrap_probabilities(&[&a, &b], BootMode::PairedByRun, 0, Some(1));
        let p2 = bootstrap_probabilities(&[&b, &a], BootMode::PairedByRun, 1, Some(0));
        assert_eq!(p1.p_best, p2.p_best);
        assert_eq!(p1.p_margin_gt_0, p2.p_margin_gt_0);
        assert_eq!(p1.p_margin_gt_1pct, p2.p_margin_gt_1pct);
    }
}
