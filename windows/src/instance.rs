//! Один экземпляр приложения на пользователя.
//!
//! Второй экземпляр PowerBench вреден не только тем, что открывает второе
//! окно. Оба процесса работают с одними и теми же файлами состояния
//! (контрольная точка, маркер прогона, карантин) и, главное, переключают
//! общую схему питания Windows. Второй может применить свою схему поверх
//! первой прямо посреди замера, и результат окажется мусором, причём
//! приписать его будет не к чему.
//!
//! Механизм — именованный мьютекс: право на имя есть только у одного процесса
//! в сеансе пользователя, и оно освобождается автоматически, даже если
//! процесс убит. Плагин `tauri-plugin-single-instance` тут недоступен (сборка
//! идёт без сети), а своя реализация на ~40 строк избавляет от лишней
//! зависимости.
//!
//! Чего мьютекс не делает: он не умеет поднять окно уже запущенного
//! приложения. Для этого нужен канал IPC между процессами, поэтому второй
//! запуск просто объясняет пользователю, что приложение уже открыто.

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};

/// Имя мьютекса. `Local\` — только в текущем сеансе пользователя, что и нужно:
/// два пользователя за одним компьютером должны мерить независимо.
const MUTEX_NAME: &str = r"Local\PowerBenchSingleInstance";

/// Право владения мьютексом держится, пока жив guard. `Drop` закрывает
/// дескриптор, иначе процесс держал бы именованный объект до конца сессии.
pub struct SingleInstance {
    handle: HANDLE,
}

// Дескриптор единственный и живёт только на одном потоке, но guard по
// устройству `Send`: он передаётся в `main` и остаётся там же.
unsafe impl Send for SingleInstance {}

impl SingleInstance {
    /// Попытаться стать единственным экземпляром.
    ///
    /// `None` — приложение уже запущено и запускать вторую копию нельзя.
    pub fn acquire() -> Option<Self> {
        unsafe {
            let name: Vec<u16> = MUTEX_NAME
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            // `bInitialOwner = 0`: право выдаётся через WaitForSingleObject, а
            // не само по себе созданию, поэтому ложное срабатывание при
            // повторном открытии не блокирует первый процесс.
            let handle = windows_sys::Win32::System::Threading::CreateMutexW(
                std::ptr::null(),
                0,
                name.as_ptr(),
            );
            if handle.is_null() {
                // Не смогли даже создать мьютекс. Лучше рискнуть вторым
                // экземпляром, чем не запустить приложение вовсе.
                return Some(SingleInstance {
                    handle: std::ptr::null_mut(),
                });
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                return None;
            }
            Some(SingleInstance { handle })
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

/// Показать пользователю, что приложение уже открыто.
///
/// Приложение собрано как `windows_subsystem = "windows"`, поэтому сообщение в
/// stderr пользователю не видно: нужен диалог.
pub fn show_already_running() {
    unsafe {
        let text: Vec<u16> = "PowerBench уже запущен.\nОткройте значок в трее —\
                               окно могло быть свёрнуто."
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let caption: Vec<u16> = "PowerBench"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_TOPMOST,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Два одновременных guard'а невозможны: право на имя одно.
    ///
    /// `CreateMutexW` с уже занятым именем возвращает тот же объект и ставит
    /// `ERROR_ALREADY_EXISTS`, поэтому второй зах обязан быть отказом. Если
    /// `PowerBench` уже запущен на этой машине, откажет и первый — тогда
    /// проверяется именно это.
    #[test]
    fn two_guards_cannot_coexist() {
        let first = SingleInstance::acquire();
        let second = SingleInstance::acquire();
        assert!(
            !(first.is_some() && second.is_some()),
            "имя мьютекса выдали дважды: запущены два экземпляра приложения"
        );
    }
}
