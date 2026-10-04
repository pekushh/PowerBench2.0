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
use windows_sys::Win32::Foundation::GetLastError;
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
    /// Исходный класс уже возвращён.
    ///
    /// Флаг нужен, чтобы [`Guard::not_raised_reason`] после восстановления не
    /// врала: без него «класс не удалось прочитать» выдавалось бы как причину
    /// неудачного подъёма, хотя подъём был и успешно отменён.
    restored: bool,
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
                    restored: false,
                };
            }
            let ok = unsafe { SetPriorityClass(process, HIGH_PRIORITY_CLASS) } != 0;
            Self {
                previous_class,
                raised: ok,
                restored: false,
            }
        }
        #[cfg(not(windows))]
        {
            Self {
                raised: false,
                restored: false,
            }
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
        if self.restored {
            return Some("приоритет уже восстановлен".to_string());
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
                // Сбрасываем ДО повторной попытки: `Drop` не должен
                // восстанавливать дважды, а паника из FFI здесь исключена —
                // значит, повтор можно сделать без риска двойного вызова.
                self.previous_class = PRIORITY_UNKNOWN;
                self.raised = false;
                self.restored = true;
                // Регресс H19/H20: возврат класса молча игнорировался
                // (`let _ =`). Если ОС отказала, процесс остаётся в HIGH до
                // конца жизни приложения — то есть все последующие программы
                // пользователя работают с изменённым приоритетом, и он об этом
                // не знает.
                //
                // Поэтому одна попытка плюс повтор: снять приоритет обычно
                // удаётся, а отказ чаще связан с кратковременным состоянием.
                if unsafe { SetPriorityClass(GetCurrentProcess(), class) } == 0 {
                    let err = unsafe { GetLastError() };
                    eprintln!(
                        "PowerBench: не удалось вернуть исходный класс приоритета \
                         ({class:#x}, код {err}) — процесс остаётся в \
                         HIGH_PRIORITY_CLASS до перезапуска приложения"
                    );
                    // Вторая попытка: снятие приоритета почти всегда проходит,
                    // а цена молчания здесь — изменённое поведение всей системы
                    // для пользователя.
                    if unsafe { SetPriorityClass(GetCurrentProcess(), class) } != 0 {
                        eprintln!(
                            "PowerBench: исходный класс приоритета возвращён со второй попытки"
                        );
                    }
                }
                return;
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

    /// Сериализация тестов: класс приоритета — состояние ПРОЦЕССА, общее для
    /// всех тестов этого крейта.
    ///
    /// Без мьютекса тесты идут параллельно, и чужой `Drop` возвращает класс
    /// прямо в середине чужой проверки: `guard_reports_and_restores` ловил
    /// `NORMAL_PRIORITY_CLASS` вместо `HIGH` на собственном же поднятом
    /// процессе. Провал выглядел как ошибка в `Guard`, хотя виноваты были
    /// параллельные тесты, — а такие «плавающие» падения в итоге и портят
    /// доверие к проверкам RAII.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Guard обязан честно сообщать, поднялся ли приоритет, и восстановить
    /// исходный класс при разрушении — иначе сессия оставила бы процесс в HIGH
    /// после себя, и все последующие программы пользователя работали бы с
    /// изменённым приоритетом.
    #[test]
    fn guard_reports_and_restores() {
        let _g = lock();
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
        let _g = lock();
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
        let _g = lock();
        let guard = Guard::raise();
        if let Some(reason) = guard.not_raised_reason() {
            assert!(!reason.trim().is_empty());
        }
    }

    /// Регресс H19/H20: отказ при возврате класса не может быть молчаливым.
    ///
    /// Раньше результат `SetPriorityClass` при восстановлении отбрасывался через
    /// `let _ =`. Если ОС отказала, процесс навсегда оставался в
    /// `HIGH_PRIORITY_CLASS`, и все программы, запущенные после PowerBench,
    /// работали с изменённым приоритетом — без единого сообщения.
    ///
    /// Проверяем контракт: ошибка видна пользователю и есть повторная попытка.
    #[test]
    fn restore_failure_is_never_silently_ignored() {
        let src = include_str!("priority.rs");
        let start = src
            .find("fn restore(&mut self)")
            .expect("не найден метод restore");
        let end = src[start..]
            .find("\nimpl Drop")
            .map(|i| start + i)
            .expect("не найден конец метода restore");
        let body = &src[start..end];

        assert!(
            !body.contains("let _ = SetPriorityClass"),
            "результат SetPriorityClass при восстановлении снова отбрасывается: {body}"
        );
        assert!(
            body.contains("GetLastError"),
            "код ошибки не читается — сообщение не объяснит, что произошло: {body}"
        );
        // Сообщение обязано называть и оставшийся приоритет, и последствие.
        assert!(
            body.contains("не удалось вернуть исходный класс приоритета"),
            "пользователь не узнает, что приоритет не вернулся: {body}"
        );
        assert!(
            body.contains("до перезапуска приложения"),
            "не сказано, что процесс остаётся в HIGH до перезапуска: {body}"
        );
        // И причина обязана быть видна на stderr, а не только в коде.
        assert!(
            body.contains("eprintln!"),
            "причина отказа не выводится пользователю: {body}"
        );
        // Повторная попытка: снятие приоритета почти всегда проходит со второй.
        let attempts = body.matches("SetPriorityClass").count();
        assert!(
            attempts >= 2,
            "повторной попытки возврата приоритета нет (вызовов {attempts}): {body}"
        );
        assert!(
            body.contains("со второй попытки"),
            "успех повторной попытки не сообщается: {body}"
        );
    }

    /// `restore` обязан быть идемпотентным: он сбрасывает состояние, и повторный
    /// вызов (в том числе из `Drop`) не должен ни писать класс заново, ни врать
    /// о причине.
    #[test]
    fn restore_is_idempotent_and_stops_claiming_a_failure() {
        let _g = lock();
        let before = unsafe { GetPriorityClass(GetCurrentProcess()) };
        let mut guard = Guard::raise();
        assert!(guard.raised(), "подготовка: приоритет не поднят");
        guard.restore();

        assert!(
            !guard.raised(),
            "после restore guard продолжает считать себя поднятым"
        );
        assert_eq!(
            guard.not_raised_reason().as_deref(),
            Some("приоритет уже восстановлен"),
            "после restore причина не должна утверждать, что подъём не удался"
        );
        assert_eq!(
            unsafe { GetPriorityClass(GetCurrentProcess()) },
            before,
            "класс процесса не восстановлен"
        );

        // Повторный restore обязан быть безопасным и не менять класс.
        guard.restore();
        assert_eq!(
            unsafe { GetPriorityClass(GetCurrentProcess()) },
            before,
            "повторный restore изменил класс процесса"
        );
        assert_eq!(
            guard.not_raised_reason().as_deref(),
            Some("приоритет уже восстановлен"),
            "повторный restore сбросил признак восстановления"
        );
    }

    /// Guard, который не поднял приоритет, не должен при разрушении ничего
    /// восстанавливать: класс процесса обязан остаться исходным.
    #[test]
    fn a_guard_that_did_not_raise_restores_nothing() {
        let _g = lock();
        let before = unsafe { GetPriorityClass(GetCurrentProcess()) };
        let mut guard = Guard::raise();
        if guard.raised() {
            // Подъём удался — снимаем его, чтобы получить «не поднят, но класс
            // известен»: так мы проверяем ветку восстановления при known-классе.
            guard.restore();
        }
        let reason = guard.not_raised_reason();
        assert!(
            reason.is_some(),
            "у неподнятого guard'а обязана быть причина"
        );
        // Причина обязана быть осмысленной, а не пустой строкой.
        assert!(!reason.unwrap().trim().is_empty());
        assert_eq!(
            unsafe { GetPriorityClass(GetCurrentProcess()) },
            before,
            "неподнятый guard изменил класс процесса"
        );
    }
}
