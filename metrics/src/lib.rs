//! PowerBench — метрики и статистика.
//!
//! Чистые функции раздела «Метрики и статистика» спецификации:
//! перцентиль (линейная интерполяция), статистика прогона, стабильность по
//! фазам, агрегация прогонов одной схемы с проверкой совместимости сигнатур
//! и подписей детерминизма.

pub mod aggregate;
pub mod percentile;
pub mod rank;
pub mod run;

pub use aggregate::{
    AggregateError, AggregateResult, CompatibilitySignature, DeterminismSignature, RunSummary,
    aggregate_runs, t95,
};
pub use percentile::{percentile, percentile_sorted};
pub use rank::{RankKey, has_data, rank_cmp, sort_by_rank, tiebreak_id};
pub use run::{
    MIN_TRIM_SAMPLES, P01_WINDOW_MS, RunStats, TRIM_FRACTION, burst_retention_percent,
    consistency_percent, filter_valid_times, jitter_p99_ms, median, p01_throughput, population_std,
    run_stats, trim_outliers, windowed_throughput, worst_second_throughput,
};
