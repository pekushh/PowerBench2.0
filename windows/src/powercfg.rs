//! Схемы управления электропитанием через `powercfg.exe`.
//!
//! stdout/stderr команды перенаправляются и перекодируются из OEM-кодировки
//! локали; код возврата проверяется. Разбор `powercfg /list` не зависит от
//! локали: GUID (36 символов формата uuid) + имя в скобках + необязательная
//! `*` активности в конце строки.

use std::path::Path;
use std::process::Command;

use crate::power::decode_oem;

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
/// «GUID (имя) *». Возвращает `None` для строк без UUID (заголовки и пустые).
pub fn parse_scheme_line(line: &str) -> Option<PowerScheme> {
    let range = find_uuid(line)?;
    let guid = line[range.clone()].to_ascii_lowercase();
    let tail = &line[range.end..];
    let name = match (tail.find('('), tail.rfind(')')) {
        (Some(a), Some(b)) if b > a => tail[a + 1..b].trim().to_string(),
        _ => String::new(),
    };
    let active = tail.contains('*');
    Some(PowerScheme { guid, name, active })
}

/// Выполнить powercfg с аргументами; вернуть перекодированный вывод.
fn run_powercfg(operation: &str, args: &[&str]) -> Result<String, PowerCfgError> {
    let output = Command::new("powercfg")
        .args(args)
        .output()
        .map_err(|e| PowerCfgError::new(operation, format!("не удалось запустить powercfg: {e}")))?;
    let stdout = decode_oem(&output.stdout);
    if !output.status.success() {
        let stderr = decode_oem(&output.stderr);
        let detail = if stderr.trim().is_empty() { stdout } else { stderr };
        return Err(PowerCfgError::new(
            operation,
            format!("код возврата {}: {}", output.status.code().unwrap_or(-1), detail.trim()),
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

/// Продублировать схему; возвращает GUID новой схемы (первый UUID в выводе,
/// отличный от исходного, если исходный тоже попал в вывод).
pub fn duplicate(guid: &str) -> Result<String, PowerCfgError> {
    let stdout = run_powercfg("duplicatescheme", &["/duplicatescheme", guid])?;
    let mut uuids: Vec<&str> = Vec::new();
    let mut rest: &str = stdout.as_str();
    while let Some(r) = find_uuid(rest) {
        uuids.push(&rest[r.clone()]);
        rest = &rest[r.end..];
    }
    let new_guid = uuids
        .iter()
        .find(|u| **u != guid.to_ascii_lowercase())
        .or_else(|| uuids.first());
    new_guid.map(|u| u.to_ascii_lowercase()).ok_or_else(|| {
        PowerCfgError::new("duplicatescheme", "в выводе powercfg не найден GUID новой схемы")
    })
}

/// Удалить схему по GUID.
pub fn delete(guid: &str) -> Result<(), PowerCfgError> {
    run_powercfg("delete", &["/delete", guid])?;
    Ok(())
}

/// Импортировать схему из файла `.pow`; возвращает GUID импортированной схемы.
pub fn import(path: &Path) -> Result<String, PowerCfgError> {
    let arg = path.to_str().ok_or_else(|| {
        PowerCfgError::new("import", "путь к .pow имеет неверную кодировку")
    })?;
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

    #[test]
    fn parse_scheme_line_extracts_guid_name_and_active() {
        // Русская локаль: необязательная метка активности в конце строки.
        let ru_active = "GUID схемы питания: 381b4222-f694-41f0-9685-ff5bb260df2e  (Сбалансированная) *";
        let s = parse_scheme_line(ru_active).unwrap();
        assert_eq!(s.guid, "381b4222-f694-41f0-9685-ff5bb260df2e");
        assert_eq!(s.name, "Сбалансированная");
        assert!(s.active);

        // Английская локаль: «(Balanced)» без звёздочки.
        let en = parse_scheme_line("GUID scheme: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)").unwrap();
        assert_eq!(en.name, "Balanced");
        assert!(!en.active);
    }

    #[test]
    fn parse_ignores_lines_without_uuid() {
        assert!(parse_scheme_line("Существующие схемы управления электропитанием").is_none());
        assert!(parse_scheme_line("").is_none());
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
}