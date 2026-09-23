//! Приоритет процесса: поднятие до HIGH на время измерений с гарантией
//! восстановления исходного класса при разрушении (в т.ч. при ошибке/отмене).

#[cfg(windows)]
use windows_sys::Win32::System::Threading::HIGH_PRIORITY_CLASS;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetPriorityClass, SetPriorityClass};

/// RAII-поднятие приоритета текущего процесса. Пока жив, процесс измерений
/// работает в классе HIGH; при разрушении восстанавливается исходный класс.
pub struct Guard {
    #[cfg(windows)]
    previous_class: u32,
}

impl Guard {
    /// Поднять приоритет текущего процесса до HIGH. На не-Windows платформах —
    /// no-op. Ошибка установки не фатальна: измерения продолжаются в исходном
    /// классе.
    #[must_use]
    pub fn raise() -> Self {
        #[cfg(windows)]
        {
            let process = unsafe { GetCurrentProcess() };
            let previous_class = unsafe { GetPriorityClass(process) };
            let _ = unsafe { SetPriorityClass(process, HIGH_PRIORITY_CLASS) };
            Self { previous_class }
        }
        #[cfg(not(windows))]
        {
            Self {}
        }
    }

    fn restore(&mut self) {
        #[cfg(windows)]
        {
            if self.previous_class != 0 {
                let class = self.previous_class;
                self.previous_class = 0;
                let _ = unsafe { SetPriorityClass(GetCurrentProcess(), class) };
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.restore();
    }
}