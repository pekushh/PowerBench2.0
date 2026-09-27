//! Оркестрация теста PowerBench.
//!
//! План сессии (пресеты, деление длительности, ротация раундов), контрольные
//! точки с атомарной записью и продолжением после прерывания, планировщик
//! сессии (преамбула, фазы, сторожевой таймер, фоновый мониторинг,
//! восстановление исходной схемы) и структуры итогового JSON-результата.

pub mod appsettings;
pub mod checkpoint;
pub mod config;
pub mod diagnostics;
pub mod history;
pub mod leader;
pub mod quarantine;
pub mod recovery;
pub mod report;
pub mod result;
pub mod score;
pub mod session;

#[cfg(test)]
mod goldens;

/// Имя каталога данных приложения (в `%LOCALAPPDATA%`).
pub const DATA_DIR_NAME: &str = "PowerBench";
