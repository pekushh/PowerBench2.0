//! Итоговый JSON-результат сессии: план, идентичность нагрузки, схемы
//! с агрегатами и прогонами, рекомендация (уровень + три вероятности).

use serde::{Deserialize, Serialize};

use powerbench_metrics::AggregateResult;
use powerbench_recommend::bootstrap::BootMode;
use powerbench_recommend::{EvidenceLevel, TieCriterion};

use crate::checkpoint::{Checkpoint, PhaseStats, StoredRun};

/// Строка рекомендации по схеме: идентификатор, активность, имя, агрегат, прогоны.
pub type RecommendationScheme = (
    String,
    bool,
    Option<String>,
    AggregateResult,
    Vec<StoredRun>,
);

/// Человекочитаемые подписи уровней доказательности.
pub fn evidence_level_label(level: EvidenceLevel) -> &'static str {
    match level {
        EvidenceLevel::Confirmed => "Подтверждено",
        EvidenceLevel::Probable => "Вероятно",
        EvidenceLevel::StabilityTieBreak => "Решено стабильностью",
        EvidenceLevel::Preliminary => "Предварительно",
        EvidenceLevel::Screening => "Скрининг",
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
        EvidenceLevel::Screening => "Screening",
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
///
/// Раньше здесь было 50/30/20. Средняя производительность и худшая секунда —
/// это один и тот же сигнал (throughput), просто взятый в разных точках
/// кривой, и на 50/30/20 вместе они получали 70 % веса. При этом среднее
/// определяется turbo-окном, где схемы питания ведут себя почти одинаково, а
/// расходятся они как раз на хвосте — там, где политика схемы (максимальное
/// состояние процессора, агрессивность разгона, охлаждение) уже вступила в
/// силу. Двадцать процентов на хвост недооценивали ровно то, ради чего замер
/// и делается.
///
/// 40/30/30: среднее остаётся заголовком, хвост получает равный голос,
/// стабильность сохраняет вес, но не перетягивает на себя итог — нестабильные
/// схемы и так отбрасываются автоматически.
pub fn default_score_weights() -> [f64; 3] {
    [40.0, 30.0, 30.0]
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
    /// Сборка и ревизия ОС (например, «22631.4169»).
    ///
    /// Обновление Windows меняет планировщик и политики питания, поэтому две
    /// сессии до и после патча нельзя считать сопоставимыми: раньше об этом
    /// можно было только догадываться.
    #[serde(default)]
    pub os_build: String,
    /// Объём физической памяти, ГБ.
    #[serde(default)]
    pub memory_gib: f64,
    /// Брендовое имя CPU для человека («AMD Ryzen 7 5800X3D»).
    #[serde(default)]
    pub cpu_brand: String,
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
    /// Метрики по фазам (медиана по прогонам).
    ///
    /// Три фазы измеряют разные вещи: лёгкая и «Отклик» реагируют на
    /// boost-политику схемы, тяжёлая упирается в лимиты мощности и может не
    /// отличаться вовсе. Усреднение их в одно число прячет случай, когда
    /// схема выигрывает в одной фазе и проигрывает в другой, — а это ровно
    /// тот вывод, который нужен пользователю, выбирающему схему.
    #[serde(default)]
    pub phases: Vec<PhaseSummaryJson>,
    pub run_duration_ms: u64,
    pub started_at_min_ns: u64,
    pub per_run: Vec<StoredRun>,
}

impl SchemeJson {
    /// Нефинитное значение (`NaN`/`inf`) в JSON пишется как `null`, а поле
    /// объявлено как `f64` — такой файл потом не читается (`load_result`
    /// падает, запись навсегда теряется для истории и отчёта). Поэтому все
    /// метрики проходят через `finite_or_zero` на входе в DTO.
    pub fn from_aggregate(
        scheme_id: String,
        rejected: bool,
        rejection_reason: Option<String>,
        aggregate: &AggregateResult,
        per_run: Vec<StoredRun>,
    ) -> Self {
        let a = aggregate;
        Self {
            scheme_id,
            name: per_run.first().and_then(|r| r.scheme_name.clone()),
            rejected,
            rejection_reason,
            runs: a.runs,
            mean_average_throughput: finite_or_zero(a.mean_average_throughput),
            sample_std: finite_or_zero(a.sample_std),
            t_value: finite_or_zero(a.t_value),
            margin: finite_or_zero(a.margin),
            ci_95: [finite_or_zero(a.ci_95[0]), finite_or_zero(a.ci_95[1])],
            run_variation_percent: finite_or_zero(a.run_variation_percent),
            cv_warning: a.cv_warning,
            median_throughput: finite_or_zero(a.median_throughput),
            median_p1_throughput: finite_or_zero(a.median_p1_throughput),
            median_p01_throughput: finite_or_zero(a.median_p01_throughput),
            median_p95_execution_time_ms: finite_or_zero(a.median_p95_execution_time_ms),
            median_p99_execution_time_ms: finite_or_zero(a.median_p99_execution_time_ms),
            median_consistency_percent: finite_or_zero(a.median_consistency_percent),
            median_burst_retention_percent: finite_or_zero(a.median_burst_retention_percent),
            median_jitter_p99_ms: finite_or_zero(a.median_jitter_p99_ms),
            median_worst_window_throughput: finite_or_zero(a.median_worst_window_throughput),
            median_background_purity: a.median_background_purity.map(finite_or_zero),
            run_duration_ms: a.run_duration_ms,
            started_at_min_ns: a.started_at_min_ns,
            phases: phase_summaries(&per_run),
            per_run,
        }
    }
}

/// Медианы по фазам из прогонов схемы.
///
/// Фазы идентифицируются индексом, а имена берутся из канонического списка:
/// так порядок и подписи не зависят от того, в каком порядке фазы попали в
/// прогон.
///
/// Подпись фазы по индексу; неизвестный индекс (старый JSON) не выдумывает
/// подпись.
fn phase_label_for(index: u8) -> &'static str {
    match powerbench_core::config::Phase::from_index(index) {
        Some(p) => p.label(),
        None => "—",
    }
}

/// Медианы по фазам из прогонов схемы.
fn phase_summaries(per_run: &[StoredRun]) -> Vec<PhaseSummaryJson> {
    let mut out: Vec<PhaseSummaryJson> = Vec::new();
    for idx in 0..=u8::MAX {
        let group: Vec<&PhaseStats> = per_run
            .iter()
            .flat_map(|r| r.phases.iter())
            .filter(|p| p.phase_index == idx)
            .collect();
        if group.is_empty() {
            continue;
        }
        let med = |f: fn(&PhaseStats) -> f64| -> f64 {
            let mut v: Vec<f64> = group
                .iter()
                .map(|p| finite_or_zero(f(p)))
                .filter(|x| *x > 0.0)
                .collect();
            if v.is_empty() {
                return 0.0;
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v[v.len() / 2]
        };
        out.push(PhaseSummaryJson {
            name: phase_label_for(idx).to_string(),
            median_throughput: med(|p| p.stats.average_throughput),
            p1_throughput: med(|p| p.stats.p1_throughput),
            consistency_percent: med(|p| p.stats.consistency_percent),
            throttled: group
                .iter()
                .any(|p| p.power.map(|x| x.throttled || x.thermal_throttle).unwrap_or(false)),
        });
    }
    out
}

/// Сводка по одной измеряемой фазе: медианы по прогонам.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PhaseSummaryJson {
    /// Название фазы («Лёгкая», «Тяжёлая», «Отклик», «Частичная»).
    pub name: String,
    /// Медиана throughput фазы, тик/с.
    pub median_throughput: f64,
    /// 1-й перцентиль throughput фазы (худшее поведение), тик/с.
    pub p1_throughput: f64,
    /// Медиана стабильности фазы, %.
    pub consistency_percent: f64,
    /// Наблюдался ли троттлинг частоты в этой фазе.
    pub throttled: bool,
}

/// `NaN`/`inf` → `0.0`; конечные значения проходят без изменений.
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
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

/// Порог разброса опорной схемы, выше которого вердикт понижается.
///
/// Смысл: если сама эталонная схема «плавает» сильнее, чем различаются
/// участники, то любое ранжирование — шум. 1.5 % — заметно типичный разброс
/// повторов на одной машине, поэтому граница именно такая: ниже неё разницу
/// между схемами видно, выше — уже нет.
pub const REFERENCE_SPAN_LIMIT_PERCENT: f64 = 1.5;

/// Наибольшее число прогонов на схему, при котором сессия считается
/// скринингом. При одном прогоне доверительный интервал не строится вовсе
/// (k − 1 = 0), и любое утверждение «схема A лучше B» опирается на единственный
/// замер. Такой результат годится, чтобы отсеять явно слабые схемы, но не для
/// выбора победителя.
pub const SCREENING_MAX_RUNS: usize = 1;

/// Порог фоновой нагрузки (p95 за прогон), при котором результат считается
/// измеренным на загруженной машине. Задаётся в % одного ядра и умножается на
/// число логических CPU при сравнении.
pub const BACKGROUND_P95_NOTE_PERCENT: f64 = 20.0;

/// Оценка стабильности машины по опорной схеме.
///
/// Сравнение «самой высокой из N схем» неявно опирается на то, что за все часы
/// замера машина вела себя одинаково. Допущение это нарушается постоянно (нагрев,
/// фон, обновления), а проверить его было нечем. Опорная схема меряется наравне
/// со всеми (дополнительного времени не требуется), а разброс её прогонов по
/// раундам показывает, насколько сама машина «плывёт» за сессию.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReferenceSummary {
    /// Идентификатор опорной схемы.
    pub scheme_id: String,
    /// Отображаемое имя опорной схемы.
    pub scheme_name: Option<String>,
    /// Throughput опорной схемы по раундам, тик/с.
    pub per_round: Vec<f64>,
    /// Размах по раундам относительно среднего, %.
    pub span_percent: f64,
    /// Наклон по раундам, % за раунд (линейный тренд).
    pub trend_percent_per_round: f64,
    /// Порог, при котором вердикт понижается: разброс эталона больше него
    /// означает, что машина плавает сильнее, чем различаются схемы.
    pub span_limit_percent: f64,
    /// Превышен ли порог.
    pub unstable: bool,
}

impl ReferenceSummary {
    /// Сводка по прогонам опорной схемы (уже отсортированным по раунду).
    ///
    /// `None`, если опорных прогонов меньше двух: по одному замеру дрейф не
    /// оценить, а выдавать за оценку ноль означало бы утверждать стабильность,
    /// которой никто не измерял.
    pub fn build(
        scheme_id: &str,
        scheme_name: Option<String>,
        per_round: Vec<f64>,
        span_limit_percent: f64,
    ) -> Option<Self> {
        let vals: Vec<f64> = per_round
            .into_iter()
            .filter(|v| v.is_finite() && *v > 0.0)
            .collect();
        if vals.len() < 2 {
            return None;
        }
        let n = vals.len() as f64;
        let mean = vals.iter().sum::<f64>() / n;
        let max = vals.iter().cloned().fold(f64::MIN, f64::max);
        let min = vals.iter().cloned().fold(f64::MAX, f64::min);
        let span_percent = if mean > 0.0 {
            (max - min) / mean * 100.0
        } else {
            0.0
        };
        // Линейный тренд по индексам раундов: Σ(x−x̄)(y−ȳ) / Σ(x−x̄)².
        let x_mean = (n - 1.0) / 2.0;
        let mut num = 0.0;
        let mut den = 0.0;
        for (i, y) in vals.iter().enumerate() {
            let dx = i as f64 - x_mean;
            num += dx * (y - mean);
            den += dx * dx;
        }
        let slope = if den > 0.0 { num / den } else { 0.0 };
        let trend_percent_per_round = if mean > 0.0 {
            slope / mean * 100.0
        } else {
            0.0
        };
        Some(Self {
            scheme_id: scheme_id.to_string(),
            scheme_name,
            per_round: vals,
            span_percent,
            trend_percent_per_round,
            span_limit_percent,
            unstable: span_percent > span_limit_percent,
        })
    }

    /// Пояснение одним предложением (для отчёта и подсказки в интерфейсе).
    pub fn note(&self) -> String {
        format!(
            "опорная схема: разброс {:.1} % по раундам (тренд {:+.1} %/раунд), порог {:.1} %",
            self.span_percent, self.trend_percent_per_round, self.span_limit_percent
        )
    }
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
    /// Оценка дрейфа машины по опорной схеме.
    #[serde(default)]
    pub reference: Option<ReferenceSummary>,
    /// Режим скрининга: прогона на схему недостаточно для ранжирования,
    /// сессия годна только для отбраковки очевидно слабых схем.
    #[serde(default)]
    pub screening: bool,
}

/// Построить машинный JSON из контрольной точки и рекомендации.
#[allow(clippy::too_many_arguments)]
pub fn build_session_json(
    checkpoint: &Checkpoint,
    identity: IdentityJson,
    aggregated: Vec<(String, AggregateResult)>,
    recommendation_schemes: &[RecommendationScheme],
    recommendation: &powerbench_recommend::Recommendation,
    warnings: Vec<String>,
    cancelled: bool,
    score_weights: [f64; 3],
) -> SessionJson {
    let _ = &aggregated;
    let schemes: Vec<SchemeJson> = recommendation_schemes
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
    // Дрейф по опорной схеме: её прогоны уже лежат в чекпоинте, отдельного
    // времени на эталон не тратится.
    let reference = checkpoint
        .plan
        .reference_scheme_id
        .as_ref()
        .and_then(|id| {
            let mut per_round: Vec<(u32, f64)> = checkpoint
                .runs
                .iter()
                .filter(|r| r.scheme_id.eq_ignore_ascii_case(id))
                .map(|r| (r.round, r.combined.average_throughput))
                .collect();
            per_round.sort_by_key(|(r, _)| *r);
            let name = checkpoint
                .runs
                .iter()
                .find(|r| r.scheme_id.eq_ignore_ascii_case(id))
                .and_then(|r| r.scheme_name.clone());
            ReferenceSummary::build(
                id,
                name,
                per_round.into_iter().map(|(_, v)| v).collect(),
                REFERENCE_SPAN_LIMIT_PERCENT,
            )
        });
    let screening = aggregated
        .iter()
        .filter(|(_, a)| a.runs > 0)
        .map(|(_, a)| a.runs)
        .min()
        .map(|min_runs| min_runs <= SCREENING_MAX_RUNS)
        .unwrap_or(false);
    // Условия среды, способные обесценить вердикт, собираются здесь, а не в
    // вызывающем коде: и приложение, и CLI должны понижать одинаково.
    let mut warnings = warnings;
    let mut level = recommendation.level;
    if screening {
        // Собственное предупреждение здесь было бы третьим повтором одного и
        // того же текста: уровень «Скрининг» уже назван в шапке отчёта и в
        // плашке результата, а пояснение печатается рядом с условиями замера.
        level = EvidenceLevel::Screening;
    }
    if let Some(r) = &reference {
        // Дрейф машины больше, чем разница между схемами: любой уровень выше
        // «Предварительно» здесь был бы обещанием, которого замер не поддерживает.
        if r.unstable && matches!(level, EvidenceLevel::Confirmed | EvidenceLevel::Probable) {
            level = EvidenceLevel::Preliminary;
        }
        if r.unstable {
            warnings.push(format!(
                "машина нестабильна: {} — вердикт понижен",
                r.note()
            ));
        }
    }
    // Фон по каждому прогону: p95 вместо одной пробы 700 мс перед замером.
    let mut bg_p95 = 0.0f64;
    let mut bg_runs: usize = 0;
    for s in &schemes {
        for r in &s.per_run {
            if r.background_cpu_p95 > bg_p95 {
                bg_p95 = r.background_cpu_p95;
            }
            if r.background_sample_seconds > 0 {
                bg_runs += 1;
            }
        }
    }
    if bg_runs > 0 && bg_p95 > BACKGROUND_P95_NOTE_PERCENT {
        warnings.push(format!(
            "фон на загруженной машине: до {:.0} % CPU (пиковое, 95-й перцентиль прогона)",
            bg_p95
        ));
    }
    // Троттлинг по фазам: если система ограничивала частоту, это нужно сказать
    // прямо — иначе «медленную» схему можно принять за неудачную.
    let throttled: Vec<String> = schemes
        .iter()
        .filter_map(|s| {
            let hit = s
                .per_run
                .iter()
                .flat_map(|r| r.phases.iter())
                .filter(|p| p.power.map(|x| x.throttled || x.thermal_throttle).unwrap_or(false))
                .count();
            (hit > 0).then(|| format!("{} ({} фаз)", s.name.clone().unwrap_or(s.scheme_id.clone()), hit))
        })
        .collect();
    if !throttled.is_empty() {
        warnings.push(format!(
            "троттлинг частоты наблюдался: {}",
            throttled.join(", ")
        ));
    }
    SessionJson {
        plan_guid: checkpoint.plan.plan_guid.clone(),
        original_scheme_guid: checkpoint.original_scheme_guid.clone(),
        original_restored: checkpoint.original_restored,
        identity,
        schemes,
        recommendation: RecommendationJson {
            level: evidence_level_id(level).to_string(),
            level_label: evidence_level_label(level).to_string(),
            recommended_scheme: recommendation.recommended_scheme.clone(),
            runner_up_scheme: recommendation.runner_up_scheme.clone(),
            reason: recommendation.reason.clone(),
            probabilities: recommendation.three_probabilities.map(|p: [f64; 3]| p),
            expected_margin_percent: recommendation.expected_margin_percent,
            bootstrap_mode: recommendation
                .bootstrap_mode
                .map(bootstrap_mode_id)
                .map(String::from),
            tie_criterion: recommendation
                .tie_criterion
                .map(tie_criterion_id)
                .map(String::from),
        },
        warnings,
        rounds_planned,
        rounds_completed,
        early_stop_reason,
        score_weights,
        reference,
        screening,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_map_to_russian() {
        assert_eq!(
            evidence_level_label(EvidenceLevel::Confirmed),
            "Подтверждено"
        );
        assert_eq!(evidence_level_label(EvidenceLevel::None), "Нет данных");
        assert_eq!(
            evidence_level_label(EvidenceLevel::KeepCurrent),
            "Оставить текущую"
        );
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

    /// Регресс: `NaN`/`inf` в агрегате (схема без прогонов) раньше писались
    /// в JSON как `null`, и запись истории становилась нечитаемой навсегда.
    #[test]
    fn non_finite_metrics_become_zero_and_survive_json() {
        let agg = AggregateResult {
            runs: 0,
            mean_average_throughput: f64::NAN,
            sample_std: f64::INFINITY,
            t_value: f64::NEG_INFINITY,
            margin: f64::NAN,
            ci_95: [f64::NAN, f64::INFINITY],
            run_variation_percent: f64::NAN,
            cv_warning: false,
            median_throughput: f64::NAN,
            median_p1_throughput: f64::NAN,
            median_p01_throughput: f64::NAN,
            median_p95_execution_time_ms: f64::NAN,
            median_p99_execution_time_ms: f64::NAN,
            median_consistency_percent: f64::NAN,
            median_burst_retention_percent: f64::NAN,
            median_jitter_p99_ms: f64::NAN,
            median_worst_window_throughput: f64::NAN,
            median_background_purity: Some(f64::NAN),
            run_duration_ms: 0,
            started_at_min_ns: 0,
        };
        let sch = SchemeJson::from_aggregate("g".into(), true, Some("брак".into()), &agg, Vec::new());
        // Метрики — числа: `null` означал бы, что файл не прочитается обратно.
        // `name` здесь `Option<String>`, его `null` допустим.
        let text = serde_json::to_string(&sch).expect("сериализация");
        for field in [
            "mean_average_throughput",
            "median_throughput",
            "run_variation_percent",
            "median_consistency_percent",
            "median_background_purity",
        ] {
            assert!(
                !text.contains(&format!("\"{field}\":null")),
                "поле {field} записано как null: {text}"
            );
        }
        let back: SchemeJson = serde_json::from_str(&text).expect("десериализация");
        assert_eq!(back.median_throughput, 0.0);
        assert_eq!(back.ci_95, [0.0, 0.0]);
        assert_eq!(back.median_background_purity, Some(0.0));
    }

    #[test]
    fn finite_metrics_pass_through_unchanged() {
        assert_eq!(finite_or_zero(12.5), 12.5);
        assert_eq!(finite_or_zero(-0.0), -0.0);
        assert_eq!(finite_or_zero(f64::NAN), 0.0);
        assert_eq!(finite_or_zero(f64::INFINITY), 0.0);
    }
}
