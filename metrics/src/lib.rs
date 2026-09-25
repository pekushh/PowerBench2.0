//! PowerBench — метрики и статистика.
//!
//! Чистые функции раздела «Метрики и статистика» спецификации:
//! перцентиль (линейная интерполяция), статистика прогона, стабильность по
//! фазам, агрегация прогонов одной схемы с проверкой совместимости сигнатур
//! и подписей детерминизма.

pub mod aggregate;
pub mod percentile;
pub mod run;

pub use aggregate::{
    AggregateError, AggregateResult, CompatibilitySignature, DeterminismSignature, RunSummary,
    aggregate_runs,
};
pub use percentile::{percentile, percentile_sorted};
pub use run::{
    RunStats, burst_retention_percent, consistency_percent, filter_valid_times, jitter_p99_ms,
    median, population_std, run_stats,
};
