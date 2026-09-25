//! Свободное место и объём тома (для проверки готовности перед сессией
//! и индикации в интерфейсе).

/// Свободное место на диске, содержащем `path`, в байтах.
///
/// Реализовано через `GetDiskFreeSpaceExW` (сообщает о доступном пользователю
/// объёме — именно то, что может записать процесс, а не физический остаток).
///
/// # Errors
/// Returns an error string if the Windows API call fails.
pub fn free_space_bytes(path: &std::path::Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_to_user: u64 = 0;
        let mut _total_unused: u64 = 0;
        let mut _free_total_unused: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &raw mut free_to_user,
                &raw mut _total_unused,
                &raw mut _free_total_unused,
            )
        };
        if ok == 0 {
            return Err(format!(
                "GetDiskFreeSpaceExW для «{}»: ошибка {}",
                path.display(),
                unsafe { GetLastError() }
            ));
        }
        Ok(free_to_user)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("свободное место диска доступно только на Windows".to_string())
    }
}

/// Общий объём тома (всего байт), содержащего `path`.
///
/// # Errors
/// Returns an error string if the Windows API call fails.
pub fn volume_bytes(path: &std::path::Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut _free_to_user_unused: u64 = 0;
        let mut total: u64 = 0;
        let mut _free_total_unused: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &raw mut _free_to_user_unused,
                &raw mut total,
                &raw mut _free_total_unused,
            )
        };
        if ok == 0 {
            return Err(format!(
                "GetDiskFreeSpaceExW для «{}» (объём тома): ошибка {}",
                path.display(),
                unsafe { GetLastError() }
            ));
        }
        Ok(total)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("объём тома диска доступен только на Windows".to_string())
    }
}

/// Человекочитаемое представление байтов (для сообщений диагностики).
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} ГБ", bytes as f64 / f64::from(1 << 30))
    } else if bytes >= 1 << 20 {
        format!("{:.1} МБ", bytes as f64 / f64::from(1 << 20))
    } else if bytes >= 1 << 10 {
        format!("{:.1} КБ", bytes as f64 / f64::from(1 << 10))
    } else {
        format!("{bytes} Б")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_formatter_human_readable() {
        assert_eq!(format_bytes(0), "0 Б");
        assert_eq!(format_bytes(1024), "1.0 КБ");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 МБ");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 ГБ");
    }

    #[test]
    #[cfg(windows)]
    fn temp_dir_has_positive_free_space() {
        let free = free_space_bytes(&std::env::temp_dir()).expect("свободное место");
        assert!(free > 0);
        let total = volume_bytes(&std::env::temp_dir()).expect("объём тома");
        assert!(total > 0);
        assert!(total >= free);
    }
}
