//! Журнал событий приложения (этап 7): кольцевой буфер с персистентностью
//! в `AppLog.json`. Используется командой `log_history` и живыми событиями
//! `log` — фронтенд видит один и тот же поток строк.
//!
//! Запись на диск не блокирует вызывающий поток: фоновый писатель
//! переписывает файл не чаще раза в [`FLUSH_INTERVAL_MS`], а на выходе из
//! приложения дописывает остаток. Раньше каждая строка журнала сериализовала
//! и атомарно перезаписывала весь файл (до 5000 записей) — во время сессии это
//! десятки полных записей файла.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use powerbench_orchestrator::history::results_dir;

/// Имя файла журнала (в каталоге данных приложения).
pub const LOG_FILE_NAME: &str = "AppLog.json";

/// Максимальное число хранимых записей (кольцо).
const MAX_ENTRIES: usize = 5000;

/// Как часто фоновый писатель переносит буфер на диск.
const FLUSH_INTERVAL_MS: u64 = 1_000;

/// Сколько ждать `flush`, если писатель завис (диск недоступен и т. п.).
const FLUSH_TIMEOUT_MS: u64 = 5_000;

/// Одна запись журнала.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts_ms: u64,
    pub level: String,
    pub text: String,
}

/// Буфер журнала: записи в памяти плюс очередь на запись.
struct Inner {
    entries: Vec<LogEntry>,
}

/// Счётчик записей, попадавших в буфер. Писатель запоминает значение, на
/// котором закончил, и `flush` ждёт, пока оно догонит счётчик.
type Seq = Arc<AtomicU64>;

/// Состояние, общее для вызывающих потоков и писателя.
struct Shared {
    inner: Mutex<Inner>,
    /// Сколько записей добавлено в буфер.
    appended: Seq,
    /// Сколько записей писатель уже перенёс на диск.
    written: Seq,
    /// Сигнал писателю: появились новые записи или пора завершаться.
    wake: Condvar,
    /// Сигнал ожидающему `flush`: запись на диске завершена.
    flushed: Condvar,
    /// Писатель завершён.
    stop: AtomicBool,
}

/// Потокобезопасный журнал с дисковым резервом и фоновой записью.
pub struct Logger {
    shared: Arc<Shared>,
    tx: Option<SyncSender<()>>,
    writer: Option<JoinHandle<()>>,
}

impl Logger {
    /// Открыть журнал в стандартном каталоге данных.
    pub fn new() -> Self {
        Self::in_file(data_dir().join(LOG_FILE_NAME))
    }

    /// Открыть журнал в указанном файле. Отдельный конструктор нужен тестам:
    /// они не должны трогать настоящий каталог данных.
    fn in_file(path: PathBuf) -> Self {
        let entries = load(&path);
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner { entries }),
            appended: Arc::new(AtomicU64::new(0)),
            written: Arc::new(AtomicU64::new(0)),
            wake: Condvar::new(),
            flushed: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        // Ёмкость 1: важен свежий хвост, а не очередь «всё и сразу».
        // Переполнение ничего не теряет — писатель забирает весь буфер целиком.
        let (tx, rx) = sync_channel::<()>(1);
        let w = Arc::clone(&shared);
        let writer = std::thread::Builder::new()
            .name("powerbench-log-writer".into())
            .spawn(move || writer_loop(&w, &rx, &path))
            .ok();
        Self {
            shared,
            tx: Some(tx),
            writer,
        }
    }

    /// Добавить запись (сейчас), обрезать кольцо и попросить писателя
    /// сохранить буфер.
    ///
    /// Отравление мьютекса игнорируем: потеря блокировки в журнале не должна
    /// превращать каждую последующую запись в панику.
    pub fn append(&self, level: &str, text: &str) {
        {
            let mut g = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            g.entries.push(LogEntry {
                ts_ms: now_ms(),
                level: level.to_string(),
                text: text.to_string(),
            });
            if g.entries.len() > MAX_ENTRIES {
                let drop = g.entries.len() - MAX_ENTRIES;
                g.entries.drain(0..drop);
            }
        }
        self.shared.appended.fetch_add(1, Ordering::Release);
        self.signal();
    }

    /// Сигнал писателю. Канал ёмкости 1, поэтому отправка не блокирует, даже
    /// если писатель занят диском: данные всё равно уже в буфере, а писатель
    /// заберёт их целиком по одному сигналу.
    fn signal(&self) {
        if let Some(tx) = &self.tx {
            match tx.try_send(()) {
                Ok(()) => self.shared.wake.notify_one(),
                // Канал полон — сигнал уже в очереди, второй ничего не изменит.
                Err(TrySendError::Full(_)) => {}
                // Писатель завершён: пишем напрямую, лучше так, чем потерять.
                Err(TrySendError::Disconnected(_)) => {
                    let path = data_dir().join(LOG_FILE_NAME);
                    let g = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = persist(&g.entries, &path);
                }
            }
        }
    }

    /// Текущий журнал (хронологический порядок, самый новый последним).
    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .clone()
    }

    /// Дождаться, пока буфер окажется на диске.
    pub fn flush(&self) {
        let target = self.shared.appended.load(Ordering::Acquire);
        self.signal();
        let mut g = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        while self.shared.written.load(Ordering::Acquire) < target {
            let (next, _) = self
                .shared
                .flushed
                .wait_timeout(g, Duration::from_millis(FLUSH_TIMEOUT_MS))
                .unwrap_or_else(|e| e.into_inner());
            g = next;
        }
    }
}

impl Default for Logger {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Logger {
    /// Финальный сброс: без него последние записи остались бы только в памяти.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake.notify_one();
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
    }
}

/// Фоновый писатель: просыпается по сигналу или по таймауту, переносит весь
/// буфер на диск и отмечает, сколько записей уже сохранено.
fn writer_loop(shared: &Arc<Shared>, rx: &Receiver<()>, path: &Path) {
    loop {
        // Ждём сигнала, но не дольше интервала: иначе последняя строка перед
        // выходом ждала бы следующего события.
        {
            let g = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            let _ = shared
                .wake
                .wait_timeout(g, Duration::from_millis(FLUSH_INTERVAL_MS))
                .unwrap_or_else(|e| e.into_inner());
        }
        // Дренируем канал: один сигнал покрывает всё, что накопилось.
        while rx.try_recv().is_ok() {}

        let saved = {
            let g = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            if persist(&g.entries, path).is_ok() {
                shared.appended.load(Ordering::Acquire)
            } else {
                // Не сохранилось — не отмечаем, иначе данные считались бы
                // записанными. Следующий круг попробует снова.
                shared.written.load(Ordering::Acquire)
            }
        };
        shared.written.store(saved, Ordering::Release);
        shared.flushed.notify_all();

        if shared.stop.load(Ordering::Acquire) {
            // На выходе пишем ещё раз: между последним `append` и `stop`
            // могла прийти ещё строка.
            let target = shared.appended.load(Ordering::Acquire);
            if shared.written.load(Ordering::Acquire) >= target {
                break;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// Уникальный каталог на тест, чтобы параллельные тесты не делили файл.
    fn temp_path(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "powerbench-log-{}-{}-{}",
            tag,
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(LOG_FILE_NAME)
    }

    fn read_log(path: &Path) -> Vec<LogEntry> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Регресс: запись не должна ждать диск. Раньше `append` синхронно
    /// сериализовал и атомарно перезаписывал весь файл, поэтому сотня строк
    /// означала сотню полных записей по 5000 записей.
    #[test]
    fn append_does_not_persist_synchronously() {
        let path = temp_path("perf");
        let log = Logger::in_file(path.clone());
        let t0 = std::time::Instant::now();
        for i in 0..500 {
            log.append("info", &format!("строка {i}"));
        }
        let per_append = t0.elapsed();
        // Если бы каждая запись переписывала файл, 500 строк заняли бы
        // заметное время; здесь проверяем именно порядок величины.
        assert!(
            per_append < std::time::Duration::from_millis(2_000),
            "500 записей заняли {per_append:?} — похоже на синхронную запись"
        );
        assert_eq!(log.snapshot().len(), 500);
        log.flush();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// `flush` обязан гарантировать, что данные на диске.
    #[test]
    fn flush_persists_everything() {
        let path = temp_path("flush");
        let log = Logger::in_file(path.clone());
        for i in 0..50 {
            log.append("warn", &format!("строка {i}"));
        }
        log.flush();
        let on_disk = read_log(&path);
        assert_eq!(on_disk.len(), 50, "файл не догнал буфер после flush");
        assert_eq!(on_disk[49].text, "строка 49");
        assert_eq!(on_disk[0].level, "warn");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Drop тоже обязан дописать: иначе последние строки теста потерялись бы.
    #[test]
    fn drop_flushes_remaining() {
        let path = temp_path("drop");
        {
            let log = Logger::in_file(path.clone());
            for i in 0..10 {
                log.append("info", &format!("строка {i}"));
            }
            // Без явного flush — полагаемся только на Drop.
        }
        assert_eq!(read_log(&path).len(), 10, "Drop не дописал буфер");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Кольцо: больше MAX_ENTRIES строк не накапливаем.
    #[test]
    fn ring_drops_oldest() {
        let path = temp_path("ring");
        let log = Logger::in_file(path.clone());
        for i in 0..(MAX_ENTRIES + 100) {
            log.append("info", &format!("строка {i}"));
        }
        let snap = log.snapshot();
        assert_eq!(snap.len(), MAX_ENTRIES);
        assert_eq!(snap[0].text, "строка 100", "осталось не самое старое");
        assert_eq!(
            snap[MAX_ENTRIES - 1].text,
            format!("строка {}", MAX_ENTRIES + 99)
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Уже существующий файл подхватывается при старте.
    #[test]
    fn loads_existing_log() {
        let path = temp_path("load");
        {
            let first = Logger::in_file(path.clone());
            first.append("info", "из прошлого запуска");
            first.flush();
        }
        let second = Logger::in_file(path.clone());
        assert_eq!(second.snapshot().len(), 1);
        assert_eq!(second.snapshot()[0].text, "из прошлого запуска");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
