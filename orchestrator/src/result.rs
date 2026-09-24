//! Итоговый JSON-результат сессии: план, идентичность нагрузки, схемы
//! с агрегатами и прогонами, рекомендация (уровень + три вероятности).

use serde::{Deserialize, Serialize};

use powerbench_metrics::AggregateResult;
use powerbench_recommend::bootstrap::BootMode;
use powerbench_recommend::{EvidenceLevel, TieCriterion};

use crate::checkpoint::{Checkpoint, StoredRun};

/// Человекочитаемые подписи уровней доказательности.
pub fn evidence_level_label(level: EvidenceLevel) -> &'static str {
    match level {
        EvidenceLevel::Confirmed => "Подтверждено",
        EvidenceLevel::Probable => "Вероятно",
        EvidenceLevel::StabilityTieBreak => "Решено стабильностью",
        EvidenceLevel::Preliminary => "Предварительно",
        EvidenceLevel::KeepCurrent => "Оставить текущую",
        EvidenceLevel::Equivalent => "Эквиваленты",
        EvidenceLevel::None => "Нет данных",
    }
}

/// Уровень доказательности как машинный идентификатор.
pub fn evidence_level_id(level: EvidenceLevel) -> &'static str {
    match level {
        EvidenceLevel::Confirmed => "Confirmed",
        EvidenceLevel::Probable => "Probable",
        EvidenceLevel::StabilityTieBreak => "StabilityTieBreak",
        EvidenceLevel::Preliminary => "Preliminary",
        EvidenceLevel::KeepCurrent => "KeepCurrent",
        EvidenceLevel::Equivalent => "Equivalent",
        EvidenceLevel::None => "None",
    }
}

/// Режим bootstrap как строка.
pub fn bootstrap_mode_id(mode: BootMode) -> &'static str {
    match mode {
        BootMode::PairedByRun => "PairedByRun",
        BootMode::Independent => "Independent",
    }
}

/// Веса скоринга по умолчанию (производительность / стабильность / худшая
/// секунда, %). Источник значения — секция `scoring` настроек приложения.
pub fn default_score_weights() -> [f64; 3] {
    [50.0, 30.0, 20.0]
}

/// Признак разрешения ничьей как строка.
pub fn tie_criterion_id(criterion: TieCriterion) -> &'static str {
    match criterion {
        TieCriterion::P1 => "P1",
        TieCriterion::Stability => "Stability",
        TieCriterion::Cv => "Cv",
        TieCriterion::P01 => "P01",
        TieCriterion::Preference => "Preference",
        TieCriterion::None => "None",
    }
}

/// Идентичность нагрузки (CompatibilitySignature + детерминированные поля).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityJson {
    pub workload_version: String,
    pub config_hash: String,
    pub seed_hex: String,
    pub worker_count: usize,
    pub logical_cpus: usize,
    pub timer_hz: u64,
    pub cpu_identifier: String,
    pub diagnostics_version: String,
}

/// Одна схема в результате: агрегат + прогоны.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemeJson {
    pub scheme_id: String,
    /// Отображаемое имя плана (PlanName). Исключительно для вывода:
    /// идентификация ведётся только по `scheme_id` (guid).
    pub name: Option<String>,
    pub rejected: bool,
    pub rejection_reason: Option<String>,
    pub runs: usize,
    pub mean_average_throughput: f64,
    pub sample_std: f64,
    pub t_value: f64,
    pub margin: f64,
    pub ci_95: [f64; 2],
    pub run_variation_percent: f64,
    pub cv_warning: bool,
    pub median_throughput: f64,
    pub median_p1_throughput: f64,
    pub median_p01_throughput: f64,
    pub median_p95_execution_time_ms: f64,
    pub median_p99_execution_time_ms: f64,
    pub median_consistency_percent: f64,
    pub median_burst_retention_percent: f64,
    pub median_jitter_p99_ms: f64,
    /// Медиана худших секунд прогонов (тик/с).
    pub median_worst_window_throughput: f64,
    /// Медиана чистоты фона (%) — None, если данные недоступны.
    #[serde(default)]
    pub median_background_purity: Option<f64>,
    pub run_duration_ms: u64,
    pub started_at_min_ns: u64,
    pub per_run: Vec<StoredRun>,
}

impl SchemeJson {
    pub fn from_aggregate(
        scheme_id: String,
        rejected: bool,
        rejection_reason: Option<String>,
        aggregate: &AggregateResult,
        per_run: Vec<StoredRun>,
    ) -> Self {
        Self {
            scheme_id,
            name: per_run.first().and_then(|r| r.scheme_name.clone()),
            rejected,
            rejection_reason,
            runs: aggregate.runs,
            mean_average_throughput: aggregate.mean_average_throughput,
            sample_std: aggregate.sample_std,
            t_value: aggregate.t_value,
            margin: aggregate.margin,
            ci_95: aggregate.ci_95,
            run_variation_percent: aggregate.run_variation_percent,
            cv_warning: aggregate.cv_warning,
            median_throughput: aggregate.median_throughput,
            median_p1_throughput: aggregate.median_p1_throughput,
            median_p01_throughput: aggregate.median_p01_throughput,
            median_p95_execution_time_ms: aggregate.median_p95_execution_time_ms,
            median_p99_execution_time_ms: aggregate.median_p99_execution_time_ms,
            median_consistency_percent: aggregate.median_consistency_percent,
            median_burst_retention_percent: aggregate.median_burst_retention_percent,
            median_jitter_p99_ms: aggregate.median_jitter_p99_ms,
            median_worst_window_throughput: aggregate.median_worst_window_throughput,
            median_background_purity: aggregate.median_background_purity,
            run_duration_ms: aggregate.run_duration_ms,
            started_at_min_ns: aggregate.started_at_min_ns,
            per_run,
        }
    }
}

/// Рекомендация в результате.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecommendationJson {
    pub level: String,
    pub level_label: String,
    pub recommended_scheme: Option<String>,
    pub runner_up_scheme: Option<String>,
    pub reason: String,
    pub probabilities: Option<[f64; 3]>,
    pub expected_margin_percent: Option<f64>,
    pub bootstrap_mode: Option<String>,
    pub tie_criterion: Option<String>,
}

/// Итог сессии: JSON, готовый к записи на диск.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionJson {
    pub plan_guid: String,
    pub original_scheme_guid: Option<String>,
    pub original_restored: bool,
    pub identity: IdentityJson,
    pub schemes: Vec<SchemeJson>,
    pub recommendation: RecommendationJson,
    pub warnings: Vec<String>,
    /// Плановых раундов (повторов на схему) в сессии.
    #[serde(default)]
    pub rounds_planned: u32,
    /// Фактически выполненных раундов.
    #[serde(default)]
    pub rounds_completed: u32,
    /// Причина досрочного завершения (например, остановка вручную).
    #[serde(default)]
    pub early_stop_reason: Option<String>,
    /// Веса скоринга, действовавшие при сохранении (производительность,
    /// стабильность, худшая секунда), в процентах.
    #[serde(default)]
    pub score_weights: [f64; 3],
}

/// Построить машинный JSON из контрольной точки и рекомендации.
pub fn build_session_json(
    checkpoint: &Checkpoint,
    identity: IdentityJson,
    aggregated: Vec<(String, AggregateResult)>,
    recommendation_schemes: &[(String, bool, Option<String>, AggregateResult, Vec<StoredRun>)],
    recommendation: &powerbench_recommend::Recommendation,
    warnings: Vec<String>,
    cancelled: bool,
    score_weights: [f64; 3],
) -> SessionJson {
    let _ = &aggregated;
    let schemes = recommendation_schemes
        .iter()
        .map(|(id, rejected, reason, agg, per_run)| {
            SchemeJson::from_aggregate(id.clone(), *rejected, reason.clone(), agg, per_run.clone())
        })
        .collect();
    let rounds_planned = checkpoint.plan.repetitions;
    let rounds_completed = {
        let mut rounds: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        for run in &checkpoint.runs {
            rounds.insert(run.round);
        }
        rounds.len() as u32
    };
    let rounds_completed = rounds_completed.min(rounds_planned);
    let early_stop_reason = if cancelled || rounds_completed < rounds_planned {
        Some(if cancelled {
            "сессия остановлена вручную".to_string()
        } else {
            "сессия завершена досрочно".to_string()
        })
    } else {
        None
    };
    SessionJson {
        plan_guid: checkpoint.plan.plan_guid.clone(),
        original_scheme_guid: checkpoint.original_scheme_guid.clone(),
        original_restored: checkpoint.original_restored,
        identity,
        schemes,
        recommendation: RecommendationJson {
            level: evidence_level_id(recommendation.level).to_string(),
            level_label: evidence_level_label(recommendation.level).to_string(),
            recommended_scheme: recommendation.recommended_scheme.clone(),
            runner_up_scheme: recommendation.runner_up_scheme.clone(),
            reason: recommendation.reason.clone(),
            probabilities: recommendation.three_probabilities.map(|p: [f64; 3]| p),
            expected_margin_percent: recommendation.expected_margin_percent,
            bootstrap_mode: recommendation.bootstrap_mode.map(bootstrap_mode_id).map(String::from),
            tie_criterion: recommendation.tie_criterion.map(tie_criterion_id).map(String::from),
        },
        warnings,
        rounds_planned,
        rounds_completed,
        early_stop_reason,
        score_weights,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_map_to_russian() {
        assert_eq!(evidence_level_label(EvidenceLevel::Confirmed), "Подтверждено");
        assert_eq!(evidence_level_label(EvidenceLevel::None), "Нет данных");
        assert_eq!(evidence_level_label(EvidenceLevel::KeepCurrent), "Оставить текущую");
        assert_eq!(evidence_level_id(EvidenceLevel::None), "None");
        assert_eq!(bootstrap_mode_id(BootMode::Independent), "Independent");
        assert_eq!(tie_criterion_id(TieCriterion::Stability), "Stability");
    }

    #[test]
    fn probabilities_passed_through() {
        let _ = powerbench_recommend::bootstrap::BootstrapProbabilities {
            p_best: 0.9,
            p_margin_gt_0: 0.8,
            p_margin_gt_1pct: 0.7,
        };
    }
}