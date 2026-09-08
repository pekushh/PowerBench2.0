//! PowerBench — метрики и статистика.
//!
//! Чистые функции раздела «Метрики и статистика» спецификации:
//! перцентиль (линейная интерполяция), статистика прогона, стабильность по
//! фазам, агрегация прогонов одной схемы с проверкой совместимости сигнатур
//! и подписей детерминизма.

pub mod aggregate;
pub mod percentile;
pub mod run;

pub use aggregate::{aggregate_runs, AggregateError, AggregateResult, CompatibilitySignature, DeterminismSignature, RunSummary};
pub use percentile::{percentile, percentile_sorted};
pub use run::{burst_retention_percent, consistency_percent, filter_valid_times, jitter_p99_ms, median, population_std, run_stats, RunStats};