//! Низкоуровневые утилиты Windows: права администратора, запрет сна,
//! питание от сети, идентификатор CPU, частота таймера QPC, перекодирование
//! OEM-вывода в Unicode.

#[cfg(windows)]
use windows_sys::Win32::Globalization::MultiByteToWideChar;
#[cfg(windows)]
use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;
#[cfg(windows)]
use windows_sys::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, EXECUTION_STATE, GetSystemPowerStatus,
    SYSTEM_POWER_STATUS, SetThreadExecutionState,
};
#[cfg(windows)]
use windows_sys::Win32::UI::Shell::IsUserAnAdmin;
/// Версия диагностики (часть CompatibilitySignature).
pub const DIAGNOSTICS_VERSION: &str = "0.1.0";

/// Ошибка работы с электропитанием.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerError {
    /// Не удалось выставить запрет сна/отключения дисплея.
    SleepPreventionFailed,
    /// Не удалось определить статус питания (система вернула «неизвестно»).
    AcStatusUnknown,
}

/// Версия диагностики.
pub fn diagnostics_version() -> &'static str {
    DIAGNOSTICS_VERSION
}

/// Частота таймера QPC в Гц.
pub fn qpc_frequency() -> u64 {
    #[cfg(windows)]
    {
        let mut freq: i64 = 0;
        // Вызов успешен: QueryPerformanceFrequency не документирует природу
        // возвращаемого BOOL — при нуле принимаем консервативное значение.
        unsafe { QueryPerformanceFrequency(&mut freq) };
        if freq > 0 { freq as u64 } else { 10_000_000 }
    }
    #[cfg(not(windows))]
    {
        10_000_000
    }
}

/// Идентификатор CPU: `PROCESSOR_IDENTIFIER`, иначе архитектура.
pub fn cpu_identifier() -> String {
    std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| std::env::consts::ARCH.to_string())
}

/// Проверка прав администратора при старте.
///
/// `IsUserAnAdmin` — shell-API: он возвращает `true` для админской учётной
/// записи, запущенной **без повышения** (split token). Тогда диагностика
/// считала окружение готовым, а `powercfg /setactive` падал с «access denied»
/// уже посреди бенчмарка. Поэтому спрашиваем настоящее состояние токена:
/// `TokenElevation == 1` означает, что процесс действительно повышен.
pub fn is_admin() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TokenElevation};
        // Псевдодескриптор текущего процесса (GetCurrentProcess()).
        const CURRENT_PROCESS: HANDLE = std::ptr::null_mut();
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned: u32 = 0;
        let ok = unsafe {
            GetTokenInformation(
                CURRENT_PROCESS,
                TokenElevation,
                std::ptr::addr_of_mut!(elevation).cast(),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        };
        if ok != 0 {
            return elevation.TokenIsElevated != 0;
        }
        // Не удалось определить — не блокируем запуск из-за проверки.
        unsafe { IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Перекодировать байты вывода системной утилиты в Unicode.
///
/// Кодировка вывода `powercfg` **не фиксирована**: она следует за кодовой
/// страницей консоли, из которой запущена программа. В одном окружении это
/// CP866, в другом — UTF-8 (например, при включённой опции «Использовать
/// UTF-8» или когда у процесса нет консоли).
///
/// Раньше вывод всегда трактовался как OEM, из-за чего схемы с кириллическими
/// именами выглядели как «╨Ь╨░╨║╤Б╨╕╨╝» — валидный UTF-8, прочитанный как
/// однобайтовая кириллица. Поэтому сначала проверяем UTF-8 (строгий разбор),
/// и только если он невалиден — откатываемся на OEM-кодировку.
pub fn decode_oem(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    // Строгий разбор: `from_utf8` отвергает и неполные последовательности, и
    // одиночные байты CP866, поэтому ложных срабатываний на кириллице нет.
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    decode_oem_strict(bytes)
}

/// Декодирование строго по OEM-кодировке, без попытки угадать UTF-8.
fn decode_oem_strict(bytes: &[u8]) -> String {
    #[cfg(windows)]
    {
        const CP_OEMCP: u32 = 1;
        let needed = unsafe {
            MultiByteToWideChar(
                CP_OEMCP,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                std::ptr::null_mut(),
                0,
            )
        };
        if needed <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buf = vec![0u16; needed as usize];
        let written = unsafe {
            MultiByteToWideChar(
                CP_OEMCP,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                buf.as_mut_ptr(),
                needed,
            )
        };
        if written <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        buf.truncate(written as usize);
        String::from_utf16(&buf).unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned())
    }
    #[cfg(not(windows))]
    {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Запрет сна и отключения дисплея на время сессии; снимается при Drop.
///
/// `SetThreadExecutionState` возвращает **предыдущее** состояние потока, и его
/// нужно вернуть при снятии guard'а. Прежний код ставил `ES_CONTINUOUS` (то
/// есть «разрешить сон»), из-за чего два вложенных guard'а ломали друг друга:
/// снятие внешнего разрешало сон, пока внутренний ещё был жив.
#[derive(Debug)]
pub struct SleepGuard {
    #[cfg(windows)]
    previous: EXECUTION_STATE,
    #[cfg(windows)]
    active: bool,
}

impl SleepGuard {
    /// Выставить запрет сна. Повторный вызов создаёт независимый guard.
    pub fn prevent() -> Result<Self, PowerError> {
        #[cfg(windows)]
        {
            let flags: EXECUTION_STATE = ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED;
            let prev = unsafe { SetThreadExecutionState(flags) };
            if prev == 0 {
                return Err(PowerError::SleepPreventionFailed);
            }
            Ok(Self {
                previous: prev,
                active: true,
            })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if self.active {
                // Возвращаем состояние, которое было до guard'а, а не «разрешить
                // сон»: так вложенные guard'а работают корректно.
                unsafe { SetThreadExecutionState(self.previous) };
                self.active = false;
            }
        }
    }
}

/// Питание от сети? `Err` — статус неизвестен (255) или вызов завершился ошибкой.
pub fn ac_power_online() -> Result<bool, PowerError> {
    #[cfg(windows)]
    {
        // SYSTEM_POWER_STATUS не реализует Default в windows-sys.
        let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetSystemPowerStatus(&mut status) };
        if ok == 0 {
            return Err(PowerError::AcStatusUnknown);
        }
        match status.ACLineStatus {
            1 => Ok(true),
            0 => Ok(false),
            _ => Err(PowerError::AcStatusUnknown),
        }
    }
    #[cfg(not(windows))]
    {
        Ok(true)
    }
}

/// Починить строку, испорченную однобайтовым декодером.
///
/// Старый код читал UTF-8 вывод `powercfg` как CP866. Ошибка обратима: если
/// обратная перекодировка в OEM даёт валидный UTF-8 с кириллицей, значит
/// перед нами именно такая порча. Нужно для истории, записанной до исправления.
///
/// Возвращает `None`, если строка не похожа на испорченную.
pub fn repair_mojibake(text: &str) -> Option<String> {
    // Признак порчи — символы псевдографики CP866, попавшие на место букв.
    if !text
        .chars()
        .any(|c| ('\u{2550}'..='\u{259F}').contains(&c))
    {
        return None;
    }
    let bytes = encode_oem(text);
    let fixed = String::from_utf8(bytes).ok()?;
    // Починено: кириллица появилась, псевдографики не осталось.
    let has_cyrillic = fixed
        .chars()
        .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё' || c == 'Ё');
    let still_box = fixed
        .chars()
        .any(|c| ('\u{2550}'..='\u{259F}').contains(&c));
    if has_cyrillic && !still_box {
        Some(fixed)
    } else {
        None
    }
}

/// Закодировать строку в OEM-кодировку текущей системы.
fn encode_oem(text: &str) -> Vec<u8> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::WideCharToMultiByte;
        const CP_OEMCP: u32 = 1;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        if utf16.is_empty() {
            return Vec::new();
        }
        let needed = unsafe {
            WideCharToMultiByte(
                CP_OEMCP,
                0,
                utf16.as_ptr(),
                utf16.len() as i32,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if needed <= 0 {
            return text.as_bytes().to_vec();
        }
        let mut buf = vec![0u8; needed as usize];
        let written = unsafe {
            WideCharToMultiByte(
                CP_OEMCP,
                0,
                utf16.as_ptr(),
                utf16.len() as i32,
                buf.as_mut_ptr(),
                needed,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if written <= 0 {
            return text.as_bytes().to_vec();
        }
        buf.truncate(written as usize);
        buf
    }
    #[cfg(not(windows))]
    {
        text.as_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_identifier_falls_back_to_architecture() {
        let id = cpu_identifier();
        assert!(!id.trim().is_empty());
    }

    #[test]
    fn oem_decoding_is_stable_for_ascii() {
        // ASCII-вывод в любом кодовом стиле даёт ту же строку.
        let decoded = decode_oem(b"GUID scheme: abc");
        assert_eq!(decoded, "GUID scheme: abc");
    }

    /// Регресс: `powercfg` на этой машине отдаёт UTF-8, и раньше вывод всегда
    /// читался как CP866 — кириллические имена схем превращались в «╨Ь╨░╨║».
    #[test]
    fn utf8_output_survives_decoding() {
        let src = "Максимальная производительность";
        let bytes = src.as_bytes();
        assert!(std::str::from_utf8(bytes).is_ok());
        assert_eq!(decode_oem(bytes), src);
    }

    /// ASCII-путь не должен ломаться, даже если весь вывод валидный UTF-8.
    #[test]
    fn mixed_ascii_and_cyrillic_is_decoded_as_utf8() {
        let src = "GUID схемы питания: 07d147ca-d013-40ae-80ee-ae3c98650195";
        assert_eq!(decode_oem(src.as_bytes()), src);
    }

    /// Уже испорченные имена в истории должны чиниться: «╨Ь╨░╨║╤Б╨╕╨╝» —
    /// это UTF-8, прочитанный как CP866, и операция обратима.
    #[test]
    fn corrupted_cyrillic_is_repairable() {
        let src = "Высокая производительность";
        // Воспроизводим ровно то, что делал старый декодер.
        let wrong = decode_oem_strict(src.as_bytes());
        assert_ne!(wrong, src, "имитация порчи не сработала");
        assert_eq!(repair_mojibake(&wrong).as_deref(), Some(src));
    }

    /// Текст без порчи чинить нечего.
    #[test]
    fn valid_text_is_left_alone() {
        assert_eq!(repair_mojibake("Обычная схема"), None);
        assert_eq!(repair_mojibake("Velo's Power Plan"), None);
        assert_eq!(repair_mojibake(""), None);
    }

    #[test]
    fn diagnostics_version_is_fixed() {
        assert_eq!(diagnostics_version(), "0.1.0");
    }
}
