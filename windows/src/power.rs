//! Низкоуровневые утилиты Windows: права администратора, запрет сна,
//! питание от сети, идентификатор CPU, частота таймера QPC, перекодирование
//! OEM-вывода в Unicode.

#[cfg(windows)]
use windows_sys::Win32::Globalization::MultiByteToWideChar;
#[cfg(windows)]
use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;
#[cfg(windows)]
use windows_sys::Win32::System::Power::{
    GetSystemPowerStatus, SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED,
    ES_SYSTEM_REQUIRED, EXECUTION_STATE, SYSTEM_POWER_STATUS,
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
        if freq > 0 {
            freq as u64
        } else {
            10_000_000
        }
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
pub fn is_admin() -> bool {
    #[cfg(windows)]
    {
        unsafe { IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Перекодировать байты OEM-консольного вывода локали в Unicode.
/// Несовместимые байты заменяются U+FFFD (как Python errors='replace').
pub fn decode_oem(bytes: &[u8]) -> String {
    #[cfg(windows)]
    {
        if bytes.is_empty() {
            return String::new();
        }
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
#[derive(Debug)]
pub struct SleepGuard {
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
            Ok(Self { active: true })
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
                unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
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

    #[test]
    fn diagnostics_version_is_fixed() {
        assert_eq!(diagnostics_version(), "0.1.0");
    }
}