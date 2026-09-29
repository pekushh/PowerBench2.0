//! Движок рекомендаций PlanRecommendationV3 (чистые функции).
//!
//! Вход — агрегаты схем + компактные сводки отдельных прогонов; выход — одна
//! рекомендация + уровень доказательности + ровно три вероятности. Сырые
//! массивы сэмплов движок не использует и не хранит.

pub mod bootstrap;
pub mod calibrate;

use std::cmp::Ordering;

use powerbench_metrics::{AggregateResult, CompatibilitySignature, DeterminismSignature};

use crate::bootstrap::{BootMode, BootstrapProbabilities, bootstrap_mode, bootstrap_probabilities};

/// Практическая ничья: кандидаты в пределах 1% от лидера по среднему.
pub const PRACTICAL_TIE_PERCENT: f64 = 0.01;
/// Перевес по P1 ≥ 0.5% (относительно).
pub const P1_TIE_LEAD_PERCENT: f64 = 0.005;
/// ...при минимуме 100 измерений в каждом сравниваемом прогоне.
pub const P1_TIE_MIN_SAMPLES: usize = 100;
/// Перевес по стабильности ≥ 1 процентный пункт (абсолютно).
pub const STABILITY_TIE_LEAD_PP: f64 = 1.0;
/// Перевес по CV ≥ 0.5 п.п. (абсолютно, меньше — лучше).
pub const CV_TIE_LEAD_PP: f64 = 0.5;
/// ...при минимуме 3 прогонах у каждого.
pub const CV_TIE_MIN_RUNS: usize = 3;
/// Перевес по P0.1 ≥ 1% (относительно).
pub const P01_TIE_LEAD_PERCENT: f64 = 0.01;
/// ...при минимуме 10 000 измерений во всех прогонах всех связанных кандидатов.
pub const P01_TIE_MIN_SAMPLES: usize = 10_000;

/// Пороги уровней доказательности.
pub const CONFIRMED_MIN_RUNS: usize = 3;
pub const CONFIRMED_MAX_CV_PERCENT: f64 = 2.0;
pub const CONFIRMED_MIN_MARGIN_PERCENT: f64 = 1.0;
pub const CONFIRMED_MIN_P_GT_1PCT: f64 = 0.95;
pub const CONFIRMED_MIN_P_BEST: f64 = 0.90;
pub const PROBABLE_MIN_RUNS: usize = 3;
pub const PROBABLE_MIN_P_GT_0: f64 = 0.80;
pub const PROBABLE_MIN_P_BEST: f64 = 0.60;

/// Компактная сводка одного прогона (движок не использует сырые сэмплы).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunCompact {
    pub average_throughput: f64,
    pub median_throughput: f64,
    pub p1_throughput: f64,
    pub p01_throughput: f64,
    pub consistency_percent: f64,
    /// Число валидных измерений прогона (для гейтов ничьи: ≥100 и ≥10 000).
    pub samples: usize,
}

impl RunCompact {
    pub fn from_stats(s: &powerbench_metrics::RunStats) -> Self {
        Self {
            average_throughput: s.average_throughput,
            median_throughput: s.median_throughput,
            p1_throughput: s.p1_throughput,
            p01_throughput: s.p01_throughput,
            consistency_percent: s.consistency_percent,
            samples: s.samples,
        }
    }
}

/// Кандидат — схема питания с её агрегатом и компактными сводками прогонов.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemeAggregate {
    pub scheme_id: String,
    /// Результат забракован (не брать в расчёт).
    pub rejected: bool,
    pub signature: CompatibilitySignature,
    pub determinism: DeterminismSignature,
    pub aggregate: AggregateResult,
    pub runs: Vec<RunCompact>,
    /// Исходная схема пользователя (нейтральное предпочтение №1).
    pub is_original: bool,
    /// Активная схема (нейтральное предпочтение №2).
    pub is_active: bool,
}

impl SchemeAggregate {
    fn admitted(&self) -> bool {
        !self.rejected
            && !self.scheme_id.is_empty()
            && !self.runs.is_empty()
            && self.aggregate.mean_average_throughput.is_finite()
            && self.aggregate.mean_average_throughput > 0.0
    }
}

/// Причина уровня `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoneReason {
    /// Нет допущенных кандидатов.
    NoCandidates,
    /// Дубли ID схемы.
    DuplicateIds,
    /// Несовместимые сигнатуры.
    IncompatibleSignatures,
}

/// Уровень доказательности рекомендации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceLevel {
    Confirmed,
    Probable,
    StabilityTieBreak,
    Preliminary,
    /// Скрининг: прогона на схему недостаточно, чтобы строить доверительный
    /// интервал (k − 1 = 0). Такой замер годен, чтобы отсеять явно слабые
    /// схемы, но не для выбора победителя, и называть его «Предварительно»
    /// значило бы обещать больше, чем он даёт.
    Screening,
    KeepCurrent,
    Equivalent,
    None,
}

/// Признак, разрешивший практическую ничью.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieCriterion {
    P1,
    Stability,
    Cv,
    P01,
    Preference,
    None,
}

/// Рекомендация.
#[derive(Debug, Clone, PartialEq)]
pub struct Recommendation {
    pub level: EvidenceLevel,
    pub recommended_scheme: Option<String>,
    pub runner_up_scheme: Option<String>,
    pub reason: String,
    /// Ровно три вероятности:
    /// `[P(кандидат лучший), P(перевес > 0), P(перевес > 1%)]`.
    pub three_probabilities: Option<[f64; 3]>,
    /// Ожидаемый перевес над вторым местом (относительный, %).
    pub expected_margin_percent: Option<f64>,
    pub bootstrap_mode: Option<BootMode>,
    pub tie_criterion: Option<TieCriterion>,
}

fn primary_cmp(a: &SchemeAggregate, b: &SchemeAggregate) -> Ordering {
    // 1. AverageThroughput (выше лучше);
    // 2. MedianThroughput; 3. P1Throughput;
    // 4. ConsistencyPercent (выше лучше); 5. меньший CV повторов;
    // 6. исходная, затем активная; 7. ID схемы — последний детерминированный ключ.
    let da = &a.aggregate;
    let db = &b.aggregate;
    if let Some(o) = cmp_f64(da.mean_average_throughput, db.mean_average_throughput).reversed_o() {
        return o;
    }
    if let Some(o) = cmp_f64(da.median_throughput, db.median_throughput).reversed_o() {
        return o;
    }
    if let Some(o) = cmp_f64(da.median_p1_throughput, db.median_p1_throughput).reversed_o() {
        return o;
    }
    if let Some(o) =
        cmp_f64(da.median_consistency_percent, db.median_consistency_percent).reversed_o()
    {
        return o;
    }
    // Меньший CV впереди — порядок обычный (не реверс). Равенство CV — не ключ.
    match cmp_f64(da.run_variation_percent, db.run_variation_percent) {
        OrderingLike::Equal => {}
        OrderingLike::Some(Ordering::Equal) => {}
        OrderingLike::Some(o) => return o,
    }
    let pref_a = preference_key(a);
    let pref_b = preference_key(b);
    if pref_a != pref_b {
        return pref_b.cmp(&pref_a);
    }
    a.scheme_id.cmp(&b.scheme_id)
}

fn preference_key(c: &SchemeAggregate) -> (u8, u8) {
    let original = if c.is_original { 1 } else { 0 };
    let active = if c.is_active { 1 } else { 0 };
    (original, active)
}

#[derive(Debug)]
enum OrderingLike {
    Equal,
    Some(std::cmp::Ordering),
}

fn cmp_f64(a: f64, b: f64) -> OrderingLike {
    match a.partial_cmp(&b) {
        Some(o) => OrderingLike::Some(o),
        None => OrderingLike::Equal,
    }
}

impl OrderingLike {
    /// Как `Option<Ordering>`, но математическое равенство и NaN оба дают `None`
    /// (равенство не должно обрывать сравнение следующих ключей).
    fn reversed_o(self) -> Option<Ordering> {
        match self {
            OrderingLike::Equal => None,
            OrderingLike::Some(Ordering::Equal) => None,
            OrderingLike::Some(o) => Some(o.reverse()),
        }
    }
}

/// Формирование основной рекомендации по кандидатам.
///
/// `expected_runs` — ожидаемое число прогонов (для выбора режима bootstrap
/// PairedByRun против Independent).
pub fn recommend(items: &[SchemeAggregate], expected_runs: usize) -> Recommendation {
    let none = |_reason: NoneReason, text: &str| Recommendation {
        level: EvidenceLevel::None,
        recommended_scheme: None,
        runner_up_scheme: None,
        reason: text.to_string(),
        three_probabilities: None,
        expected_margin_percent: None,
        bootstrap_mode: None,
        tie_criterion: None,
    };

    // --- Допуск и совместимость ---
    let admitted: Vec<&SchemeAggregate> = items.iter().filter(|c| c.admitted()).collect();
    if admitted.is_empty() {
        return none(NoneReason::NoCandidates, "нет допущенных кандидатов");
    }
    let mut ids: Vec<&str> = admitted.iter().map(|c| c.scheme_id.as_str()).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|w| w[0] == w[1]) {
        return none(
            NoneReason::DuplicateIds,
            "ID схем у кандидатов не уникальны",
        );
    }
    let first = admitted[0];
    for c in &admitted[1..] {
        if c.signature != first.signature || c.determinism != first.determinism {
            return none(
                NoneReason::IncompatibleSignatures,
                "несовместимые сигнатуры кандидатов: такие результаты не ранжируются вместе",
            );
        }
    }

    // --- Основной порядок (детерминированный) ---
    // Вход сортируется до сэмплирования, чтобы порядок коллекции не влиял
    // на результат bootstrap.
    let mut order: Vec<usize> = (0..admitted.len()).collect();
    order.sort_by(|&i, &j| primary_cmp(admitted[i], admitted[j]));
    let sorted: Vec<&SchemeAggregate> = order.iter().map(|&i| admitted[i]).collect();

    if sorted.len() == 1 {
        // Единственная допущенная схема: вероятности не вычисляем. «P(лучший) =
        // 100 %» при отсутствии конкурента — не результат, а тавтология, и в
        // отчёте она читалась как «100 % вероятность, что выбран лучший».
        return Recommendation {
            level: EvidenceLevel::Preliminary,
            recommended_scheme: Some(sorted[0].scheme_id.clone()),
            runner_up_scheme: None,
            reason: "мало данных: единственная допущенная схема, сравнивать не с чем"
                .to_string(),
            three_probabilities: None,
            expected_margin_percent: None,
            bootstrap_mode: None,
            tie_criterion: Some(TieCriterion::None),
        };
    }

    // --- Практическая ничья ---
    let leader_mean = sorted[0].aggregate.mean_average_throughput;
    let tie_indices: Vec<usize> = (0..sorted.len())
        .filter(|&i| {
            i == 0
                || (leader_mean - sorted[i].aggregate.mean_average_throughput)
                    <= leader_mean * PRACTICAL_TIE_PERCENT
        })
        .collect();

    if tie_indices.len() >= 2 {
        let (winner_idx, criterion, level) = resolve_tie(&sorted, &tie_indices);
        let winner = sorted[winner_idx];
        // Раннер — лучший НЕ победитель (иначе сравнение с самим собой
        // даёт P(перевес>0) = 0, а при победе 3-го+ — отрицательный перевес).
        let runner_idx = if winner_idx == 0 { 1 } else { 0 };
        let (probs, mode) = bootstrap_pair(&sorted, winner_idx, Some(runner_idx), expected_runs);
        let margin = margin_percent(
            winner.aggregate.mean_average_throughput,
            sorted[runner_idx].aggregate.mean_average_throughput,
        );
        // Ничью развели не по среднему, поэтому победитель может иметь худшую
        // среднюю: «перевес» тогда отрицателен, и печатать его как «−0.5 %»
        // бессмысленно — разницы по скорости нет вовсе, решение было о другом.
        let margin = margin.filter(|m| *m > 0.0);
        // И вероятности «перевес > 0» тут недоопределены: победитель выбран по
        // стабильности, а не по скорости, поэтому их не публикуем.
        let probs = [probs.p_best, 0.0, 0.0];
        let reason = match criterion {
            TieCriterion::P1 => {
                "практическая ничья по среднему, разрешена по перевесу P1".to_string()
            }
            TieCriterion::Stability => {
                "практическая ничья по среднему, разрешена по стабильности".to_string()
            }
            TieCriterion::Cv => {
                "практическая ничья по среднему, разрешена по меньшему CV повторов".to_string()
            }
            TieCriterion::P01 => {
                "практическая ничья по среднему, разрешена по перевесу P0.1".to_string()
            }
            TieCriterion::Preference => {
                "значимого различия по среднему нет, выбрана текущая схема".to_string()
            }
            TieCriterion::None => "значимого различия нет — схемы эквивалентны".to_string(),
        };
        return Recommendation {
            level,
            recommended_scheme: Some(winner.scheme_id.clone()),
            runner_up_scheme: Some(sorted[runner_idx].scheme_id.clone()),
            reason,
            three_probabilities: Some(probs),
            expected_margin_percent: margin,
            bootstrap_mode: Some(mode),
            tie_criterion: Some(criterion),
        };
    }

    // --- Нет ничьи: уровни доказательности ---
    let winner = &sorted[0];
    let runner = &sorted[1];
    let margin = margin_percent(
        winner.aggregate.mean_average_throughput,
        runner.aggregate.mean_average_throughput,
    )
    .unwrap_or(0.0);
    let (probs, mode) = bootstrap_pair(&sorted, 0, Some(1), expected_runs);

    let both_at_least_3 =
        winner.run_count() >= CONFIRMED_MIN_RUNS && runner.run_count() >= CONFIRMED_MIN_RUNS;

    let level = if both_at_least_3
        && winner.aggregate.run_variation_percent <= CONFIRMED_MAX_CV_PERCENT
        && runner.aggregate.run_variation_percent <= CONFIRMED_MAX_CV_PERCENT
        && margin >= CONFIRMED_MIN_MARGIN_PERCENT
        && probs.p_margin_gt_1pct >= CONFIRMED_MIN_P_GT_1PCT
        && probs.p_best >= CONFIRMED_MIN_P_BEST
    {
        EvidenceLevel::Confirmed
    } else if both_at_least_3
        && margin > 0.0
        && probs.p_margin_gt_0 >= PROBABLE_MIN_P_GT_0
        && probs.p_best >= PROBABLE_MIN_P_BEST
    {
        EvidenceLevel::Probable
    } else {
        EvidenceLevel::Preliminary
    };

    let reason = match level {
        EvidenceLevel::Confirmed => {
            "перевес подтверждён статистически (ДИ и bootstrap устойчивы)".to_string()
        }
        EvidenceLevel::Probable => {
            "перевес вероятен, но данных/CV недостаточно для полного подтверждения".to_string()
        }
        _ => "менее 3 прогонов или высокая неопределённость".to_string(),
    };

    Recommendation {
        level,
        recommended_scheme: Some(winner.scheme_id.clone()),
        runner_up_scheme: Some(runner.scheme_id.clone()),
        reason,
        three_probabilities: Some([probs.p_best, probs.p_margin_gt_0, probs.p_margin_gt_1pct]),
        expected_margin_percent: margin_percent(
            winner.aggregate.mean_average_throughput,
            runner.aggregate.mean_average_throughput,
        ),
        bootstrap_mode: Some(mode),
        tie_criterion: Some(TieCriterion::None),
    }
}

impl SchemeAggregate {
    fn run_count(&self) -> usize {
        self.runs.len()
    }
}

/// Ожидаемый перевес по среднему (относительный, %).
fn margin_percent(winner: f64, runner: f64) -> Option<f64> {
    if runner > 0.0 && winner.is_finite() && runner.is_finite() {
        Some((winner - runner) / runner * 100.0)
    } else {
        None
    }
}

/// Bootstrap для пары «победитель — второе место» поверх средних прогонов.
fn bootstrap_pair(
    sorted: &[&SchemeAggregate],
    winner: usize,
    runner: Option<usize>,
    expected_runs: usize,
) -> (BootstrapProbabilities, BootMode) {
    let mode = bootstrap_mode(sorted, expected_runs);
    let probs = bootstrap_probabilities(sorted, mode, winner, runner);
    (probs, mode)
}

fn resolve_tie(sorted: &[&SchemeAggregate], tie: &[usize]) -> (usize, TieCriterion, EvidenceLevel) {
    // Первый применимый отличительный признак.
    for criterion in [
        TieCriterion::P1,
        TieCriterion::Stability,
        TieCriterion::Cv,
        TieCriterion::P01,
    ] {
        if let Some(winner) = separate_by(sorted, tie, criterion) {
            return (winner, criterion, EvidenceLevel::StabilityTieBreak);
        }
    }
    // Признак 5: исходная, затем активная схема.
    if let Some(&i) = tie.iter().find(|&&i| sorted[i].is_original) {
        return (i, TieCriterion::Preference, EvidenceLevel::KeepCurrent);
    }
    if let Some(&i) = tie.iter().find(|&&i| sorted[i].is_active) {
        return (i, TieCriterion::Preference, EvidenceLevel::KeepCurrent);
    }
    // Иначе — лидер основного порядка со статусом «Эквиваленты».
    (tie[0], TieCriterion::None, EvidenceLevel::Equivalent)
}

/// Отделить лучшего из связанных кандидатов по признаку `criterion`.
/// Возвращает `Some(индекс победителя)`, только если гейт пройден и перевес
/// соответствует порогу.
fn separate_by(
    sorted: &[&SchemeAggregate],
    tie: &[usize],
    criterion: TieCriterion,
) -> Option<usize> {
    if tie.len() < 2 {
        return None;
    }
    let val = |i: usize| -> f64 {
        let a = &sorted[i].aggregate;
        match criterion {
            TieCriterion::P1 => a.median_p1_throughput,
            TieCriterion::Stability => a.median_consistency_percent,
            TieCriterion::Cv => a.run_variation_percent,
            TieCriterion::P01 => a.median_p01_throughput,
            TieCriterion::Preference | TieCriterion::None => f64::NAN,
        }
    };

    // Два лучших по признаку (при равенстве — порядок первичной сортировки).
    // Для CV меньше — лучше, поэтому ранг по возрастанию.
    // cmp(val(j), val(i)) ставит больший val первым (убывание); для CV — наоборот.
    let ascending = criterion == TieCriterion::Cv;
    let mut rank: Vec<usize> = tie.to_vec();
    rank.sort_by(|&i, &j| {
        let o = if ascending {
            cmp_f64(val(i), val(j))
        } else {
            cmp_f64(val(j), val(i))
        };
        match o {
            OrderingLike::Equal => i.cmp(&j),
            OrderingLike::Some(ord) => ord,
        }
    });

    let top = rank[0];
    let second = rank[1];
    if !val(top).is_finite() || !val(second).is_finite() || val(top) == val(second) {
        return None;
    }

    // Гейты признаков.
    let runs_ok = |i: usize, min_samples: usize, min_runs: usize| -> bool {
        let c = sorted[i];
        c.runs.len() >= min_runs && c.runs.iter().all(|r| r.samples >= min_samples)
    };
    let all_ok = |min_samples: usize| -> bool { tie.iter().all(|&i| runs_ok(i, min_samples, 1)) };

    let lead = match criterion {
        TieCriterion::P1 => {
            if !(runs_ok(top, P1_TIE_MIN_SAMPLES, 1) && runs_ok(second, P1_TIE_MIN_SAMPLES, 1)) {
                return None;
            }
            let v2 = val(second);
            if v2 <= 0.0 {
                return None;
            }
            (val(top) - v2) / v2
        }
        TieCriterion::Stability => val(top) - val(second),
        TieCriterion::Cv => {
            if !(runs_ok(top, 1, CV_TIE_MIN_RUNS) && runs_ok(second, 1, CV_TIE_MIN_RUNS)) {
                return None;
            }
            // CV меньше — лучше: перевес = улучшение второго над первым.
            val(second) - val(top)
        }
        TieCriterion::P01 => {
            if !all_ok(P01_TIE_MIN_SAMPLES) {
                return None;
            }
            let v2 = val(second);
            if v2 <= 0.0 {
                return None;
            }
            (val(top) - v2) / v2
        }
        TieCriterion::Preference | TieCriterion::None => return None,
    };

    let threshold = match criterion {
        TieCriterion::P1 => P1_TIE_LEAD_PERCENT,
        TieCriterion::Stability => STABILITY_TIE_LEAD_PP,
        TieCriterion::Cv => CV_TIE_LEAD_PP,
        TieCriterion::P01 => P01_TIE_LEAD_PERCENT,
        TieCriterion::Preference | TieCriterion::None => 0.0,
    };

    if lead >= threshold { Some(top) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use powerbench_metrics::DeterminismSignature;

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

    /// Руками заданная сводка кандидата без прогонов с ненулевой длиной.
    #[allow(clippy::too_many_arguments)]
    fn scheme(
        id: &str,
        avg: f64,
        median: f64,
        p1: f64,
        p01: f64,
        consistency: f64,
        cv: f64,
        runs: &[(f64, usize)],
        original: bool,
        active: bool,
    ) -> SchemeAggregate {
        let runs_c: Vec<RunCompact> = runs
            .iter()
            .map(|&(m, s)| RunCompact {
                average_throughput: m,
                median_throughput: m,
                p1_throughput: m * 0.99,
                p01_throughput: m * 0.97,
                consistency_percent: consistency,
                samples: s,
            })
            .collect();
        let means: Vec<f64> = runs.iter().map(|&(m, _)| m).collect();
        let agg = AggregateResult {
            runs: means.len(),
            mean_average_throughput: avg,
            sample_std: 0.0,
            t_value: 0.0,
            margin: 0.0,
            ci_95: [avg - cv / 100.0 * avg, avg + cv / 100.0 * avg],
            run_variation_percent: cv,
            cv_warning: cv > 3.0,
            median_throughput: median,
            median_p1_throughput: p1,
            median_p01_throughput: p01,
            median_p95_execution_time_ms: 0.0,
            median_p99_execution_time_ms: 0.0,
            median_consistency_percent: consistency,
            median_burst_retention_percent: 0.0,
            median_jitter_p99_ms: 0.0,
            median_worst_window_throughput: median * 0.9,
            median_background_purity: None,
            run_duration_ms: 0,
            started_at_min_ns: 0,
        };
        SchemeAggregate {
            scheme_id: id.to_string(),
            rejected: false,
            signature: sig(),
            determinism: DeterminismSignature::new(vec![1, 2, 3]),
            aggregate: agg,
            runs: runs_c,
            is_original: original,
            is_active: active,
        }
    }

    #[test]
    fn admission_refuses_empty_duplicates_and_incompatible() {
        // Пустой вход.
        let r = recommend(&[], 3);
        assert_eq!(r.level, EvidenceLevel::None);
        assert_eq!(r.reason, "нет допущенных кандидатов");

        // Все забракованы.
        let mut c = scheme(
            "A",
            1000.0,
            1000.0,
            900.0,
            800.0,
            90.0,
            1.0,
            &[(1000.0, 200)],
            false,
            false,
        );
        c.rejected = true;
        let r = recommend(&[c.clone()], 3);
        assert_eq!(r.level, EvidenceLevel::None);
        assert!(r.reason.contains("нет допущенных кандидатов"));

        // Дубли ID.
        let a = scheme(
            "A",
            1000.0,
            1000.0,
            900.0,
            800.0,
            90.0,
            1.0,
            &[(1000.0, 200)],
            false,
            false,
        );
        let b = scheme(
            "A",
            999.0,
            900.0,
            800.0,
            700.0,
            85.0,
            2.0,
            &[(999.0, 200)],
            false,
            false,
        );
        let r = recommend(&[a, b], 3);
        assert_eq!(r.level, EvidenceLevel::None);
        assert_eq!(r.reason, "ID схем у кандидатов не уникальны");

        // Несовместимые сигнатуры.
        let x = scheme(
            "X",
            1000.0,
            1000.0,
            900.0,
            800.0,
            90.0,
            1.0,
            &[(1000.0, 200)],
            false,
            false,
        );
        let mut y = scheme(
            "Y",
            999.0,
            900.0,
            800.0,
            700.0,
            85.0,
            2.0,
            &[(999.0, 200)],
            false,
            false,
        );
        y.signature.config_hash = "BBBB".to_string();
        let r = recommend(&[x, y], 3);
        assert_eq!(r.level, EvidenceLevel::None);
        assert_eq!(
            r.reason,
            "несовместимые сигнатуры кандидатов: такие результаты не ранжируются вместе"
        );

        // Невалидный средний throughput исключается из допуска.
        let z = scheme("Z", f64::NAN, 0.0, 0.0, 0.0, 0.0, 0.0, &[], false, false);
        let ok = scheme(
            "W",
            950.0,
            950.0,
            850.0,
            800.0,
            90.0,
            1.0,
            &[(950.0, 200)],
            false,
            false,
        );
        let r = recommend(&[z, ok], 3);
        assert!(matches!(r.level, EvidenceLevel::Preliminary));
    }

    /// Базовые кандидаты для тестов ничьи и порядков: A — лидер (1000),
    /// B — «в пределах 1%» (995).
    fn tie_pair() -> (SchemeAggregate, SchemeAggregate) {
        (
            scheme(
                "A",
                1000.0,
                1000.0,
                800.0,
                700.0,
                90.0,
                2.0,
                &[(1000.0, 200); 3],
                false,
                false,
            ),
            scheme(
                "B",
                995.0,
                995.0,
                800.0,
                700.0,
                90.0,
                2.0,
                &[(995.0, 200); 3],
                false,
                false,
            ),
        )
    }

    #[test]
    fn practical_tie_resolved_by_p1_with_sample_guard() {
        let (mut a, mut b) = tie_pair();
        // B лучше по P1 на 12.5% при 200 измерений в каждом прогоне.
        a.aggregate.median_p1_throughput = 800.0;
        b.aggregate.median_p1_throughput = 900.0;
        let r = recommend(&[a, b], 3);
        assert_eq!(r.level, EvidenceLevel::StabilityTieBreak);
        assert_eq!(r.recommended_scheme.as_deref(), Some("B"));
        assert_eq!(r.tie_criterion, Some(TieCriterion::P1));
    }

    #[test]
    fn practical_tie_p1_guard_blocks_with_few_samples() {
        let (a, mut b) = tie_pair();
        // Гейт P1 требует ≥ 100 измерений в каждом прогоне — 50 запрещает.
        b.runs = vec![
            crate::RunCompact {
                average_throughput: 995.0,
                median_throughput: 995.0,
                p1_throughput: 900.0,
                p01_throughput: 700.0,
                consistency_percent: 90.0,
                samples: 50,
            },
            crate::RunCompact {
                average_throughput: 995.0,
                median_throughput: 995.0,
                p1_throughput: 900.0,
                p01_throughput: 700.0,
                consistency_percent: 90.0,
                samples: 50,
            },
            crate::RunCompact {
                average_throughput: 995.0,
                median_throughput: 995.0,
                p1_throughput: 900.0,
                p01_throughput: 700.0,
                consistency_percent: 90.0,
                samples: 50,
            },
        ];
        b.aggregate.median_p1_throughput = 900.0;
        // Никакой признак не применим, предпочтений нет → Эквиваленты.
        let r = recommend(&[a, b], 3);
        assert_eq!(r.level, EvidenceLevel::Equivalent);
        assert_eq!(r.recommended_scheme.as_deref(), Some("A"));
        assert_eq!(r.tie_criterion, Some(TieCriterion::None));
    }

    #[test]
    fn practical_tie_resolved_by_stability() {
        let (mut a, b) = tie_pair();
        // B стабильнее на 5 процентных пунктов (≥1 п.п.).
        a.aggregate.median_consistency_percent = 90.0;
        let mut b2 = b;
        b2.aggregate.median_consistency_percent = 95.0;
        let r = recommend(&[a, b2], 3);
        assert_eq!(r.level, EvidenceLevel::StabilityTieBreak);
        assert_eq!(r.tie_criterion, Some(TieCriterion::Stability));
        assert_eq!(r.recommended_scheme.as_deref(), Some("B"));
    }

    #[test]
    fn practical_tie_resolved_by_lower_cv_with_three_runs() {
        let (mut a, b) = tie_pair();
        // CV: A = 3.0, B = 2.0 → перевес 1.0 п.п. (меньше CV — лучше).
        a.aggregate.run_variation_percent = 3.0;
        let mut b2 = b;
        b2.aggregate.run_variation_percent = 2.0;
        let r = recommend(&[a, b2], 3);
        assert_eq!(r.level, EvidenceLevel::StabilityTieBreak);
        assert_eq!(r.tie_criterion, Some(TieCriterion::Cv));
        assert_eq!(r.recommended_scheme.as_deref(), Some("B"));
    }

    #[test]
    fn practical_tie_cv_guard_blocks_with_two_runs() {
        let mut a = scheme(
            "A",
            1000.0,
            1000.0,
            800.0,
            700.0,
            90.0,
            3.0,
            &[(1000.0, 200); 2],
            false,
            false,
        );
        let mut b = scheme(
            "B",
            995.0,
            995.0,
            800.0,
            700.0,
            90.0,
            2.0,
            &[(995.0, 200); 2],
            false,
            false,
        );
        a.aggregate.run_variation_percent = 3.0;
        b.aggregate.run_variation_percent = 2.0;
        let r = recommend(&[a, b], 3);
        // Гейт CV требует ≥ 3 прогонов у каждого — не выполнен, предпочтений нет.
        assert_eq!(r.level, EvidenceLevel::Equivalent);
    }

    #[test]
    fn practical_tie_resolved_by_p01_with_ten_thousand_samples() {
        // B лучше по P0.1 на ~2.1%; все прогоны ≥ 10 000 измерений — гейт P01 открыт.
        let a = scheme(
            "A",
            1000.0,
            1000.0,
            800.0,
            700.0,
            90.0,
            2.0,
            &[(1000.0, 20000); 3],
            false,
            false,
        );
        let b = scheme(
            "B",
            995.0,
            995.0,
            800.0,
            715.0,
            90.0,
            2.0,
            &[(995.0, 20000); 3],
            false,
            false,
        );
        let r = recommend(&[a, b], 3);

        assert_eq!(r.level, EvidenceLevel::StabilityTieBreak);
        assert_eq!(r.tie_criterion, Some(TieCriterion::P01));
        assert_eq!(r.recommended_scheme.as_deref(), Some("B"));
    }

    #[test]
    fn practical_tie_keep_current_for_original_then_active() {
        // Исходная схема.
        let (a, b) = tie_pair();
        let rb = recommend(&[a.clone(), b.clone().mut_set(|c| c.is_original = true)], 3);
        assert_eq!(rb.level, EvidenceLevel::KeepCurrent);
        assert_eq!(rb.recommended_scheme.as_deref(), Some("B"));

        // Активная схема (исходной нет).
        let (a, b) = tie_pair();
        let rb = recommend(
            &[
                a.clone()
                    .mut_set(|c| c.is_active = true)
                    .mut_set(|c| c.is_original = false),
                b,
            ],
            3,
        );
        assert_eq!(rb.level, EvidenceLevel::KeepCurrent);
        assert_eq!(rb.recommended_scheme.as_deref(), Some("A"));
    }

    #[test]
    fn primary_order_prefers_median_then_consistency_then_lower_cv() {
        // Равные средние: решает медиана (B > A).
        let a = scheme(
            "A",
            1000.0,
            900.0,
            800.0,
            700.0,
            90.0,
            2.0,
            &[(1000.0, 200); 3],
            false,
            false,
        );
        let b = scheme(
            "B",
            1000.0,
            990.0,
            800.0,
            700.0,
            90.0,
            2.0,
            &[(1000.0, 200); 3],
            false,
            false,
        );
        let r = recommend(&[a, b], 3);
        assert_eq!(r.recommended_scheme.as_deref(), Some("B"));

        // Равные средние и медиана: решает CV (B меньше/лучше).
        let a = scheme(
            "A",
            1000.0,
            990.0,
            800.0,
            700.0,
            90.0,
            3.0,
            &[(1000.0, 200); 3],
            false,
            false,
        );
        let b = scheme(
            "B",
            1000.0,
            990.0,
            800.0,
            700.0,
            90.0,
            1.0,
            &[(1000.0, 200); 3],
            false,
            false,
        );
        let r = recommend(&[a, b], 3);
        assert_eq!(r.recommended_scheme.as_deref(), Some("B"));
    }

    #[test]
    fn confirmed_level_with_three_runs_per_scheme() {
        let a = scheme(
            "A",
            1000.0,
            1000.0,
            950.0,
            900.0,
            95.0,
            1.0,
            &[(990.0, 200), (1000.0, 200), (1010.0, 200)],
            false,
            false,
        );
        let b = scheme(
            "B",
            960.0,
            960.0,
            910.0,
            860.0,
            95.0,
            1.0,
            &[(950.0, 200), (960.0, 200), (970.0, 200)],
            false,
            false,
        );
        let r = recommend(&[a, b], 3);
        assert_eq!(r.level, EvidenceLevel::Confirmed);
        assert_eq!(r.recommended_scheme.as_deref(), Some("A"));
        let p = r.three_probabilities.unwrap();
        assert!(p[0] >= 0.90, "P(лучший) = {}", p[0]);
        assert!(p[2] >= 0.95, "P(перевес > 1%) = {}", p[2]);
        let margin = r.expected_margin_percent.unwrap();
        assert!((margin - 4.1667).abs() < 0.01, "margin = {margin}");
    }

    #[test]
    fn probable_level_requires_positive_margin_but_not_confirmed() {
        // Перевес ~1.2%, но разброс велик: около 40% реплик падают ниже порога 1%,
        // поэтому P(перевес > 1%) ≈ 0.59 < 95% → Probable, не Confirmed.
        let a = scheme(
            "A",
            1015.0,
            1015.0,
            990.0,
            970.0,
            95.0,
            2.0,
            &[(1000.0, 200), (1010.0, 200), (1035.0, 200)],
            false,
            false,
        );
        let b = scheme(
            "B",
            1003.0,
            1003.0,
            980.0,
            960.0,
            95.0,
            2.0,
            &[(995.0, 200), (1002.0, 200), (1012.0, 200)],
            false,
            false,
        );
        let r = recommend(&[a, b], 3);
        assert_eq!(r.level, EvidenceLevel::Probable);
        let p = r.three_probabilities.unwrap();
        assert!(p[0] >= 0.60, "P(лучший) = {}", p[0]);
        assert!(p[1] >= 0.80, "P(перевес > 0) = {}", p[1]);
        assert!(p[2] < 0.95, "P(перевес > 1%) должен быть < 95%: {}", p[2]);
    }

    #[test]
    fn preliminary_when_fewer_than_three_runs() {
        let a = scheme(
            "A",
            1000.0,
            1000.0,
            900.0,
            800.0,
            90.0,
            1.0,
            &[(1000.0, 200), (1000.0, 200)],
            false,
            false,
        );
        let b = scheme(
            "B",
            920.0,
            920.0,
            830.0,
            750.0,
            90.0,
            1.0,
            &[(920.0, 200), (920.0, 200)],
            false,
            false,
        );
        let r = recommend(&[a, b], 3);
        assert_eq!(r.level, EvidenceLevel::Preliminary);

        // Один кандидат — всегда Preliminary.
        let only = scheme(
            "A",
            1000.0,
            1000.0,
            900.0,
            800.0,
            90.0,
            1.0,
            &[(1000.0, 200); 3],
            false,
            false,
        );
        let r = recommend(&[only], 3);
        assert_eq!(r.level, EvidenceLevel::Preliminary);
    }

    #[test]
    fn recommendation_is_invariant_to_input_order() {
        let a = scheme(
            "A",
            1000.0,
            1000.0,
            950.0,
            900.0,
            95.0,
            1.0,
            &[(990.0, 200), (1000.0, 200), (1010.0, 200)],
            false,
            false,
        );
        let b = scheme(
            "B",
            960.0,
            960.0,
            910.0,
            860.0,
            95.0,
            1.0,
            &[(950.0, 200), (960.0, 200), (970.0, 200)],
            false,
            false,
        );
        let c = scheme(
            "C",
            920.0,
            920.0,
            870.0,
            820.0,
            90.0,
            2.0,
            &[(910.0, 200), (920.0, 200), (930.0, 200)],
            false,
            false,
        );
        let r1 = recommend(&[a.clone(), b.clone(), c.clone()], 3);
        let r2 = recommend(&[c.clone(), a.clone(), b.clone()], 3);
        let r3 = recommend(&[b.clone(), c.clone(), a.clone()], 3);
        assert_eq!(r1, r2);
        assert_eq!(r1, r3);
    }

    /// Мелкий хелпер: мутирующее изменение копии кандидата.
    trait MutSet {
        fn mut_set(self, f: impl FnOnce(&mut Self)) -> Self;
    }
    impl MutSet for SchemeAggregate {
        fn mut_set(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }
}
