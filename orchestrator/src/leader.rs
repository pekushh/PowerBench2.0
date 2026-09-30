//! Робастный лидер сессии: выбор по медиане с оценкой доверия.
//!
//! Проблема, которую решает модуль: `recommend/` сортирует кандидатов по
//! среднему (`mean`) в первую очередь. Среднее чувствительно к выбросам:
//! один провальный отрезок внутри прогона (фоновый процесс, рамп частоты)
//! сдвигает mean, а медиана остаётся на месте. В отчётах это выглядело как
//! «перевес 771%» при медианном разрыве ×3.7.
//!
//! Модуль НЕ меняет `recommend/` (контракт, тесты, голдены): он даёт второе,
//! устойчивое мнение для отчёта и UI — «кто лидер по медиане и можно ли
//! этому верить».

use crate::result::SchemeJson;

/// Доверие к лидерству.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderConfidence {
    /// Данных мало (менее 2 прогонов у лидера) — вердикт только предварительный.
    Insufficient,
    /// Лидер есть, но есть тревожные флаги (см. `flags`).
    Low,
    /// Лидер по медиане, флагов нет.
    Normal,
}

/// Итог выбора лидера.
#[derive(Debug, Clone)]
pub struct RobustLeader {
    pub scheme_id: String,
    pub name: Option<String>,
    pub median: f64,
    pub mean: f64,
    pub runs: usize,
    pub confidence: LeaderConfidence,
    /// Человекочитаемые предупреждения (пусто ⇔ `Normal`, кроме Insufficient).
    pub flags: Vec<String>,
}

/// Порог расхождения среднего и медианы (%), выше — выбросы в прогонах.
pub const DIVERGENCE_LIMIT_PERCENT: f64 = 10.0;
/// Разрыв с 2 местом ниже — в пределах шума, лидерство неустойчиво.
pub const NOISE_GAP_LIMIT_PERCENT: f64 = 1.0;
/// CV прогонов выше — лидерство шаткое.
pub const LEADER_CV_LIMIT_PERCENT: f64 = 5.0;

/// Выбрать лидера среди допущенных (незабракованных) схем по медиане.
/// Забракованные игнорируются; при пустом входе — `None`.
pub fn robust_leader(schemes: &[SchemeJson]) -> Option<RobustLeader> {
    let mut admitted: Vec<&SchemeJson> = schemes
        .iter()
        .filter(|s| !s.rejected && s.median_throughput.is_finite() && s.median_throughput > 0.0)
        .collect();
    if admitted.is_empty() {
        return None;
    }
    admitted.sort_by(|a, b| {
        b.median_throughput
            .total_cmp(&a.median_throughput)
            .then_with(|| {
                b.mean_average_throughput
                    .total_cmp(&a.mean_average_throughput)
            })
    });
    let win = admitted[0];
    let mut flags = Vec::new();

    if win.runs < 2 {
        flags.push("у лидера менее 2 прогонов — перевес недостоверен, нужны повторы".to_string());
    }
    if win.mean_average_throughput.is_finite() && win.mean_average_throughput > 0.0 {
        let div = (win.mean_average_throughput - win.median_throughput).abs()
            / win.median_throughput
            * 100.0;
        if div > DIVERGENCE_LIMIT_PERCENT {
            flags.push(format!(
                "среднее и медиана лидера расходятся на {div:.0}% — в прогонах выбросы"
            ));
        }
    }
    if admitted.len() > 1 {
        let runner = admitted[1];
        if runner.median_throughput > 0.0 {
            let gap = (win.median_throughput - runner.median_throughput) / runner.median_throughput
                * 100.0;
            if gap < NOISE_GAP_LIMIT_PERCENT {
                flags.push(
                    "разрыв со 2 местом менее 1% — в пределах шума, лидерство неустойчиво"
                        .to_string(),
                );
            }
        }
    }
    if win.run_variation_percent.is_finite()
        && win.run_variation_percent > LEADER_CV_LIMIT_PERCENT
        && win.runs >= 2
    {
        flags.push(format!(
            "разброс прогонов лидера CV={:.1}% — повторите сессию",
            win.run_variation_percent
        ));
    }

    let confidence = if win.runs < 2 {
        LeaderConfidence::Insufficient
    } else if flags.is_empty() {
        LeaderConfidence::Normal
    } else {
        LeaderConfidence::Low
    };
    Some(RobustLeader {
        scheme_id: win.scheme_id.clone(),
        name: win.name.clone(),
        median: win.median_throughput,
        mean: win.mean_average_throughput,
        runs: win.runs,
        confidence,
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::SchemeJson;
    use powerbench_metrics::AggregateResult;

    fn aggregate(median: f64, mean: f64) -> AggregateResult {
        AggregateResult {
            runs: 1,
            mean_average_throughput: mean,
            sample_std: 0.0,
            t_value: 0.0,
            margin: 0.0,
            ci_95: [0.0, 0.0],
            run_variation_percent: 1.0,
            cv_warning: false,
            median_throughput: median,
            median_p1_throughput: 0.0,
            median_p01_throughput: 0.0,
            median_p95_execution_time_ms: 0.0,
            median_p99_execution_time_ms: 0.0,
            median_consistency_percent: 96.0,
            median_burst_retention_percent: 0.0,
            median_jitter_p99_ms: 0.0,
            median_worst_window_throughput: 0.0,
            median_background_purity: None,
            run_duration_ms: 0,
            started_at_min_ns: 0,
        }
    }

    fn scheme(id: &str, median: f64, mean: f64, runs: usize, cv: f64) -> SchemeJson {
        let mut agg = aggregate(median, mean);
        agg.runs = runs;
        agg.run_variation_percent = cv;
        let mut s = SchemeJson::from_aggregate(id.to_string(), false, None, &agg, Vec::new());
        s.runs = runs;
        s
    }

    #[test]
    fn picks_median_not_mean() {
        // У «шумной» среднее выше за счёт выброса, но медиана ниже.
        let noisy = scheme("noisy", 400.0, 900.0, 3, 1.0);
        let solid = scheme("solid", 500.0, 505.0, 3, 1.0);
        let leader = robust_leader(&[noisy, solid]).unwrap();
        assert_eq!(leader.scheme_id, "solid");
    }

    #[test]
    fn single_run_is_insufficient() {
        let s = scheme("a", 500.0, 500.0, 1, 0.0);
        let leader = robust_leader(&[s]).unwrap();
        assert_eq!(leader.confidence, LeaderConfidence::Insufficient);
        assert!(!leader.flags.is_empty());
    }

    #[test]
    fn divergence_flag_on_outliers() {
        // Среднее ушло от медианы на 20% — флаг выбросов.
        let s = scheme("a", 500.0, 600.0, 3, 1.0);
        let leader = robust_leader(&[s]).unwrap();
        assert_eq!(leader.confidence, LeaderConfidence::Low);
        assert!(leader.flags.iter().any(|f| f.contains("выбросы")));
    }

    #[test]
    fn ignores_rejected() {
        let mut bad = scheme("bad", 9000.0, 9000.0, 3, 1.0);
        bad.rejected = true;
        let good = scheme("good", 500.0, 500.0, 3, 1.0);
        let leader = robust_leader(&[bad, good]).unwrap();
        assert_eq!(leader.scheme_id, "good");
    }

    #[test]
    fn empty_or_all_rejected_is_none() {
        assert!(robust_leader(&[]).is_none());
        let mut bad = scheme("bad", 9000.0, 9000.0, 3, 1.0);
        bad.rejected = true;
        assert!(robust_leader(&[bad]).is_none());
    }
}
