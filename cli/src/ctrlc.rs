//! Мягкое прерывание (Ctrl+C): вместо завершения процесса выставляется флаг,
//! который сессия опрашивает между фазами/прогонами и передаёт сторожевому
//! таймеру для остановки текущей фазы. План и контрольная точка сохраняются,
//! исходная схема восстанавливается.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// Единственный разделяемый флаг остановки (все вызовы возвращают тот же Arc).
fn flag() -> &'static Arc<AtomicBool> {
    static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    STOP.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// Разделяемый флаг остановки для сессии/сторожевого таймера.
pub fn stop() -> Arc<AtomicBool> {
    flag().clone()
}

/// Установить обработчик Ctrl+C (несколько вызовов безопасны).
pub fn install() {
    #[cfg(windows)]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            unsafe extern "system" fn handler(_: u32) -> i32 {
                flag().store(true, Ordering::Relaxed);
                1
            }
            unsafe {
                let _ =
                    windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(handler), 1);
            }
        });
    }
    #[cfg(not(windows))]
    {
        // На не-Windows флаг остаётся пустым; тесты работают без обработчика.
    }
}
