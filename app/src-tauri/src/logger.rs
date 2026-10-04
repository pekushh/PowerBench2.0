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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use powerbench_orchestrator::history::results_dir;

/// Имя файла журнала (в каталоге данных приложения).
pub const LOG_FILE_NAME: &str = "AppLog.json";

/// Максимальное число хранимых записей (кольцо).
const MAX_ENTRIES: usize = 5000;

/// Как часто фоновый писатель переносит буфер на диск.
const FLUSH_INTERVAL_MS: u64 = 1_000;

/// Шаг ожидания в `flush`: как часто проверять, что запись дошла.
const FLUSH_POLL_MS: u64 = 250;

/// Суммарный потолок ожидания в `flush`.
///
/// `flush` зовут при выходе из приложения и перед сбором отчёта для поддержки,
/// то есть ровно там, где зависание недопустимо. Раньше цикл ожидания был
/// бесконечным: пока `persist()` падал (нет прав на `%LOCALAPPDATA%`, файл
/// держит антивирус), `flush()` не возвращался и приложение не закрывалось.
/// Потерять хвост журнала на диске лучше, чем не закрыться вовсе: данные
/// остаются в памяти и попадают в отчёт.
const FLUSH_BUDGET_MS: u64 = 5_000;

/// Сколько неудачных записей подряд терпит писатель, прежде чем остановиться.
///
/// Журнал — вспомогательная задача, и отказ диска не лечится ожиданием.
/// Счётчик ограничивает цикл: без него писатель крутился бы вечно, `Drop`
/// не проходил бы через `join()`, и приложение зависало бы на закрытии.
/// Данные при этом не теряются молча — они остаются в памяти (`snapshot`).
const WRITE_GIVE_UP_AFTER: u32 = 3;

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
    /// Писатель окончательно отказался писать на диск. Смысла ждать
    /// подтверждения больше нет: `flush` возвращается сразу, и вызывающий
    /// поток не тратит время на синхронную запись, которая тоже не пройдёт.
    give_up: AtomicBool,
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
            give_up: AtomicBool::new(false),
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
                Err(TrySendError::Disconnected(_)) => {
                    // Писатель уже отказался писать: повторять запись на
                    // потоке вызывающего бессмысленно (она тоже не пройдёт),
                    // а вот заблокировать `append` может. Данные остаются в
                    // буфере и уходят в отчёт для поддержки.
                    if self.shared.give_up.load(Ordering::Acquire) {
                        return;
                    }
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

    /// Записи лежат на диске? `false` — значит писатель не смог и сдался.
    ///
    /// `flush` ничего не возвращает, и это молчание было половиной проблемы:
    /// вызывающий (выход из приложения, отчёт для поддержки) ждал вечно и не
    /// знал, что данные уже не появятся на диске.
    pub fn is_persisted(&self) -> bool {
        !self.shared.give_up.load(Ordering::Acquire)
    }

    /// Видно ли пользователю, что журнал не записан на диск.
    ///
    /// Запись молча терять нельзя: журнал — это то, что пользователь приложит
    /// к обращению в поддержку. `true` — потери были и об этом сказано.
    pub fn warn_if_not_persisted(&self, level: &str) {
        if !self.is_persisted() {
            self.append(
                level,
                "журнал не сохранён на диск: запись в файл журнала отказывает. \
                 Диагностику всё равно можно сохранить — она берётся из памяти",
            );
        }
    }

    /// Дождаться, пока буфер окажется на диске.
    ///
    /// Ждёт не больше [`FLUSH_BUDGET_MS`] и выходит сразу, если писатель
    /// отказался писать. Прежде цикл был бесконечным, и при отказе диска
    /// (`persist()` падает стабильно) приложение зависало на закрытии и на
    /// запросе диагностики.
    pub fn flush(&self) {
        let target = self.shared.appended.load(Ordering::Acquire);
        self.signal();
        if self.shared.give_up.load(Ordering::Acquire) {
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(FLUSH_BUDGET_MS);
        let mut g = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
        while self.shared.written.load(Ordering::Acquire) < target {
            if self.shared.give_up.load(Ordering::Acquire) || Instant::now() >= deadline {
                return;
            }
            let (next, _) = self
                .shared
                .flushed
                .wait_timeout(g, Duration::from_millis(FLUSH_POLL_MS))
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
    ///
    /// `Drop` не имеет права блокировать: его ждут при выходе из приложения.
    /// Поэтому сигнал отправляется через `try_send` — при полном канале
    /// ничего терять не надо, писатель и так проснётся по `stop`, а при
    /// завершившемся писателе отправка вернёт ошибку мгновенно.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake.notify_one();
        if let Some(tx) = self.tx.take() {
            let _ = tx.try_send(());
        }
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
    }
}

/// Фоновый писатель: просыпается по сигналу или по таймауту, переносит весь
/// буфер на диск и отмечает, сколько записей уже сохранено.
///
/// Цикл ограничен по двум независимым причинам, иначе отказ диска вешал бы
/// приложение навсегда:
/// * `WRITE_GIVE_UP_AFTER` неудачных записей подряд — писатель сдаётся и
///   выходит, `Drop::join()` проходит;
/// * на выходе (`stop`) дописывается последняя порция, но не бесконечно:
///   пока флаг не снят, попытки исчерпаются тем же счётчиком.
fn writer_loop(shared: &Arc<Shared>, rx: &Receiver<()>, path: &Path) {
    let mut fails: u32 = 0;
    loop {
        // Ждём сигнала, но не дольше интервала: иначе последняя строка перед
        // выходом ждала бы следующего события. После `stop` ждать нечего —
        // буфер надо дописать и завершиться.
        if !shared.stop.load(Ordering::Acquire) {
            let g = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            let _ = shared
                .wake
                .wait_timeout(g, Duration::from_millis(FLUSH_INTERVAL_MS))
                .unwrap_or_else(|e| e.into_inner());
        }
        // Дренируем канал: один сигнал покрывает всё, что накопилось.
        while rx.try_recv().is_ok() {}

        let mut saved_ok = true;
        let saved = {
            let g = shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            match persist(&g.entries, path) {
                Ok(()) => {
                    fails = 0;
                    shared.appended.load(Ordering::Acquire)
                }
                Err(_) => {
                    saved_ok = false;
                    // Не сохранилось — не отмечаем, иначе данные считались бы
                    // записанными. Следующий круг попробует снова.
                    shared.written.load(Ordering::Acquire)
                }
            }
        };
        shared.written.store(saved, Ordering::Release);
        shared.flushed.notify_all();

        if !saved_ok {
            fails += 1;
            if fails >= WRITE_GIVE_UP_AFTER {
                // Отказ диска ожиданием не лечится: дальше только вечный цикл.
                // Помечаем сдачу (чтобы `flush` и `append` не тратили время) и
                // завершаем поток — иначе `Drop::join()` не пройдёт.
                shared.give_up.store(true, Ordering::Release);
                eprintln!(
                    "PowerBench: не удалось записать журнал в {} ({WRITE_GIVE_UP_AFTER} попытки \
                     подряд) — запись на диск прекращена, журнал остаётся только в памяти \
                     и попадёт в отчёт для поддержки",
                    path.display()
                );
                break;
            }
        }

        if shared.stop.load(Ordering::Acquire) {
            // На выходе пишем ещё раз: между последним `append` и `stop`
            // могла прийти ещё строка.
            let target = shared.appended.load(Ordering::Acquire);
            if shared.written.load(Ordering::Acquire) >= target {
                break;
            }
            // Пауза между попытками на выходе: иначе при продолжающихся
            // `append` цикл крутился бы вхолостую. Попытки всё равно
            // ограничены счётчиком `fails`, так что выход не зависнет.
            std::thread::sleep(Duration::from_millis(FLUSH_POLL_MS));
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

    /// Путь, на котором запись невозможна при любых условиях.
    ///
    /// Каталог подменён обычным файлом, поэтому `create_dir_all` внутри
    /// `persist` всегда отказывает — ровно ситуация «нет прав на
    /// `%LOCALAPPDATA%`» или «файл держит антивирус», только без зависимости
    /// от прав и чужих процессов.
    fn unwritable_path(tag: &str) -> PathBuf {
        let dir = temp_path(tag);
        let blocker = dir.with_extension("blocker");
        std::fs::remove_dir_all(&dir).unwrap_or(());
        std::fs::write(&blocker, "это файл, а не каталог").unwrap();
        blocker.join(LOG_FILE_NAME)
    }

    /// Регресс C5: постоянно падающая запись больше не вешает приложение.
    ///
    /// Раньше `flush()` и цикл писателя крутились бесконечно, пока `persist()`
    /// падал: приложение не закрывалось (висел `Drop::join()`), а запрос
    /// диагностики не отвечал. Теперь и `flush`, и выход ограничены по времени.
    #[test]
    fn a_permanently_failing_write_does_not_hang_the_logger() {
        let path = unwritable_path("blocked");
        let log = Logger::in_file(path.clone());
        log.append("info", "запись, которая не дойдёт до диска");

        // `flush` обязан уложиться в бюджет и не ждать подтверждения, которого
        // не будет никогда.
        let t0 = Instant::now();
        log.flush();
        let flush_took = t0.elapsed();
        assert!(
            flush_took < Duration::from_millis(FLUSH_BUDGET_MS + 2_000),
            "flush() не уложился в бюджет: {flush_took:?}"
        );

        // Данные не теряются молча: они в памяти и честно помечены как
        // не дошедшие до диска.
        assert_eq!(log.snapshot().len(), 1);
        let t1 = Instant::now();
        drop(log);
        let drop_took = t1.elapsed();
        assert!(
            drop_took
                < Duration::from_millis(
                    FLUSH_BUDGET_MS + (WRITE_GIVE_UP_AFTER as u64 + 1) * FLUSH_INTERVAL_MS + 2_000
                ),
            "Drop завис на join(): {drop_took:?}"
        );
    }

    /// Писатель сдаётся после ограниченного числа попыток, а не живёт вечно:
    /// иначе `Drop::join()` не проходит. Проверяем и сам факт сдачи, и то,
    /// что она честно отражена в `is_persisted`.
    #[test]
    fn the_writer_gives_up_instead_of_retrying_forever() {
        let path = unwritable_path("giveup");
        let log = Logger::in_file(path);
        log.append("info", "одна");
        // Ждём сдачи: писатель обязан исчерпать попытки и завершиться.
        let deadline = Instant::now()
            + Duration::from_millis(FLUSH_BUDGET_MS + (WRITE_GIVE_UP_AFTER as u64 + 1) * FLUSH_INTERVAL_MS + 2_000);
        while log.is_persisted() && Instant::now() < deadline {
            log.append("info", "подталкиваем писателя");
            std::thread::sleep(Duration::from_millis(FLUSH_POLL_MS));
        }
        assert!(
            !log.is_persisted(),
            "писатель не сдался после {WRITE_GIVE_UP_AFTER} неудачных попыток"
        );
        // `flush` после сдачи обязан выходить мгновенно, а не ждать.
        let t0 = Instant::now();
        log.flush();
        assert!(
            t0.elapsed() < Duration::from_millis(FLUSH_POLL_MS * 4),
            "flush() после сдачи писателя всё ещё ждёт: {:?}",
            t0.elapsed()
        );
        drop(log);
    }

    /// Обычный случай не сломать: успешная запись по-прежнему гарантируется,
    /// и `is_persisted` не срабатывает ложно.
    #[test]
    fn a_healthy_log_never_reports_persistence_failure() {
        let path = temp_path("healthy");
        let log = Logger::in_file(path.clone());
        for i in 0..20 {
            log.append("info", &format!("строка {i}"));
        }
        log.flush();
        assert!(log.is_persisted(), "здоровый журнал не должен сдаваться");
        assert_eq!(read_log(&path).len(), 20);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
