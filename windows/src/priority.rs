//! Приоритет процесса: поднятие до HIGH на время измерений с гарантией
//! восстановления исходного класса при разрушении (в т.ч. при ошибке/отмене).
//!
//! Зачем это нужно. Приоритет поднимается **на время измеряемой фазы**, а не
//! на всё время сессии: фаза — единственный момент, когда важно, чтобы потоки
//! бенчмарка вытесняли постороннюю работу. Между фазами (разогрев, стабилизация,
//! ожидание) процесс возвращается к обычному классу, чтобы не мешать ни
//! интерфейсу, ни фоновой службе.
//!
//! Класс `HIGH_PRIORITY_CLASS` выбран сознательно, а не
//! `REALTIME_PRIORITY_CLASS`: режим реального времени вытесняет даже
//! DPC-обработчики и сам становится источником шума — а шум в хвостовых
//! метриках здесь опаснее, чем конкуренция с фоновым процессом.
//!
//! Приоритет **процесса**, а не потока: так приоритет получают и главный поток
//! тика, и все воркеры пула сразу, без обхода. Маску ядер этому не мешает —
//! привязка и приоритет независимы.

#[cfg(windows)]
use windows_sys::Win32::System::Threading::HIGH_PRIORITY_CLASS;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetPriorityClass, SetPriorityClass,
};

/// Исходный класс не удалось прочитать (`GetPriorityClass` вернул 0).
///
/// Восстанавливать нечего, и молча оставлять процесс в HIGH нельзя: это
/// изменило бы поведение приложения после сессии. Поэтому такое состояние
/// считается «приоритет не поднят».
const PRIORITY_UNKNOWN: u32 = 0;

/// RAII-поднятие приоритета текущего процесса. Пока жив, процесс измерений
/// работает в классе HIGH; при разрушении восстанавливается исходный класс.
#[derive(Debug)]
pub struct Guard {
    #[cfg(windows)]
    previous_class: u32,
    /// Приоритет действительно поднят (ложь — ОС отказала или исходный класс
    /// не удалось прочитать и потому нечего восстанавливать).
    raised: bool,
}

impl Guard {
    /// Поднять приоритет текущего процесса до HIGH. На не-Windows платформах —
    /// no-op. Ошибка установки не фатальна: измерения продолжаются в исходном
    /// классе, но вызывающий может узнать об этом через [`Guard::raised`] и
    /// сказать об этом в журнале.
    #[must_use]
    pub fn raise() -> Self {
        #[cfg(windows)]
        {
            let process = unsafe { GetCurrentProcess() };
            let previous_class = unsafe { GetPriorityClass(process) };
            if previous_class == PRIORITY_UNKNOWN {
                return Self {
                    previous_class,
                    raised: false,
                };
            }
            let ok = unsafe { SetPriorityClass(process, HIGH_PRIORITY_CLASS) } != 0;
            Self {
                previous_class,
                raised: ok,
            }
        }
        #[cfg(not(windows))]
        {
            Self { raised: false }
        }
    }

    /// Поднят ли приоритет на самом деле.
    pub fn raised(&self) -> bool {
        self.raised
    }

    /// Причина, по которой приоритет не поднят (пусто — поднят).
    pub fn not_raised_reason(&self) -> Option<String> {
        if self.raised {
            return None;
        }
        #[cfg(windows)]
        {
            if self.previous_class == PRIORITY_UNKNOWN {
                return Some("не удалось прочитать исходный класс приоритета".to_string());
            }
            Some(format!(
                "ОС отказала в HIGH_PRIORITY_CLASS (исходный класс {:#x})",
                self.previous_class
            ))
        }
        #[cfg(not(windows))]
        {
            Some("приоритет процесса недоступен на этой платформе".to_string())
        }
    }

    fn restore(&mut self) {
        #[cfg(windows)]
        {
            // Восстанавливаем только если действительно поднимали: иначе
            // «восстановление» записало бы в процесс класс, которого там
            // никогда не было.
            if self.raised && self.previous_class != PRIORITY_UNKNOWN {
                let class = self.previous_class;
                self.previous_class = PRIORITY_UNKNOWN;
                let _ = unsafe { SetPriorityClass(GetCurrentProcess(), class) };
            }
            self.raised = false;
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetPriorityClass, HIGH_PRIORITY_CLASS,
    };

    /// Guard обязан честно сообщать, поднялся ли приоритет, и восстановить
    /// исходный класс при разрушении — иначе сессия оставила бы процесс в HIGH
    /// после себя, и все последующие программы пользователя работали бы с
    /// изменённым приоритетом.
    #[test]
    fn guard_reports_and_restores() {
        let before = unsafe { GetPriorityClass(GetCurrentProcess()) };
        {
            let guard = Guard::raise();
            // Соответствие между «поднят» и причиной обязано быть полным:
            // либо поднят, либо сказано почему нет.
            assert_eq!(
                guard.raised(),
                guard.not_raised_reason().is_none(),
                "raised={} reason={:?}",
                guard.raised(),
                guard.not_raised_reason()
            );
            if guard.raised() {
                assert_eq!(
                    unsafe { GetPriorityClass(GetCurrentProcess()) },
                    HIGH_PRIORITY_CLASS
                );
            }
        }
        assert_eq!(
            unsafe { GetPriorityClass(GetCurrentProcess()) },
            before,
            "после разрушения guard'а приоритет обязан вернуться к исходному"
        );
    }

    /// Вложенные guard'ы не должны терять исходный класс: внешний снимет
    /// HIGH раньше внутреннего, и второй по счёту Drop обязан это учесть.
    #[test]
    fn nested_guards_do_not_corrupt_priority() {
        let before = unsafe { GetPriorityClass(GetCurrentProcess()) };
        {
            let _outer = Guard::raise();
            let _inner = Guard::raise();
        }
        assert_eq!(
            unsafe { GetPriorityClass(GetCurrentProcess()) },
            before,
            "вложенные guard'ы оставили процесс в изменённом приоритете"
        );
    }

    /// Приоритет не удалось поднять — причина обязана быть, а не пустота.
    #[test]
    fn failure_is_explicit() {
        let guard = Guard::raise();
        if let Some(reason) = guard.not_raised_reason() {
            assert!(!reason.trim().is_empty());
        }
    }
}
