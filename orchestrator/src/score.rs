//! Скоринг схем по трем нормализованным метрикам с пользовательскими весами.
//!
//! Отчёт превращается из сводки цифр в рекомендацию: каждая допущенная схема
//! получает балл 0..100 по взвешенному среднему трёх нормированных метрик
//! (производительность, стабильность, худшая секунда). Нормировка — «лучшая
//! среди допущенных = 100» по каждой метрике; отвергнутые схему получают 0.

use crate::result::{SchemeJson, default_score_weights};

/// Веса скоринга (производительность / стабильность / худшая секунда, %).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreWeights {
    pub performance: f64,
    pub stability: f64,
    pub worst_second: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        let d = default_score_weights();
        Self {
            performance: d[0],
            stability: d[1],
            worst_second: d[2],
        }
    }
}

impl ScoreWeights {
    /// Вектор весов в порядке «производительность, стабильность, худшая секунда»,
    /// нормализованный к сумме 1; при недопустимой сумме — дефолтные веса.
    pub fn normalized(&self) -> [f64; 3] {
        let total = self.performance + self.stability + self.worst_second;
        if total.is_finite() && total > 0.0 {
            [
                self.performance / total,
                self.stability / total,
                self.worst_second / total,
            ]
        } else {
            let d = default_score_weights();
            let s = d[0] + d[1] + d[2];
            [d[0] / s, d[1] / s, d[2] / s]
        }
    }
}

/// Балл одной схемы.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemeScore {
    pub scheme_id: String,
    pub name: Option<String>,
    /// Итоговый балл 0..=100 (100 — лучшая среди допущенных по весам).
    pub score: f64,
    /// Нормированная производительность (0..=100).
    pub performance: f64,
    /// Нормированная стабильность (0..=100).
    pub stability: f64,
    /// Нормированная худшая секунда (0..=100).
    pub worst_second: f64,
    /// Схема отвергнута до среднего? Если да — балл 0.
    pub rejected: bool,
}

fn usable_score(value: f64) -> Option<f64> {
    if value.is_finite() && value > 0.0 {
        Some(value)
    } else {
        None
    }
}

/// Рассчитать баллы всех схем из JSON-результата по заданным весам.
/// Порядок элементов соответствует порядку `schemes` (уже отсортированному
/// рекомендацией: лидер первым).
pub fn score_schemes(schemes: &[SchemeJson], weights: &ScoreWeights) -> Vec<SchemeScore> {
    // «Лучшая среди допущенных» по каждой метрике (максимум — везде: и у
    // производительности, и у стабильности, и у худшей секунды больше=лучше).
    let mut best_perf: Option<f64> = None;
    let mut best_stab: Option<f64> = None;
    let mut best_worst: Option<f64> = None;
    for s in schemes {
        if s.rejected {
            continue;
        }
        if let Some(v) = usable_score(s.mean_average_throughput) {
            best_perf = Some(best_perf.map_or(v, |b: f64| b.max(v)));
        }
        if let Some(v) = usable_score(s.median_consistency_percent) {
            best_stab = Some(best_stab.map_or(v, |b: f64| b.max(v)));
        }
        if let Some(v) = usable_score(s.median_worst_window_throughput) {
            best_worst = Some(best_worst.map_or(v, |b: f64| b.max(v)));
        }
    }
    let norm = weights.normalized();
    let [w_p, w_s, w_w] = norm;

    schemes
        .iter()
        .map(|s| {
            if s.rejected {
                return SchemeScore {
                    scheme_id: s.scheme_id.clone(),
                    name: s.name.clone(),
                    score: 0.0,
                    performance: 0.0,
                    stability: 0.0,
                    worst_second: 0.0,
                    rejected: true,
                };
            }
            let perf = best_perf.map_or(0.0, |b| {
                usable_score(s.mean_average_throughput).map_or(0.0, |v| v / b * 100.0)
            });
            let stab = best_stab.map_or(0.0, |b| {
                usable_score(s.median_consistency_percent).map_or(0.0, |v| v / b * 100.0)
            });
            let worst = best_worst.map_or(0.0, |b| {
                usable_score(s.median_worst_window_throughput).map_or(0.0, |v| v / b * 100.0)
            });
            let score = w_p * perf + w_s * stab + w_w * worst;
            SchemeScore {
                scheme_id: s.scheme_id.clone(),
                name: s.name.clone(),
                score,
                performance: perf,
                stability: stab,
                worst_second: worst,
                rejected: false,
            }
        })
        .collect()
}

/// Балл лидера среди допущенных (None — нет ни одного допущенного).
pub fn score_leader(scores: &[SchemeScore]) -> Option<&SchemeScore> {
    scores.iter().filter(|s| !s.rejected).max_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::SchemeJson;

    fn scheme(id: &str, rejected: bool, perf: f64, stab: f64, worst: f64) -> SchemeJson {
        SchemeJson {
            scheme_id: id.to_string(),
            name: Some(id.to_string()),
            rejected,
            rejection_reason: None,
            runs: 3,
            mean_average_throughput: perf,
            sample_std: 0.0,
            t_value: 0.0,
            margin: 0.0,
            ci_95: [0.0, 0.0],
            run_variation_percent: 0.0,
            cv_warning: false,
            median_throughput: perf,
            median_p1_throughput: 0.0,
            median_p01_throughput: 0.0,
            median_p95_execution_time_ms: 0.0,
            median_p99_execution_time_ms: 0.0,
            median_consistency_percent: stab,
            median_burst_retention_percent: 0.0,
            median_jitter_p99_ms: 0.0,
            median_worst_window_throughput: worst,
            median_background_purity: None,
            run_duration_ms: 0,
            started_at_min_ns: 0,
            per_run: Vec::new(),
        }
    }

    #[test]
    fn equal_schemes_get_equal_50_scores_by_default_weights() {
        let schemes = vec![
            scheme("a", false, 100.0, 50.0, 40.0),
            scheme("b", false, 100.0, 50.0, 40.0),
        ];
        let scores = score_schemes(&schemes, &ScoreWeights::default());
        assert_eq!(scores.len(), 2);
        // Обе схемы лучшие по всем метрикам → 100, никаких перекосов в весах.
        assert!((scores[0].score - 100.0).abs() < 1e-9);
        assert!((scores[1].score - 100.0).abs() < 1e-9);
    }

    #[test]
    fn performance_edge_dominates_under_weight() {
        // a — сильнейшая по производительности, одинаковая стабильность/худшая.
        let schemes = vec![
            scheme("a", false, 200.0, 50.0, 40.0),
            scheme("b", false, 100.0, 50.0, 40.0),
        ];
        let weights = ScoreWeights {
            performance: 100.0,
            stability: 0.0,
            worst_second: 0.0,
        };
        let scores = score_schemes(&schemes, &weights);
        assert!((scores[0].score - 100.0).abs() < 1e-9);
        assert!((scores[1].score - 50.0).abs() < 1e-9);
    }

    #[test]
    fn stability_weight_flips_leader() {
        // a — выше производительность; b — сильнее по стабильности.
        let schemes = vec![
            scheme("a", false, 300.0, 40.0, 40.0),
            scheme("b", false, 150.0, 100.0, 40.0),
        ];
        let perf_only = score_schemes(
            &schemes,
            &ScoreWeights {
                performance: 100.0,
                stability: 0.0,
                worst_second: 0.0,
            },
        );
        assert!(perf_only[0].score > perf_only[1].score);
        let stab_only = score_schemes(
            &schemes,
            &ScoreWeights {
                performance: 0.0,
                stability: 100.0,
                worst_second: 0.0,
            },
        );
        assert!(stab_only[1].score > stab_only[0].score);
    }

    #[test]
    fn rejected_scheme_scores_zero_and_excluded_from_baseline() {
        // a — допущенная (плохая), b — отвергнутая (была бы базовая линия).
        let schemes = vec![
            scheme("a", false, 30.0, 60.0, 40.0),
            scheme("b", true, 300.0, 100.0, 200.0),
        ];
        let scores = score_schemes(&schemes, &ScoreWeights::default());
        assert_eq!(
            scores[0].score, 100.0,
            "базовая линия — единственная допущенная"
        );
        assert_eq!(
            scores[1].score, 0.0,
            "отвергнутая — 0, в нормировке не участвует"
        );
    }

    #[test]
    fn degenerate_weights_fall_back_to_defaults() {
        let schemes = vec![
            scheme("a", false, 100.0, 50.0, 40.0),
            scheme("b", false, 50.0, 100.0, 40.0),
        ];
        let weights = ScoreWeights {
            performance: 0.0,
            stability: 0.0,
            worst_second: 0.0,
        };
        let norms = weights.normalized();
        assert!((norms[0] - 0.5).abs() < 1e-9);
        assert!((norms[1] - 0.3).abs() < 1e-9);
        assert!((norms[2] - 0.2).abs() < 1e-9);
        // И сами баллы вычисляются (не паникуют).
        let scores = score_schemes(&schemes, &weights);
        assert!(scores[0].score > 0.0 && scores[1].score > 0.0);
    }

    #[test]
    fn no_admitted_schemes_means_none_scores() {
        let schemes = vec![scheme("a", true, 100.0, 50.0, 40.0)];
        let scores = score_schemes(&schemes, &ScoreWeights::default());
        assert_eq!(scores[0].score, 0.0);
        assert!(score_leader(&scores).is_none());
    }
}
