//! Атомарная запись файлов и безопасные операции «прочитать-изменить-записать».
//!
//! Зачем отдельный модуль: приложение пишет одни и те же файлы из двух
//! источников — фонового потока сессии (чекпоинт, карантин) и IPC-потоков
//! интерфейса (настройки, журнал). Раньше `atomic_write` использовал одно
//! фиксированное имя временного файла на весь процесс:
//!
//! ```text
//! писатель A: write("data.json.tmp") ──┐
//! писатель B:       write("data.json.tmp") ─┐   ← затирает временный файл A
//! писатель A:              MoveFileEx(tmp → data.json)   ← публикует данные B
//! писатель B:              MoveFileEx(tmp → data.json)   ← падает, потеря записи
//! ```
//!
//! Здесь две гарантии:
//!  1. **Уникальное имя временного файла** на каждую запись, поэтому
//!     параллельные писатели не мешают друг другу.
//!  2. **Блокировка по пути** вокруг read-modify-write, чтобы две команды
//!     интерфейса не теряли правки друг друга (например, одновременное
//!     изменение избранного и настроек сканирования).
//!
//! Сама публикация атомарна на всех платформах: сначала полностью пишем во
//! временный файл, затем переименовываем поверх целевого (на Windows —
//! `MoveFileExW` с `REPLACE_EXISTING | WRITE_THROUGH`).

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Счётчик для уникальных имён временных файлов в пределах процесса.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Сколько раз повторять публикацию, если целевой файл занят другим
/// процессом/потоком (ERROR_ACCESS_DENIED / ERROR_SHARING_VIOLATION).
const PUBLISH_ATTEMPTS: u32 = 8;
/// Пауза перед очередной попыткой публикации (умножается на номер попытки).
const PUBLISH_RETRY_MS: u64 = 2;

/// Блокировки в процессе: ключ — нормализованный путь, значение — мьютекс.
fn path_locks() -> &'static Mutex<HashMap<PathBuf, &'static Mutex<()>>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, &'static Mutex<()>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Взять блокировку файла. Блокировки переиспользуются, `Mutex` статический —
/// отравление мьютекса не должно ломать последующие вызовы, поэтому
/// игнорируем его (`PoisonError::into_inner`).
pub fn lock_file(path: &Path) -> MutexGuard<'static, ()> {
    let key = path.to_path_buf();
    let mutex: &'static Mutex<()> = {
        let mut map = path_locks().lock().unwrap_or_else(|e| e.into_inner());
        map.entry(key)
            .or_insert_with(|| Box::leak(Box::new(Mutex::new(()))))
    };
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Выполнить «прочитать-изменить-записать» под блокировкой файла.
///
/// `update` получает текущее содержимое (пустой вектор, если файла нет или он
/// не читается) и возвращает новое. Запись атомарная, повторов при гонке нет:
/// гонка исключена блокировкой.
pub fn update_file<F>(path: &Path, update: F) -> io::Result<()>
where
    F: FnOnce(&[u8]) -> Vec<u8>,
{
    let _guard = lock_file(path);
    let current = std::fs::read(path).unwrap_or_default();
    let next = update(&current);
    atomic_write(path, &next)
}

/// Атомарная запись: полная запись во временный файл + переименование.
/// Временный файл получает уникальное имя (счётчик + id потока), поэтому
/// параллельные вызовы не затирают временные файлы друг друга.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = unique_tmp_path(path);
    // Ошибка записи обязана убрать временный файл, иначе мусор копится.
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = publish(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Уникальный временный путь рядом с целью (тот же каталог — чтобы
/// переименование было атомарным в пределах тома).
fn unique_tmp_path(path: &Path) -> PathBuf {
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "data".to_string());
    let pid = std::process::id();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(".{stem}.{pid}.{seq}.tmp"))
}

/// Опубликовать временный файл поверх целевого.
///
/// На Windows `MoveFileExW` отказывает с `ACCESS_DENIED`, если целевой файл
/// в этот момент открыт кем-то ещё (интерфейс читает чекпоинт, пока поток
/// сессии его пишет). Это штатная ситуация, поэтому делаем несколько попыток
/// с короткой паузой — файл успеет закрыться.
#[cfg(windows)]
fn publish(tmp: &Path, path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let from = wide(tmp);
    let to = wide(path);
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
    let mut last = io::Error::last_os_error();
    for attempt in 0..PUBLISH_ATTEMPTS {
        let ok = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) };
        if ok != 0 {
            return Ok(());
        }
        last = io::Error::last_os_error();
        if !matches!(last.raw_os_error(), Some(5) | Some(32)) {
            // Не «файл занят» — повтор не поможет.
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(
            PUBLISH_RETRY_MS * u64::from(attempt + 1),
        ));
    }
    Err(last)
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(not(windows))]
fn publish(tmp: &Path, path: &Path) -> io::Result<()> {
    // `rename` поверх существующего файла атомарен на POSIX.
    std::fs::rename(tmp, path)
}

/// Прочитать файл, вернуть `None` при отсутствии или битом содержимом.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Прочитать JSON, различая «файла нет» и «файл есть, но прочитать нельзя».
///
/// `read_json` возвращает `None` в обоих случаях, и вызывающий трактовал
/// битый чекпоинт как отсутствующий: сессия начиналась с нуля, а первая же
/// запись затирала файл. Для чекпоинта это означало тихую потерю всех
/// выполненных прогонов.
pub fn read_json_checked<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("не удалось прочитать {}: {e}", path.display())),
    };
    if text.trim().is_empty() {
        // Пустой файл после сбоя записи — это не «нет чекпоинта», но и не
        // повод для паники: трактуем как отсутствие.
        return Ok(None);
    }
    match serde_json::from_str(&text) {
        Ok(v) => Ok(Some(v)),
        Err(e) => Err(format!(
            "повреждён {} ({e}); файл сохранён как есть, ничего не перезаписано",
            path.display()
        )),
    }
}

/// Удалить файл; отсутствие файла — не ошибка.
pub fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    /// Битый JSON — это ошибка, а не «файла нет».
    ///
    /// `read_json` возвращает `None` в обоих случаях, и чекпоинт с битым
    /// содержимым молча считался отсутствующим: сессия начиналась с нуля, а
    /// первая запись затирала все выполненные прогоны.
    #[test]
    fn corrupt_json_is_an_error_not_absence() {
        let dir = std::env::temp_dir().join("powerbench-read-checked-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.json");

        // Файла нет — это «нет данных», а не ошибка.
        let missing: Result<Option<i32>, String> = read_json_checked(&path);
        assert!(matches!(missing, Ok(None)), "отсутствующий файл не ошибка");

        // Файл есть, но битый: раньше это молча трактовалось как отсутствие.
        std::fs::write(&path, b"{ not json").unwrap();
        match read_json_checked::<i32>(&path) {
            Err(msg) => assert!(msg.contains("повреждён"), "нет диагноза: {msg}"),
            Ok(v) => panic!("битый файл не должен читаться как Ok({v:?})"),
        }

        // Пустой файл после сбоя записи — отсутствие, а не паника.
        std::fs::write(&path, b"   ").unwrap();
        let empty: Result<Option<i32>, String> = read_json_checked(&path);
        assert!(matches!(empty, Ok(None)), "пустой файл не ошибка");

        // Корректный файл читается как раньше.
        std::fs::write(&path, b"7").unwrap();
        assert_eq!(read_json_checked::<i32>(&path).unwrap(), Some(7));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("powerbench-atomicio-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_write_replaces_content() {
        let dir = tmp_dir("replace");
        let f = dir.join("a.json");
        atomic_write(&f, b"one").unwrap();
        atomic_write(&f, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Регресс: параллельные записи не должны терять данные и оставлять
    /// чужие временные файлы.
    #[test]
    fn concurrent_writes_do_not_collide() {
        let dir = tmp_dir("concurrent");
        let path = Arc::new(dir.join("shared.json"));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        let payload = format!("{{\"writer\":{i}}}").into_bytes();
                        atomic_write(&path, &payload).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // Файл существует, читается, временных файлов не осталось.
        let text = std::fs::read_to_string(path.as_path()).unwrap();
        assert!(text.contains("\"writer\""));
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "остались tmp: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Регресс: параллельные read-modify-write не теряют правки.
    #[test]
    fn update_file_serializes_read_modify_write() {
        let dir = tmp_dir("rmw");
        let path = Arc::new(dir.join("counter.json"));
        update_file(&path, |_| b"[]".to_vec()).unwrap();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        update_file(&path, |cur| {
                            let mut list: Vec<String> = String::from_utf8_lossy(cur)
                                .is_empty()
                                .then(Vec::new)
                                .unwrap_or_else(|| {
                                    serde_json::from_slice::<Vec<String>>(cur).unwrap_or_default()
                                });
                            list.push(format!("w{i}"));
                            serde_json::to_vec(&list).unwrap()
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let list: Vec<String> = read_json(path.as_path()).expect("файл читается как список");
        assert_eq!(list.len(), 80, "правки потеряны: {}", list.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_is_idempotent() {
        let dir = tmp_dir("remove");
        let f = dir.join("x.bin");
        atomic_write(&f, b"data").unwrap();
        remove_file_if_exists(&f).unwrap();
        remove_file_if_exists(&f).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
