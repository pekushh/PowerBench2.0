//! Схемы управления электропитанием через `powercfg.exe`.
//!
//! stdout/stderr команды перенаправляются и перекодируются из OEM-кодировки
//! локали; код возврата проверяется. Разбор `powercfg /list` не зависит от
//! локали: GUID (36 символов формата uuid) + имя в скобках + необязательная
//! `*` активности в конце строки.

use std::io;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

// `creation_flags` живёт не в общем API `Command`, а в Windows-расширении.
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::power::decode_oem;

/// Потолок ожидания `powercfg`: переключение схемы не должно висеть вечно.
const POWERCFG_TIMEOUT_SECS: u64 = 10;

/// Ошибка вызова powercfg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerCfgError {
    pub operation: String,
    pub message: String,
}

impl PowerCfgError {
    fn new(operation: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            message: message.into(),
        }
    }
}

/// Одна схема управления электропитанием.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerScheme {
    pub guid: String,
    pub name: String,
    pub active: bool,
}

/// Длина символьной строки UUID: 8-4-4-4-12 с дефисами.
const UUID_LEN: usize = 36;

/// Проверка, что в `text` по индексу `start` начинается строго UUID.
/// Палитра дефисов: позиции 8, 13, 18, 23 (индексы с нуля).
fn is_uuid_at(text: &str, start: usize) -> bool {
    let bytes = text.as_bytes();
    if start + UUID_LEN > bytes.len() {
        return false;
    }
    for (off, &b) in bytes[start..start + UUID_LEN].iter().enumerate() {
        let is_dash_pos = off == 8 || off == 13 || off == 18 || off == 23;
        if is_dash_pos {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Найти первый UUID в тексте: вернуть диапазон [start, end).
pub fn find_uuid(text: &str) -> Option<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let mut start = 0usize;
    while start < bytes.len() {
        if is_uuid_at(text, start) {
            return Some(start..start + UUID_LEN);
        }
        start += 1;
    }
    None
}

/// Разбор одной строки `powercfg /list`: локале-независимый синтаксис
/// «GUID (имя) *».
///
/// Звёздочка активной схемы стоит **после закрывающей скобки**, но не внутри
/// имени. Раньше проверка была `tail.contains('*')` — схема с названием
/// «Мой \*профиль\*» считалась активной, и приложение переключало бы не ту
/// схему при восстановлении.
pub fn parse_scheme_line(line: &str) -> Option<PowerScheme> {
    let range = find_uuid(line)?;
    let guid = line[range.clone()].to_ascii_lowercase();
    let tail = &line[range.end..];
    let close = tail.rfind(')');
    let (name, after_name) = match (tail.find('('), close) {
        (Some(a), Some(b)) if b > a => (tail[a + 1..b].trim().to_string(), &tail[b + 1..]),
        // Без скобок: имя — весь хвост без завершающей звёздочки-маркера.
        _ => (
            tail.trim_end().trim_end_matches('*').trim().to_string(),
            tail,
        ),
    };
    let active = after_name.trim_end().ends_with('*');
    Some(PowerScheme { guid, name, active })
}

/// Абсолютный путь к `powercfg.exe`.
///
/// Имя без пути ищется по `PATH` **и по текущему каталогу** — приложение
/// запускается с правами администратора, поэтому подложенный рядом
/// `powercfg.exe` выполнялся бы с этими правами. Используем системный путь.
fn powercfg_path() -> std::path::PathBuf {
    let sys = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::path::PathBuf::from(sys)
        .join("System32")
        .join("powercfg.exe")
}

/// Флаг `CREATE_NO_WINDOW` из `winbase.h`.
///
/// Без него каждый вызов `powercfg.exe` мигает чёрным окном консоли. На
/// странице «Схемы» список и активная схема читаются при каждом открытии,
/// и пользователь видел пачку консолей. Приложение — GUI, дочерняя консоль
/// ему не нужна, а stdin/stdout и так перенаправлены в каналы.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Поток, читающий один канал `powercfg` целиком.
type PipeReader = std::thread::JoinHandle<io::Result<Vec<u8>>>;

/// Запустить поток чтения канала: ошибка пайпа возвращается вызывающему.
///
/// Раньше поток возвращал «сколько успел прочитать», а `Err` от
/// `read_to_end` выбрасывался в `_`. Частичный вывод из-за оборванного пайпа
/// разбирался как валидный: для `/list` это `original_scheme_guid = None`,
/// то есть потерянная исходная схема питания, которую потом нечем вернуть.
fn spawn_pipe_reader<R>(mut pipe: Option<R>) -> PipeReader
where
    R: io::Read + Send + 'static,
{
    std::thread::spawn(move || -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        if let Some(p) = pipe.as_mut() {
            io::Read::read_to_end(p, &mut buf)?;
        }
        Ok(buf)
    })
}

/// Итог потока-читателя: и ошибка пайпа, и паника — одинаково фатальны.
///
/// Прежний `unwrap_or_else(|_| Vec::new())` превращал и панику, и сбой в
/// «powercfg ничего не сказал» — то есть в пустой, но валидный вывод.
/// Отличать «пусто» от «не прочитано» обязан сам вызывающий: цена ошибки —
/// потерянная исходная схема питания пользователя.
fn collect_pipe_reader(handle: PipeReader, what: &str) -> io::Result<Vec<u8>> {
    match handle.join() {
        Ok(result) => result.map_err(|e| io::Error::other(format!("канал {what}: {e}"))),
        Err(_) => Err(io::Error::other(format!(
            "поток чтения канала {what} завершился паникой"
        ))),
    }
}

/// Выполнить powercfg с аргументами; вернуть перекодированный вывод.
///
/// Вызов ограничен по времени: зависший `powercfg.exe` иначе держал бы поток
/// сессии (а вместе с ним — переключение схем питания) неограниченно долго.
///
/// Вывод читается двумя отдельными потоками **до** ожидания завершения.
/// Иначе получается классическая взаимоблокировка: буфер анонимного канала
/// ограничен (4 КБ), `powercfg /list` с несколькими схемами в него не влезает,
/// процесс блокируется на записи и уже никогда не выходит — потолок ожидания
/// срабатывал всегда, и проверка готовности сообщала «powercfg недоступен».
fn run_powercfg(operation: &str, args: &[&str]) -> Result<String, PowerCfgError> {
    let mut cmd = Command::new(powercfg_path());
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd.spawn().map_err(|e| {
        PowerCfgError::new(operation, format!("не удалось запустить powercfg: {e}"))
    })?;

    // Читатели забирают содержимое каналов, пока процесс жив, иначе он встанет
    // на переполненном буфере. Ошибка чтения пробрасывается наверх (см.
    // [`spawn_pipe_reader`]), а не превращается в пустой вывод.
    let stdout_reader = spawn_pipe_reader(child.stdout.take());
    let stderr_reader = spawn_pipe_reader(child.stderr.take());

    // Читатели заканчиваются, как только закрывается конец канала, а он
    // закрывается вместе с процессом. Поэтому join после `kill`+`wait`
    // не может висеть и делает уборку потоков детерминированной.
    let join_readers = || -> io::Result<(Vec<u8>, Vec<u8>)> {
        Ok((
            collect_pipe_reader(stdout_reader, "stdout")?,
            collect_pipe_reader(stderr_reader, "stderr")?,
        ))
    };

    // Ждём с потолком; по истечении — снимаем процесс и возвращаем ошибку.
    let deadline = Instant::now() + Duration::from_secs(POWERCFG_TIMEOUT_SECS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Раньше потоки оставались болтаться после возврата
                    // ошибки: приложение жило месяцами, и каждый зависший
                    // `powercfg` оставлял после себя пару висящих потоков.
                    // Ошибку чтения при этом уже не разбираем: потолок
                    // ожидания — сам по себе исчерпывающая причина отказа.
                    let _ = join_readers();
                    return Err(PowerCfgError::new(
                        operation,
                        format!("powercfg не ответил за {} с", POWERCFG_TIMEOUT_SECS),
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_readers();
                return Err(PowerCfgError::new(
                    operation,
                    format!("не удалось дождаться завершения powercfg: {e}"),
                ));
            }
        }
    };
    let (stdout, stderr) = join_readers()
        .map_err(|e| PowerCfgError::new(operation, format!("вывод powercfg не прочитан: {e}")))?;
    let stdout = decode_oem(&stdout);
    if !status.success() {
        let stderr = decode_oem(&stderr);
        let detail = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        return Err(PowerCfgError::new(
            operation,
            format!(
                "код возврата {}: {}",
                status.code().unwrap_or(-1),
                detail.trim()
            ),
        ));
    }
    Ok(stdout)
}

/// Список схем питания.
pub fn list_schemes() -> Result<Vec<PowerScheme>, PowerCfgError> {
    let stdout = run_powercfg("list", &["/list"])?;
    Ok(stdout.lines().filter_map(parse_scheme_line).collect())
}

/// Активировать схему питания по GUID.
pub fn activate(guid: &str) -> Result<(), PowerCfgError> {
    run_powercfg("setactive", &["/setactive", guid])?;
    Ok(())
}

/// GUID активной схемы питания.
///
/// Нужен эталоном дрейфа: без него CLI не мог оценить дрейф машины вообще
/// и выдавал другой вердикт, чем приложение на тех же данных.
pub fn active_scheme() -> Result<String, PowerCfgError> {
    let stdout = run_powercfg("getactive", &["/getactivescheme"])?;
    parse_active_scheme(&stdout)
}

/// Вытащить GUID из ответа `powercfg /getactivescheme`.
fn parse_active_scheme(stdout: &str) -> Result<String, PowerCfgError> {
    // Вывод: `Power Scheme GUID: 381b4222-... (Balanced)`
    let r = find_uuid(stdout)
        .ok_or_else(|| PowerCfgError::new("getactive", "в выводе нет GUID активной схемы"))?;
    let guid = stdout[r].trim().to_string();
    if guid.is_empty() {
        return Err(PowerCfgError::new(
            "getactive",
            "пустой GUID активной схемы",
        ));
    }
    Ok(guid)
}

/// Полный дамп настроек схемы питания (`powercfg /query`).
///
/// Зачем он сохраняется в отчёт: GUID говорит только «какая это схема», а не
/// «что в ней задано». Windows молча правит планы (обновления), OEM-агенты
/// — постоянно. Через год в истории остаётся `381b4222-…`, и по нему
/// нельзя понять, был ли при замере поднят минимальный/максимальный
/// processor state, какая политика охлаждения и агрессивность буста. Для
/// инструмента, который сравнивает планы питания, это делает результат
/// невоспроизводимым.
///
/// Вывод локализован, поэтому сохраняется как есть, вместе с датой съёма.
pub fn query(guid: &str) -> Result<String, PowerCfgError> {
    let g = guid.trim();
    if g.is_empty() {
        return Err(PowerCfgError::new("query", "не указан GUID схемы"));
    }
    run_powercfg("query", &["/query", g])
}

/// Снять дампы настроек для набора схем: `guid -> текст /query`.
///
/// Схемы, для которых дамп снять не удалось, молча пропускаются: отсутствие
/// дампа в отчёте не должно ронять сессию из-за одного отказа. Вызывающий
/// узнает о пропуске по тому, что схемы нет в карте.
pub fn query_many(guids: &[String]) -> Vec<(String, String)> {
    guids
        .iter()
        .filter_map(|g| query(g).ok().map(|text| (g.clone(), text)))
        .collect()
}

/// Продублировать схему; возвращает GUID новой схемы.
///
/// В выводе powercfg может встретиться GUID исходной схемы, поэтому берём
/// первый, отличный от него. Прежний вариант с `or_else(|| uuids.first())`
/// возвращал исходный GUID — интерфейс показывал бы «дубль успешно создан»,
/// указывая на ту же самую схему.
pub fn duplicate(guid: &str) -> Result<String, PowerCfgError> {
    let stdout = run_powercfg("duplicatescheme", &["/duplicatescheme", guid])?;
    let mut uuids: Vec<&str> = Vec::new();
    let mut rest: &str = stdout.as_str();
    while let Some(r) = find_uuid(rest) {
        uuids.push(&rest[r.clone()]);
        rest = &rest[r.end..];
    }
    let want = guid.to_ascii_lowercase();
    uuids
        .iter()
        .find(|u| u.to_ascii_lowercase() != want)
        .map(|u| u.to_ascii_lowercase())
        .ok_or_else(|| {
            PowerCfgError::new(
                "duplicatescheme",
                "в выводе powercfg не найден GUID новой схемы",
            )
        })
}

/// Экспортировать схему в файл `.pow`.
///
/// `powercfg /export` при ошибке иногда возвращает код 0, не создав файл
/// (например, когда каталог назначения недоступен для записи). Поэтому
/// результат проверяется по самому файлу: иначе интерфейс сообщал бы «схема
/// экспортирована», а на диске ничего не было бы.
pub fn export(guid: &str, path: &Path) -> Result<(), PowerCfgError> {
    let g = guid.trim();
    if g.is_empty() {
        return Err(PowerCfgError::new("export", "не указан GUID схемы"));
    }
    let arg = path
        .to_str()
        .ok_or_else(|| PowerCfgError::new("export", "путь к .pow имеет неверную кодировку"))?;
    run_powercfg("export", &["/export", g, arg])?;
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 0 => Ok(()),
        Ok(_) => Err(PowerCfgError::new(
            "export",
            "файл .pow создан, но пуст — вероятно, powercfg не смог записать схему",
        )),
        Err(e) => Err(PowerCfgError::new(
            "export",
            format!("файл .pow не создан: {e}"),
        )),
    }
}

/// Удалить схему по GUID.
pub fn delete(guid: &str) -> Result<(), PowerCfgError> {
    run_powercfg("delete", &["/delete", guid])?;
    Ok(())
}

/// Импортировать схему из файла `.pow`; возвращает GUID импортированной схемы.
pub fn import(path: &Path) -> Result<String, PowerCfgError> {
    let arg = path
        .to_str()
        .ok_or_else(|| PowerCfgError::new("import", "путь к .pow имеет неверную кодировку"))?;
    let stdout = run_powercfg("import", &["/import", arg])?;
    match find_uuid(&stdout) {
        Some(r) => Ok(stdout[r].to_ascii_lowercase()),
        None => Err(PowerCfgError::new(
            "import",
            "в выводе powercfg не найден GUID импортированной схемы",
        )),
    }
}

/// Восстановить стандартные схемы (активной становится системная по умолчанию).
pub fn restore_defaults() -> Result<(), PowerCfgError> {
    run_powercfg("restoredefaultschemes", &["/restoredefaultschemes"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Регрессия C4: оборванный канал не должен выглядеть как пустой вывод.
    ///
    /// Прежний поток-читатель возвращал `Vec<u8>` и ронял `Err` от
    /// `read_to_end` в `_`. Частичный вывод `powercfg /list` после обрыва
    /// пайпа разбирался как валидный и давал `original_scheme_guid = None` —
    /// исходная схема питания терялась безвозвратно.
    #[test]
    fn broken_pipe_is_not_reported_as_empty_output() {
        /// Читатель: отдаёт начало вывода, потом рвёт канал.
        struct Truncated {
            served: bool,
        }
        impl io::Read for Truncated {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.served {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "обрыв канала",
                    ));
                }
                self.served = true;
                let head = b"381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)\r\n";
                let n = head.len().min(buf.len());
                buf[..n].copy_from_slice(&head[..n]);
                Ok(n)
            }
        }

        let handle = spawn_pipe_reader(Some(Truncated { served: false }));
        let err = collect_pipe_reader(handle, "stdout").expect_err("обрыв пайпа — фатальная ошибка");
        let text = err.to_string();
        assert!(
            text.contains("обрыв канала"),
            "причина сбоя чтения потеряна: {text}"
        );
        assert!(text.contains("stdout"), "не сказано, какой канал: {text}");
    }

    /// Паника в потоке-читателе — тоже отказ, а не «powercfg молчал».
    #[test]
    fn panicking_reader_thread_is_a_fatal_error() {
        let handle = std::thread::spawn(|| -> io::Result<Vec<u8>> {
            panic!("упало при чтении");
        });
        let err =
            collect_pipe_reader(handle, "stderr").expect_err("паника читателя не должна прятаться");
        assert!(
            err.to_string().contains("паник"),
            "паника не опознана: {err}"
        );
    }

    /// Отсутствие канала (`Stdio::piped()` не сработало) — пустой вывод, а не
    /// ошибка: канала просто нет, и это не обрыв.
    #[test]
    fn absent_pipe_reads_as_empty_output() {
        let handle = spawn_pipe_reader(None::<std::io::Empty>);
        assert_eq!(
            collect_pipe_reader(handle, "stdout").expect("нет канала"),
            Vec::<u8>::new()
        );
    }

    /// Целый канал читается целиком — базовый контракт потока.
    #[test]
    fn whole_pipe_is_read_to_the_end() {
        let payload: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 251) as u8).collect();
        let expected = payload.clone();
        let handle = spawn_pipe_reader(Some(std::io::Cursor::new(payload)));
        assert_eq!(
            collect_pipe_reader(handle, "stdout").expect("канал цел"),
            expected
        );
    }

    /// Регрессия: `powercfg /list` на этой машине выдаёт ~9 КБ, а буфер
    /// анонимного канала — 4 КБ. Если вывод не читается параллельно с
    /// ожиданием, процесс встаёт на записи и всегда срывается по потолку
    /// ожидания. Тест ловит именно эту взаимоблокировку на живом `powercfg`.
    #[test]
    fn list_schemes_survives_output_larger_than_pipe_buffer() {
        let schemes = match list_schemes() {
            Ok(s) => s,
            Err(e) => panic!("powercfg недоступен: {}", e.message),
        };
        assert!(
            !schemes.is_empty(),
            "powercfg вернул пустой список — разбор вывода сломан"
        );
        assert!(
            schemes.iter().any(|s| s.guid.len() == 36),
            "ни одна строка не разобралась как GUID"
        );
    }

    #[test]
    fn parse_scheme_line_extracts_guid_name_and_active() {
        // Русская локаль: необязательная метка активности в конце строки.
        let ru_active =
            "GUID схемы питания: 381b4222-f694-41f0-9685-ff5bb260df2e  (Сбалансированная) *";
        let s = parse_scheme_line(ru_active).unwrap();
        assert_eq!(s.guid, "381b4222-f694-41f0-9685-ff5bb260df2e");
        assert_eq!(s.name, "Сбалансированная");
        assert!(s.active);

        // Английская локаль: «(Balanced)» без звёздочки.
        let en = parse_scheme_line("GUID scheme: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)")
            .unwrap();
        assert_eq!(en.name, "Balanced");
        assert!(!en.active);
    }

    #[test]
    fn parse_ignores_lines_without_uuid() {
        assert!(parse_scheme_line("Существующие схемы управления электропитанием").is_none());
        assert!(parse_scheme_line("").is_none());
    }

    /// Регресс: звёздочка внутри имени не должна считаться признаком активной
    /// схемы, иначе приложение переключало бы не ту схему при восстановлении.
    #[test]
    fn asterisk_inside_name_is_not_active_marker() {
        let line = "GUID схемы питания: 381b4222-f694-41f0-9685-ff5bb260df2e  (Мой *профиль*)";
        let s = parse_scheme_line(line).unwrap();
        assert_eq!(s.name, "Мой *профиль*");
        assert!(
            !s.active,
            "звёздочка в имени ошибочно принята за маркер активности"
        );

        // Настоящий маркер — после закрывающей скобки.
        let active = "381b4222-f694-41f0-9685-ff5bb260df2e  (Мой *профиль*) *";
        assert!(parse_scheme_line(active).unwrap().active);
        assert_eq!(
            parse_scheme_line(active).unwrap().name,
            "Мой *профиль*",
            "маркер активности попал в имя"
        );
    }

    /// Регресс: без скобок активная схема определяется по звёздочке в хвосте.
    #[test]
    fn active_without_parentheses() {
        let s = parse_scheme_line("381b4222-f694-41f0-9685-ff5bb260df2e  Без скобок *").unwrap();
        assert!(s.active);
        assert_eq!(s.name, "Без скобок");
    }

    #[test]
    fn find_uuid_locates_first_occurrence() {
        let text = "пишем: 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c и ещё 381b4222-f694-41f0-9685-ff5bb260df2e";
        let r = find_uuid(text).unwrap();
        assert_eq!(&text[r], "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c");

        // Полный диапазон 36 символов (с дефисами).
        let r2 = find_uuid("1-2-3-4-5").is_none();
        assert!(r2);
    }

    #[test]
    fn uuid_validation_rejects_bad_forms() {
        // Слишком короткая строка после хвостика — не UUID.
        assert!(!is_uuid_at("381b4222f69441f09685", 0));
        // Не hex-символ на месте цифры.
        assert!(!is_uuid_at("381b4222-f694-41f0-9685-ff5bb260df2G", 0));
        // Дефис не на своём месте.
        assert!(!is_uuid_at("381b4222f694-41f0-9685-ff5bb260df2e", 0));
        // Верхний регистр — тоже hex.
        assert!(is_uuid_at("381B4222-F694-41F0-9685-FF5BB260DF2E", 0));
    }

    #[test]
    fn duplicate_picks_guid_different_from_source() {
        // Исходный GUID иногда попадает в вывод — берём отличный от него.
        let out = "Скопировано: 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c\r\n";
        let source = "381b4222-f694-41f0-9685-ff5bb260df2e";
        let src_overlay = format!("...{source}... {out}");
        let mut uuids: Vec<&str> = Vec::new();
        let mut rest: &str = src_overlay.as_str();
        while let Some(r) = find_uuid(rest) {
            uuids.push(&rest[r.clone()]);
            rest = &rest[r.end..];
        }
        let chosen = uuids
            .iter()
            .find(|u| **u != source)
            .or_else(|| uuids.first())
            .unwrap();
        assert_eq!(*chosen, "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c");
    }

    #[test]
    fn active_scheme_guid_is_extracted_from_getactive_output() {
        // Реальный вывод: подпись может быть русской, имя схемы - в скобках.
        let out = "Power Scheme GUID: 03753f69-7473-464f-9f9b-f37dd2e465cc (Community-Plan-V3)\r\n";
        assert_eq!(
            parse_active_scheme(out).unwrap(),
            "03753f69-7473-464f-9f9b-f37dd2e465cc"
        );
        let ru = "Схема электропитания GUID: 381b4222-f694-41f0-9685-ff5bb260df2e (Сбалансированная)\r\n";
        assert_eq!(
            parse_active_scheme(ru).unwrap(),
            "381b4222-f694-41f0-9685-ff5bb260df2e"
        );
        // Без GUID это ошибка, а не пустая строка: пустой эталон молча ломает
        // расчёт дрейфа машины.
        let err = parse_active_scheme("Power Scheme GUID:\r\n").expect_err("нет GUID");
        assert!(
            err.message.contains("GUID"),
            "сообщение должно называть GUID"
        );
    }
}
