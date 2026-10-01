//! Согласованность выбора победителя между модулями.
//!
//! Отдельный модуль не из-за логики, а из-за инварианта: `recommend`,
//! `leader` и `score` трижды вычисляли «кто лучший» по разным правилам, и
//! один отчёт мог назвать победителем разные схемы. Проверять это надо было
//! на данных, где правила расходятся, — то есть где среднее и медиана указывают
//! на разные схемы. На «спокойных» данных расхождения не видно, и тест
//! проходит при сломанной логике.
//!
//! Здесь собраны данные одного и того же набора в трёх представлениях, которые
//! потребляют модули, и проверяется, что все называют одного победителя.

use crate::leader::robust_leader;
use crate::result::{PhaseSummaryJson, SchemeJson};
use crate::score::{ScoreWeights, score_leader, score_schemes};

/// Схема отчёта с заданными метриками. Фаза не влияет на выбор победителя, но
/// нужна для заполнения структуры целиком.
fn scheme(
    id: &str,
    median_throughput: f64,
    mean_average_throughput: f64,
    p1: f64,
    consistency: f64,
    cv: f64,
    rejected: bool,
) -> SchemeJson {
    let phase = PhaseSummaryJson {
        name: "Тяжёлая".into(),
        median_throughput,
        p1_throughput: p1,
        consistency_percent: consistency,
        frequency_drop_percent: 0.0,
        frequency_mhz: 5000.0,
    };
    SchemeJson {
        scheme_id: id.to_string(),
        name: Some(id.to_string()),
        rejected,
        rejection_reason: None,
        runs: 5,
        mean_average_throughput,
        sample_std: 0.0,
        t_value: 0.0,
        margin: 0.0,
        ci_95: [0.0, 0.0],
        run_variation_percent: cv,
        cv_warning: false,
        median_throughput,
        median_p1_throughput: p1,
        median_p01_throughput: 0.0,
        median_p95_execution_time_ms: 0.0,
        median_p99_execution_time_ms: 0.0,
        median_consistency_percent: consistency,
        median_burst_retention_percent: 0.0,
        median_jitter_p99_ms: 0.0,
        median_worst_window_throughput: p1,
        median_background_purity: None,
        phases: vec![phase],
        run_duration_ms: 300_000,
        started_at_min_ns: 0,
        per_run: Vec::new(),
    }
}

/// Ключевой случай расхождения: у «шумной» среднее выше из-за выброса, а
/// медиана ниже. Раньше `recommend` и `score` называли победителем «шумную»,
/// а `leader` — «устойчивую», и в одном отчёте стояли оба вывода.
#[test]
fn all_three_modules_agree_when_mean_and_median_disagree() {
    let noisy = scheme("noisy", 400.0, 900.0, 400.0, 80.0, 1.0, false);
    let solid = scheme("solid", 500.0, 505.0, 500.0, 80.0, 1.0, false);

    let by_leader = robust_leader(&[noisy.clone(), solid.clone()])
        .expect("должен быть лидер")
        .scheme_id;
    let by_score = score_leader(&score_schemes(
        &[noisy.clone(), solid.clone()],
        &ScoreWeights::default(),
    ))
    .expect("должен быть балл")
    .scheme_id
    .clone();

    assert_eq!(
        by_leader, "solid",
        "лидер обязан определяться по медиане, а не по среднему с выбросом"
    );
    assert_eq!(
        by_score, by_leader,
        "балл и лидер обязаны указывать на одну схему: балл выбрал {by_score}, \
         лидер — {by_leader}"
    );
}

/// Схема с испорченными числами не должна выигрывать ни в одном модуле.
#[test]
fn broken_numbers_do_not_become_leader_anywhere() {
    let broken = scheme("broken", f64::NAN, f64::NAN, f64::NAN, 100.0, 0.0, false);
    let good = scheme("good", 500.0, 500.0, 500.0, 90.0, 1.0, false);

    assert_eq!(
        robust_leader(&[broken.clone(), good.clone()])
            .expect("лидер есть")
            .scheme_id,
        "good"
    );
    assert_eq!(
        score_leader(&score_schemes(
            &[broken.clone(), good.clone()],
            &ScoreWeights::default()
        ))
        .expect("балл есть")
        .scheme_id,
        "good"
    );
}

/// Забракованная схема не лидер нигде, даже с лучшими числами.
#[test]
fn rejected_scheme_is_excluded_consistently() {
    let rejected = scheme("rejected", 9999.0, 9999.0, 9999.0, 100.0, 0.1, true);
    let normal = scheme("normal", 500.0, 500.0, 500.0, 90.0, 1.0, false);

    assert_eq!(
        robust_leader(&[rejected.clone(), normal.clone()])
            .expect("лидер есть")
            .scheme_id,
        "normal"
    );
    assert_eq!(
        score_leader(&score_schemes(
            &[rejected, normal],
            &ScoreWeights::default()
        ))
        .expect("балл есть")
        .scheme_id,
        "normal"
    );
}

/// При равной статистике выбор обязан быть детерминирован, иначе отчёт
/// менялся бы от запуска к запуску на одних и тех же данных.
#[test]
fn identical_schemes_give_identical_verdicts_in_stable_order() {
    let a = scheme("aaa", 500.0, 500.0, 500.0, 90.0, 1.0, false);
    let b = scheme("bbb", 500.0, 500.0, 500.0, 90.0, 1.0, false);

    for _ in 0..8 {
        let leader = robust_leader(&[a.clone(), b.clone()])
            .expect("лидер есть")
            .scheme_id;
        let scored = score_leader(&score_schemes(
            &[a.clone(), b.clone()],
            &ScoreWeights::default(),
        ))
        .expect("балл есть")
        .scheme_id
        .clone();
        assert_eq!(
            leader, scored,
            "порядок схем изменился от запуска к запуску"
        );
    }
}
