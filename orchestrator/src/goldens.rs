//! Golden-тесты отчётов и фаззинг десериализаторов (только для тестов).
//!
//! Золотые снимки проверяют устойчивость HTML-отчётов: одинаковый ввод
//! даёт побайтово одинаковый вывод, а чувствительные поля (перевес, имена)
//! заметно меняют документ. Фаззинг гоняет случайные искажённые JSON через
//! десериализаторы `SessionJson`/`Checkpoint`/`AppSettings` и гарантирует
//! отсутствие паник на неструктурированных входных данных.

use crate::appsettings::AppSettings;
use crate::checkpoint::Checkpoint;
use crate::report::{build_report, build_session_report};
use crate::result::{IdentityJson, RecommendationJson, SchemeJson, SessionJson};

/// Фиксированный снимок сессии (все поля заданы явно: никакой неопределённости).
fn snapshot_session(id: &str, median: f64, margin: f64) -> SessionJson {
    let mut sch = SchemeJson::from_aggregate(
        id.to_string(),
        false,
        None,
        &empty_aggregate(median, margin),
        Vec::new(),
    );
    sch.name = Some(format!("План {id}"));
    let mut second = SchemeJson::from_aggregate(
        "runner".to_string(),
        false,
        None,
        &empty_aggregate(median * 0.94, margin * 1.1),
        Vec::new(),
    );
    second.name = Some("План runner".to_string());
    SessionJson {
        plan_guid: format!("golden-{id}"),
        original_scheme_guid: None,
        original_restored: true,
        identity: IdentityJson {
            workload_version: "GamingCpuV1".into(),
            config_hash: "H1".into(),
            seed_hex: "SEED0080".into(),
            worker_count: 8,
            logical_cpus: 16,
            timer_hz: 10_000_000,
            cpu_identifier: "Golden CPU".into(),
            diagnostics_version: "1.0.0".into(),
            os_build: String::new(),
            memory_gib: 0.0,
            cpu_brand: String::new(),
            affinity_mode: "p-only".into(),
            affinity_signature: "p-only:test".into(),
        },
        schemes: vec![sch, second],
        recommendation: RecommendationJson {
            level: "Confirmed".into(),
            level_label: "Подтверждено".into(),
            recommended_scheme: Some(id.into()),
            runner_up_scheme: Some("runner".into()),
            reason: "лидер значимо быстрее в фазе «Отклик»".into(),
            probabilities: Some([0.97, 0.92, 0.85]),
            expected_margin_percent: Some(margin),
            bootstrap_mode: Some("PairedByRun".into()),
            tie_criterion: None,
        },
        warnings: Vec::new(),
        rounds_planned: 5,
        rounds_completed: 5,
        early_stop_reason: None,
        score_weights: [50.0, 30.0, 20.0],
        reference: None,
        screening: false,
    }
}

fn empty_aggregate(median: f64, margin: f64) -> powerbench_metrics::AggregateResult {
    powerbench_metrics::AggregateResult {
        runs: 5,
        mean_average_throughput: median,
        sample_std: 0.1,
        t_value: 12.0,
        margin,
        ci_95: [median - margin, median + margin],
        run_variation_percent: 1.0,
        cv_warning: false,
        median_throughput: median,
        median_p1_throughput: median * 0.98,
        median_p01_throughput: median * 0.97,
        median_p95_execution_time_ms: 1.0,
        median_p99_execution_time_ms: 1.2,
        median_consistency_percent: 97.0,
        median_burst_retention_percent: 90.0,
        median_jitter_p99_ms: 0.05,
        median_worst_window_throughput: median * 0.9,
        median_background_purity: Some(99.2),
        run_duration_ms: 54000,
        started_at_min_ns: 1_700_000_000_000_000_000,
    }
}

#[test]
fn session_report_is_deterministic() {
    // Штамп генерации берётся из реального времени; всё остальное обязано
    // быть побайтово одинаковым между двумя независимыми построениями.
    let strip = |html: &str| html.split("Сгенерировано").next().unwrap().to_string();
    let s = snapshot_session("AAA", 500.0, 12.5);
    let first = build_session_report(&s);
    let again = build_session_report(&s);
    assert_eq!(
        strip(&first),
        strip(&again),
        "повторный прогон обязан дать побайтово тот же HTML"
    );
    let html = first.as_str();
    // Золотые маркеры компактного формата: вердикт, рекомендованная схема,
    // её медиана, перевес в вердикте и имя плана в таблице.
    assert!(html.contains("ВЕРДИКТ БЕНЧМАРКА"));
    assert!(html.contains("Подтверждено"));
    assert!(html.contains("Рекомендуем: «План AAA»"));
    assert!(html.contains("РЕКОМЕНДУЕТСЯ"));
    assert!(html.contains("500.0"));
    assert!(html.contains("План AAA"));
    assert!(html.contains("План runner"));
    assert!(html.ends_with("</body></html>"));
}

/// Перевес обязан попадать в документ: в компактном отчёте он попадает в
/// дельту лидера относительно опорной схемы.
#[test]
fn session_report_is_sensitive_to_input() {
    let base = build_session_report(&snapshot_session("BBB", 500.0, 12.5));
    let louder = build_session_report(&snapshot_session("BBB", 500.0, 42.0));
    assert_ne!(base, louder, "перевес обязан попадать в документ");
}

#[test]
fn history_report_is_deterministic_modulo_timestamp() {
    let strip = |html: &str| html.split("Сгенерировано").next().unwrap().to_string();
    let a = snapshot_session("CCC", 720.0, 8.0);
    let b = snapshot_session("DDD", 690.0, 6.0);
    let one = build_report(&[a.clone(), b.clone()]);
    let two = build_report(&[a.clone(), b.clone()]);
    assert_eq!(strip(&one), strip(&two));
    assert!(one.contains("Сессии (2)"));
    assert!(one.contains("План CCC"));
    assert!(one.contains("<svg"));
}

/// Корпус сознательно сломанных или граничных JSON (литералы, усечения,
/// крайние числа, дубли ключей, неверные типы, вложенность, Unicode).
fn edge_corpus() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut fragments: Vec<String> = vec![
        "".into(),
        " ".into(),
        "{".into(),
        "}".into(),
        "[]]{}".into(),
        "null".into(),
        "true".into(),
        "false".into(),
        "42".into(),
        r#"{"round":0}"#.into(),
        r#"{"round":1e999}"#.into(),
        r#"{"round":-5}"#.into(),
        r#"{"round":"7"}"#.into(),
        r#"{"schemes":[{"scheme_id":"a"},"x",7]}"#.into(),
        r#"{"scheme_ids":["a","b"]}"#.into(),
        r#"{"plan":{"plan_guid":"g","repetitions":2}}"#.into(),
    ];
    fragments.push(r#"{"plan_guid":"g","plan_guid":"h"}"#.to_string());
    fragments.push(format!("{{\"runs\":[{}}}}}", "x".repeat(2000)));
    fragments.push(format!(r#"{{"pad":"{}"}}"#, "a".repeat(50_000)));
    fragments.push("{\"a\":\"\\u0000\"}".into());
    fragments.push("{\"a\":\"\\uD800\"}".into());
    fragments
        .push("[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]".into());
    fragments.push("{\"identity\":{\"worker_count\":-1,\"logical_cpus\":0}}".into());
    for f in fragments {
        out.push(f.to_string());
        // Усечённые версии (в середине и в конце).
        let bytes = f.as_bytes();
        let mid = bytes.len() / 2;
        out.push(String::from_utf8_lossy(&bytes[..mid]).into_owned());
        if !bytes.is_empty() {
            out.push(String::from_utf8_lossy(&bytes[..bytes.len() - 1]).into_owned());
        }
    }
    out
}

/// Простой битовый RNG (xorshift64*).
struct XorRng(u64);
impl XorRng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

/// Псевдослучайная строка на основе равномерно искажённых токенов.
fn mutate(seed: &mut XorRng) -> String {
    let tokens = [
        "{", "}", "[", "]", ",", ":", "\"", "n", "u", "l", "t", "r", "e", "0", "1", "2", ".", "-",
        "+", "e", " ", "\n", "\\", "a", "b", "c", "d", "plan", "guid", "_",
    ];
    let len = (seed.next_u64() % 220) as usize;
    let mut text = String::with_capacity(len + 1);
    for _ in 0..len {
        let idx = (seed.next_u64() as usize) % tokens.len();
        text.push_str(tokens[idx]);
    }
    text
}

/// Все три десериализатора обязаны не паниковать на любом вводе.
fn fuzz_one(text: &str) {
    let _ = serde_json::from_str::<SessionJson>(text);
    let _ = serde_json::from_str::<Checkpoint>(text);
    let _ = serde_json::from_str::<AppSettings>(text);
}

#[test]
fn fuzz_roundtrip_and_deserializers_never_panic() {
    let mut rng = XorRng(0x9E3779B97F4A7C15);
    for src in edge_corpus() {
        fuzz_one(&src);
    }
    for _ in 0..400 {
        let text = mutate(&mut rng);
        fuzz_one(&text);
    }
}

#[test]
fn fuzz_respects_serialize_roundtrip() {
    // Практическое свойство формата: (1) один и тот же текст всегда
    // десериализуется в одни и те же биты; (2) чтение собственной записи
    // численно близко к исходным значениям (сердечный проход f64 сохраняет
    // точность гораздо лучше 1е-12 относительной).
    let s = snapshot_session("AAA", 500.0, 12.5);
    let text = serde_json::to_string(&s).unwrap();
    let a: SessionJson = serde_json::from_str(&text).unwrap();
    let b: SessionJson = serde_json::from_str(&text).unwrap();
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
        "парсинг одного текста обязан быть детерминированным"
    );
    let max_rel = scheme_floats(&a)
        .into_iter()
        .zip(scheme_floats(&s))
        .map(|(x, y)| (x - y).abs() / y.abs().max(1e-9))
        .fold(0.0, f64::max);
    assert!(
        max_rel < 1e-12,
        "чтение собственной записи без потери точности: {max_rel}"
    );
}

fn scheme_floats(s: &SessionJson) -> Vec<f64> {
    let mut out = Vec::new();
    for sch in &s.schemes {
        out.extend_from_slice(&[
            sch.mean_average_throughput,
            sch.sample_std,
            sch.t_value,
            sch.margin,
        ]);
        out.extend_from_slice(&sch.ci_95);
        out.extend_from_slice(&[
            sch.run_variation_percent,
            sch.median_throughput,
            sch.median_p1_throughput,
            sch.median_p01_throughput,
            sch.median_p95_execution_time_ms,
            sch.median_p99_execution_time_ms,
            sch.median_consistency_percent,
            sch.median_burst_retention_percent,
            sch.median_jitter_p99_ms,
            sch.median_worst_window_throughput,
        ]);
        if let Some(p) = sch.median_background_purity {
            out.push(p);
        }
    }
    out
}
