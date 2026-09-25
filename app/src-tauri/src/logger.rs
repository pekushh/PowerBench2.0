//! Журнал событий приложения (этап 7): кольцевой буфер с персистентностью
//! в `AppLog.json`. Используется командой `log_history` и живыми событиями
//! `log` — фронтенд видит один и тот же поток строк.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use powerbench_orchestrator::history::results_dir;

/// Имя файла журнала (в каталоге данных приложения).
pub const LOG_FILE_NAME: &str = "AppLog.json";

/// Максимальное число хранимых записей (кольцо).
const MAX_ENTRIES: usize = 5000;

/// Одна запись журнала.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts_ms: u64,
    pub level: String,
    pub text: String,
}

struct Inner {
    entries: Vec<LogEntry>,
    path: PathBuf,
}

/// Потокобезопасный журнал с дисковым резервом.
pub struct Logger {
    inner: Mutex<Inner>,
}

impl Logger {
    /// Открыть журнал в стандартном каталоге данных.
    pub fn new() -> Self {
        let path = data_dir().join(LOG_FILE_NAME);
        let entries = load(&path);
        Self {
            inner: Mutex::new(Inner { entries, path }),
        }
    }

    /// Добавить запись (сейчас), обрезать кольцо и сохранить на диск.
    pub fn append(&self, level: &str, text: &str) {
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        g.entries.push(LogEntry {
            ts_ms: now_ms(),
            level: level.to_string(),
            text: text.to_string(),
        });
        if g.entries.len() > MAX_ENTRIES {
            let drop = g.entries.len() - MAX_ENTRIES;
            g.entries.drain(0..drop);
        }
        let _ = persist(&g.entries, &g.path);
    }

    /// Текущий журнал (хронологический порядок, самый новый последним).
    pub fn snapshot(&self) -> Vec<LogEntry> {
        match self.inner.lock() {
            Ok(g) => g.entries.clone(),
            Err(_) => Vec::new(),
        }
    }
}

impl Default for Logger {
    fn default() -> Self {
        Self::new()
    }
}

fn data_dir() -> PathBuf {
    results_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn load(path: &Path) -> Vec<LogEntry> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn persist(entries: &[LogEntry], path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(entries)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    powerbench_orchestrator::checkpoint::atomic_write(path, &bytes)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
