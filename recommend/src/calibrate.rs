//! Калибровочный валидатор качества рекомендаций.
//!
//! Детерминированная матрица: истинные перевесы 0 / 0.5 / 1 / 2%,
//! шум ±1 / ±3 / ±5%, 5 парных прогонов, 100–200 серий на ячейку.
//! Гейты: для равных схем доля `Confirmed` ≤ 5% серий; точность выбора
//! ≥ 60% (0.5%), ≥ 70% (1%), ≥ 80% (2%) на каждом шуме; чистый перевес 2%
//! при шуме ±1% даёт `Confirmed` ≥ 85% серий; CV > 2% никогда не Confirmed.

use powerbench_metrics::{AggregateResult, CompatibilitySignature, DeterminismSignature};

use crate::bootstrap::SplitMix64;
use crate::{EvidenceLevel, Recommendation, RunCompact, SchemeAggregate, recommend};

/// Истинные перевесы калибровочной матрицы (%). 0 / 0.5 / 1 / 2.
pub const CALIBRATION_MARGINS_PERCENT: [f64; 4] = [0.0, 0.5, 1.0, 2.0];
/// Уровни шума калибровочной матрицы (%). ±1 / ±3 / ±5.
pub const CALIBRATION_NOISES_PERCENT: [f64; 3] = [1.0, 3.0, 5.0];
/// Число парных прогонов в серии.
pub const CALIBRATION_PAIRED_RUNS: usize = 5;
/// Измерений в каждом компактном прогоне (гейты ничьи проходят).
const CALIBRATION_SAMPLES: usize = 20_000;
/// Доля шума, приходящаяся на независимый остаток прогона: основная часть —
/// общий дрейф раунда, общий для пары (его и сохраняет PairedByRun).
const INDEPENDENT_NOISE_FRACTION: f64 = 0.3;

/// Отчёт по одной ячейке матрицы.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellReport {
    pub margin_pct: f64,
    pub noise_pct: f64,
    pub series: usize,
    /// Серий, где рекомендован истинно лучший кандидат B.
    pub correct_picks: usize,
    /// Серий с уровнем Confirmed.
    pub confirmed: usize,
    /// Серий, где при CV победителя > 2% уровень всё равно Confirmed.
    pub high_cv_confirmed: usize,
}

/// Итоговый отчёт валидатора.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationReport {
    pub cells: Vec<CellReport>,
    /// Доля Confirmed для равных схем (перевес 0) ≤ 5% на каждой ячейке.
    pub equal_confirmed_ok: bool,
}

/// Кандидат из средних парных прогонов (компактные сводки — вход движка).
fn scheme_from_runs(id: &str, avgs: &[f64]) -> SchemeAggregate {
    let runs: Vec<RunCompact> = avgs
        .iter()
        .map(|&m| RunCompact {
            average_throughput: m,
            median_throughput: m,
            p1_throughput: m * 0.99,
            p01_throughput: m * 0.97,
            consistency_percent: 95.0,
            samples: CALIBRATION_SAMPLES,
        })
        .collect();

    let mean_avg = runs.iter().map(|r| r.average_throughput).sum::<f64>() / runs.len() as f64;
    let std = if runs.len() > 1 {
        let m = mean_avg;
        (runs
            .iter()
            .map(|r| (r.average_throughput - m).powi(2))
            .sum::<f64>()
            / (runs.len() - 1) as f64)
            .sqrt()
    } else {
        0.0
    };
    let cv = if mean_avg > 0.0 {
        std / mean_avg * 100.0
    } else {
        0.0
    };

    let mut avgs_sorted: Vec<f64> = avgs.to_vec();
    avgs_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_avg = powerbench_metrics::percentile(&avgs_sorted, 0.5);
    let aggregate = AggregateResult {
        runs: runs.len(),
        mean_average_throughput: mean_avg,
        sample_std: std,
        t_value: 0.0,
        margin: 0.0,
        ci_95: [0.0, 0.0],
        run_variation_percent: cv,
        cv_warning: cv > 3.0,
        median_throughput: median_avg,
        median_p1_throughput: median_avg * 0.99,
        median_p01_throughput: median_avg * 0.97,
        median_p95_execution_time_ms: 0.0,
        median_p99_execution_time_ms: 0.0,
        median_consistency_percent: 95.0,
        median_burst_retention_percent: 0.0,
        median_jitter_p99_ms: 0.0,
        median_worst_window_throughput: median_avg * 0.9,
        median_background_purity: None,
        run_duration_ms: 0,
        started_at_min_ns: 0,
    };

    SchemeAggregate {
        scheme_id: id.to_string(),
        rejected: false,
        signature: CompatibilitySignature {
            workload_version: "GamingCpuV1".to_string(),
            config_hash: "C-A-L".to_string(),
            seed: 0xC52A202600000001,
            worker_count: 4,
            logical_cpus: 6,
            timer_hz: 10_000_000,
            cpu_identifier: "calibrator-cpu".to_string(),
            diagnostics_version: "0.1.0".to_string(),
        },
        determinism: DeterminismSignature::new(vec![42]),
        aggregate,
        runs,
        is_original: false,
        is_active: false,
    }
}

/// Генерация серии: пара схем A (базовая) и B (A × (1 + margin) с независимым
/// шумом), по CALIBRATION_PAIRED_RUNS парных прогонов. Общий дрейф раунда
/// сохраняется (режим PairedByRun). Детерминирована по seed.
pub fn calibrate_series(
    margin_pct: f64,
    noise_pct: f64,
    series_index: u64,
) -> (SchemeAggregate, SchemeAggregate) {
    let mut rng = SplitMix64::new(series_index.wrapping_mul(1_000_003) ^ 0xA5A5);
    let margin = margin_pct / 100.0;
    let noise = noise_pct / 100.0;
    let base = 1000.0;

    let mut a_avgs = Vec::with_capacity(CALIBRATION_PAIRED_RUNS);
    let mut b_avgs = Vec::with_capacity(CALIBRATION_PAIRED_RUNS);

    for _ in 0..CALIBRATION_PAIRED_RUNS {
        // Общий дрейф раунда — одинаков для пары A/B в одном раунде.
        let drift = (rng.next_f64() * 2.0 - 1.0) * noise;
        // Независимый остаток «помехи измерения» — малая доля шумового бюджета.
        let a_ind = (rng.next_f64() * 2.0 - 1.0) * noise * INDEPENDENT_NOISE_FRACTION;
        let b_ind = (rng.next_f64() * 2.0 - 1.0) * noise * INDEPENDENT_NOISE_FRACTION;
        let a = base * (1.0 + drift) * (1.0 + a_ind);
        let b = base * (1.0 + drift) * (1.0 + margin) * (1.0 + b_ind);
        a_avgs.push(a);
        b_avgs.push(b);
    }

    (
        scheme_from_runs("A", &a_avgs),
        scheme_from_runs("B", &b_avgs),
    )
}

/// Прогнать матрицу калибровки: `series_per_cell` серий на ячейку.
pub fn run_calibration(series_per_cell: usize) -> CalibrationReport {
    let mut cells =
        Vec::with_capacity(CALIBRATION_MARGINS_PERCENT.len() * CALIBRATION_NOISES_PERCENT.len());

    for &margin in &CALIBRATION_MARGINS_PERCENT {
        for &noise in &CALIBRATION_NOISES_PERCENT {
            let mut correct = 0usize;
            let mut confirmed = 0usize;
            let mut high_cv_confirmed = 0usize;
            for s in 0..series_per_cell as u64 {
                let (a, b) = calibrate_series(margin, noise, s);
                let rec = recommend(&[a.clone(), b.clone()], CALIBRATION_PAIRED_RUNS);
                process_series(
                    &a,
                    &b,
                    &rec,
                    &mut correct,
                    &mut confirmed,
                    &mut high_cv_confirmed,
                );
            }
            cells.push(CellReport {
                margin_pct: margin,
                noise_pct: noise,
                series: series_per_cell,
                correct_picks: correct,
                confirmed,
                high_cv_confirmed,
            });
        }
    }

    let equal_confirmed_ok = cells
        .iter()
        .filter(|c| c.margin_pct == 0.0)
        .all(|c| c.confirmed as f64 / c.series as f64 <= 0.05);

    CalibrationReport {
        cells,
        equal_confirmed_ok,
    }
}

fn process_series(
    a: &SchemeAggregate,
    b: &SchemeAggregate,
    rec: &Recommendation,
    correct: &mut usize,
    confirmed: &mut usize,
    high_cv_confirmed: &mut usize,
) {
    if rec.recommended_scheme.as_deref() == Some("B") {
        *correct += 1;
    }
    if rec.level == EvidenceLevel::Confirmed {
        *confirmed += 1;
        let winner_cv = match rec.recommended_scheme.as_deref() {
            Some("A") => a.aggregate.run_variation_percent,
            Some("B") => b.aggregate.run_variation_percent,
            _ => 0.0,
        };
        if winner_cv > 2.0 {
            *high_cv_confirmed += 1;
        }
    }
}

/// Гейты матрицы (критерии приёмки Этапа 4).
pub fn all_gates_pass(report: &CalibrationReport) -> bool {
    for cell in &report.cells {
        let acc = cell.correct_picks as f64 / cell.series as f64;
        let conf = cell.confirmed as f64 / cell.series as f64;
        // Точность выбора: ≥ 60% (0.5%), ≥ 70% (1%), ≥ 80% (2%) на каждом шуме.
        // Для равных схем (margin 0) точность не проверяется.
        let min_acc = if cell.margin_pct <= 0.0 {
            0.0
        } else if cell.margin_pct < 0.75 {
            0.60
        } else if cell.margin_pct < 1.5 {
            0.70
        } else {
            0.80
        };
        if acc < min_acc - 1e-9 {
            return false;
        }
        // Чистый перевес 2% при шуме ±1% → Confirmed ≥ 85% серий.
        if cell.margin_pct == 2.0 && cell.noise_pct == 1.0 && conf < 0.85 - 1e-9 {
            return false;
        }
        // CV > 2% никогда не Confirmed.
        if cell.high_cv_confirmed != 0 {
            return false;
        }
    }
    report.equal_confirmed_ok
}

/// Запуск калибровки с гейтами — точка входа для консольного валидатора.
pub fn validate(series_per_cell: usize) -> (CalibrationReport, bool) {
    let report = run_calibration(series_per_cell);
    let ok = all_gates_pass(&report);
    (report, ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_matrix_gates_hold() {
        // 100 серий на ячейку (спецификация: 100–200).
        let (report, ok) = validate(100);
        assert!(ok, "гейты матрицы не выполнены: {report:?}");
    }
}
