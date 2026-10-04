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

use crate::checkpoint::{PhaseStats, PowerSnapshot, StoredRun};
use crate::leader::robust_leader;
use crate::result::{
    PhaseSummaryJson, SchemeJson, phase_summaries_for_test, power_warnings_for_test,
};
use crate::score::{ScoreWeights, score_leader, score_schemes};
use powerbench_metrics::run::run_stats;

// ---------------------------------------------------------------------------
// Питание, выбросы и длительность фаз доходят до машинных выводов
// ---------------------------------------------------------------------------

fn stored_run() -> StoredRun {
    let stats = run_stats(&[10.0; 200]).expect("эталон");
    StoredRun {
        key: "0:g".to_string(),
        round: 0,
        scheme_id: "g".into(),
        scheme_name: None,
        started_at_ns: 0,
        duration_ms: 10_000,
        ticks: 200,
        supercycles: 0,
        first_tick_checksums: [1; crate::config::PHASES_PER_RUN as usize],
        run_checksums: [1; crate::config::PHASES_PER_RUN as usize],
        phases: Vec::new(),
        combined: stats,
        cross_phase_consistency: 0.0,
        burst_retention_percent: 0.0,
        background: Vec::new(),
        spike_windows: 0,
        power: None,
        scheme_dump: None,
        background_cpu_p50: 0.0,
        background_cpu_p95: 0.0,
        background_sample_seconds: 0,
        worker_count: 4,
        affinity_mode: "p-only".to_string(),
        affinity_signature: "p-only:test".to_string(),
        config_hash: "cfg-test".to_string(),
    }
}

fn throttling_snapshot() -> PowerSnapshot {
    PowerSnapshot {
        max_mhz: 3200,
        current_mhz: 3200,
        throttled: true,
        thermal_throttle: true,
        policy_reason: 0,
        max_idle_minutes: 0,
        on_ac: true,
        unavailable: false,
    }
}

fn run_with_phase_power(power: Option<PowerSnapshot>) -> StoredRun {
    let phase_power = power;
    StoredRun {
        phases: vec![PhaseStats {
            phase_index: 0,
            stats: run_stats(&[10.0; 200]).expect("эталон"),
            power: phase_power,
            seconds: 10,
        }],
        power,
        ..stored_run()
    }
}

/// Снимок питания фазы обязан доехать до `SessionJson.warnings`.
///
/// Раньше `PowerSnapshot::note()` жил только в HTML, и троттлинг на второй
/// фазе второго прогона не оставлял в JSON ничего: машинно прочитанный отчёт
/// показывал исправную машину.
#[test]
fn power_note_reaches_machine_readable_warnings() {
    let warnings = power_warnings_for_test(&[run_with_phase_power(Some(throttling_snapshot()))]);
    assert!(
        warnings.iter().any(|w| w.contains("тепловая")),
        "замечание о тепловой защите не попало в предупреждения: {warnings:?}"
    );
}

/// Один и тот же дефект не должен засорять отчёт десятью одинаковыми строками.
#[test]
fn identical_power_notes_are_collapsed() {
    let runs: Vec<StoredRun> = (0..10)
        .map(|_| run_with_phase_power(Some(throttling_snapshot())))
        .collect();
    let warnings = power_warnings_for_test(&runs);
    assert_eq!(
        warnings.len(),
        1,
        "десять одинаковых замечаний должны схлопнуться в одно: {warnings:?}"
    );
}

/// Дамп настроек схемы обязан пережить сериализацию: без него результат
/// нельзя воспроизвести — Windows и OEM-агенты молча правят планы.
#[test]
fn scheme_dump_survives_serialization() {
    let dump = "  Имя схемы\n    Параметры указателя\n      Индекс         : 50\n";
    let run = StoredRun {
        scheme_dump: Some(dump.to_string()),
        ..stored_run()
    };
    let json = serde_json::to_string(&run).expect("сериализация");
    let back: StoredRun = serde_json::from_str(&json).expect("десериализация");
    assert_eq!(
        back.scheme_dump.as_deref(),
        Some(dump),
        "дамп настроек схемы не пережил сериализацию"
    );
}

/// Длительность фазы и счётчики сэмплов обязаны попасть в сводку фаз.
///
/// Без них p1 и p0.001 несравнимы: 3 000 тиков за 10 с и 3 000 тиков за 60 с —
/// разная точность измерения, а в таблице выглядели одинаково.
#[test]
fn phase_summary_reports_duration_and_sample_counts() {
    let times: Vec<f64> = (0..1000).map(|i| 10.0 + (i % 7) as f64 * 0.1).collect();
    let stats = run_stats(&times).expect("эталон");
    let run = StoredRun {
        phases: vec![PhaseStats {
            phase_index: 0,
            stats,
            power: None,
            seconds: 37,
        }],
        ..stored_run()
    };
    let summaries = phase_summaries_for_test(&[run], 0.0);
    let phase = &summaries[0];
    assert_eq!(phase.seconds, 37, "длительность фазы потерялась");
    assert_eq!(
        phase.samples_used, stats.samples,
        "число использованных сэмплов не совпало с расчётом"
    );
    assert_eq!(phase.samples_raw, stats.samples_raw);
    assert!(
        (phase.excluded_fraction - stats.excluded_fraction).abs() < 1e-12,
        "доля отброшенных выбросов не совпала: {} против {}",
        phase.excluded_fraction,
        stats.excluded_fraction
    );
}

// ---------------------------------------------------------------------------
// Согласованность выбора победителя между модулями
// ---------------------------------------------------------------------------

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
        seconds: 0,
        samples_used: 0,
        samples_raw: 0,
        excluded_fraction: 0.0,
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
