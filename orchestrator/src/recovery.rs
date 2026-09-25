//! Восстановление прерванной сессии (Этап 8, надёжность): если приложение
//! закрылось/упало во время замера, на диске остаётся контрольная точка с
//! зафиксированной исходной схемой (`original_scheme_guid`, `original_restored =
//! false`). При следующем запуске (и перед стартом нового теста) такая схема
//! восстанавливается автоматически — тестовая схема не остаётся активной.

use crate::checkpoint::{Checkpoint, load_checkpoint, save_checkpoint};
use crate::session::SchemeDriver;

/// Итог попытки восстановления после прерывания.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryOutcome {
    /// На диске была контрольная точка прерванной сессии.
    pub interrupted_checkpoint: bool,
    /// Исходная схема требовала возврата (вообще).
    pub needed_restore: bool,
    /// Исходная схема активна/стала активна к моменту выхода.
    pub restored: bool,
    /// Всё в порядке с первого взгляда (не было что восстанавливать).
    pub already_ok: bool,
    /// Нефатальные сообщения (например, схема восстановлена, но чекпоинт
    /// не перезаписан; либо недостаточно прав).
    pub error: Option<String>,
}

impl RecoveryOutcome {
    fn nothing() -> Self {
        Self {
            interrupted_checkpoint: false,
            needed_restore: false,
            restored: true,
            already_ok: true,
            error: None,
        }
    }

    fn after_restore(checkpoint: &mut Checkpoint, error: Option<String>) -> Self {
        checkpoint.original_restored = true;
        let save_result = save_checkpoint(checkpoint);
        let mut outcome = RecoveryOutcome {
            interrupted_checkpoint: true,
            needed_restore: true,
            restored: true,
            already_ok: save_result.is_ok(),
            error,
        };
        if let Err(cause) = save_result {
            outcome.error = Some(match outcome.error {
                Some(e) => format!("{e} (чекпоинт не обновлён: {cause})"),
                None => format!("исходная схема восстановлена; чекпоинт не обновлён: {cause}"),
            });
        }
        outcome
    }
}

/// Восстановить исходную схему по контрольной точке прерванной сессии.
///
/// Требует прав администратора для `set_active`; если активация не удалась,
/// чекпоинт НЕ помечается восстановленным — следующий запуск повторит попытку.
pub fn recover_interrupted_session(driver: &dyn SchemeDriver) -> RecoveryOutcome {
    let Some(mut checkpoint) = load_checkpoint() else {
        return RecoveryOutcome::nothing();
    };

    if checkpoint.original_restored {
        return RecoveryOutcome {
            interrupted_checkpoint: true,
            needed_restore: false,
            restored: true,
            already_ok: true,
            error: None,
        };
    }

    let Some(original) = checkpoint.original_scheme_guid.clone() else {
        // Исходная схема не зафиксирована (прерывание случилось до первого
        // прогона) — активная схема и есть исходная.
        return RecoveryOutcome {
            interrupted_checkpoint: true,
            needed_restore: false,
            restored: true,
            already_ok: true,
            error: None,
        };
    };

    let schemes = match driver.list_schemes() {
        Ok(list) => list,
        Err(cause) => {
            return RecoveryOutcome {
                interrupted_checkpoint: true,
                needed_restore: true,
                restored: false,
                already_ok: false,
                error: Some(format!("не удалось получить список схем: {cause:?}")),
            };
        }
    };

    let active = schemes.iter().find(|s| s.active).map(|s| s.guid.clone());
    if active.as_deref() == Some(original.as_str()) {
        // Уже активна исходная: помечаем восстановленной и сохраняем.
        return RecoveryOutcome::after_restore(&mut checkpoint, None);
    }

    // Исходная схема должна существовать в списке (защита от битой CT).
    let name = schemes
        .iter()
        .find(|s| s.guid.eq_ignore_ascii_case(&original))
        .map(|s| s.name.clone());

    if let Err(cause) = driver.set_active(&original) {
        let label = name
            .as_deref()
            .map(|n| format!(" «{n}»"))
            .unwrap_or_default();
        return RecoveryOutcome {
            interrupted_checkpoint: true,
            needed_restore: true,
            restored: false,
            already_ok: false,
            error: Some(format!(
                "не удалось восстановить исходную схему{label}: {cause:?}"
            )),
        };
    }

    RecoveryOutcome::after_restore(&mut checkpoint, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{checkpoint_path, save_checkpoint};
    use crate::config::SessionConfig;
    use powerbench_windows::powercfg::PowerScheme;

    /// Склеивает recovery-тесты в одну цепочку: они работают с общим
    /// (глобальным) файлом контрольной точки.
    static RECOVERY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_checkpoint() {
        let _ = std::fs::remove_file(checkpoint_path());
    }

    /// Минимальный драйвер для тестов восстановления.
    struct FakeDriver {
        schemes: Vec<String>,
        active: std::cell::RefCell<String>,
        fail_restore: std::cell::RefCell<Option<String>>,
    }

    impl FakeDriver {
        fn new(schemes: &[&str], active: &str) -> Self {
            Self {
                schemes: schemes.iter().map(|s| s.to_string()).collect(),
                active: std::cell::RefCell::new(active.to_string()),
                fail_restore: std::cell::RefCell::new(None),
            }
        }

        fn active(&self) -> String {
            self.active.borrow().clone()
        }
    }

    impl crate::session::SchemeDriver for FakeDriver {
        fn list_schemes(&self) -> Result<Vec<PowerScheme>, String> {
            let active = self.active.borrow().clone();
            Ok(self
                .schemes
                .iter()
                .map(|g| PowerScheme {
                    guid: g.clone(),
                    name: g.clone(),
                    active: *g == active,
                })
                .collect())
        }

        fn set_active(&self, guid: &str) -> Result<(), String> {
            if self.fail_restore.borrow().as_deref() == Some(guid) {
                return Err("fail_restore".to_string());
            }
            *self.active.borrow_mut() = guid.to_string();
            Ok(())
        }

        fn ac_power_online(&self) -> Result<bool, String> {
            Ok(true)
        }

        fn is_admin(&self) -> bool {
            true
        }
    }

    fn plan() -> SessionConfig {
        SessionConfig {
            duration_seconds: 9,
            warmup_seconds: 2,
            cooling_seconds: 0,
            repetitions: 1,
            background_threshold_percent: 5.0,
            worker_count: None,
            scheme_ids: vec!["test-a".to_string()],
            plan_guid: "recovery-plan".to_string(),
        }
    }

    fn checkpoint_with(active: Option<&str>, restored: bool) -> crate::checkpoint::Checkpoint {
        let mut cp = crate::checkpoint::Checkpoint::new(plan());
        cp.original_scheme_guid = active.map(|s| s.to_string());
        cp.original_restored = restored;
        cp
    }

    fn with_clean_checkpoint<T>(f: impl FnOnce() -> T) -> T {
        let _guard = RECOVERY_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_checkpoint();
        let out = f();
        clear_checkpoint();
        out
    }

    /// Драйвер, где исходной (до тестовой сессии) считается схема `orig`.
    fn recovery_driver(active: &str) -> FakeDriver {
        FakeDriver::new(&["test-a", "orig"], active)
    }

    #[test]
    fn no_checkpoint_is_ok() {
        with_clean_checkpoint(|| {
            let outcome = recover_interrupted_session(&recovery_driver("test-a"));
            assert!(!outcome.interrupted_checkpoint);
            assert!(outcome.already_ok);
            assert!(outcome.restored);
        });
    }

    #[test]
    fn already_restored_is_left_as_is() {
        with_clean_checkpoint(|| {
            save_checkpoint(&checkpoint_with(Some("orig"), true)).unwrap();
            let outcome = recover_interrupted_session(&recovery_driver("test-a"));
            assert!(outcome.interrupted_checkpoint);
            assert!(outcome.restored);
            assert!(outcome.already_ok);
        });
    }

    #[test]
    fn restores_original_when_test_scheme_is_active() {
        with_clean_checkpoint(|| {
            save_checkpoint(&checkpoint_with(Some("orig"), false)).unwrap();
            let driver = recovery_driver("test-a");
            let outcome = recover_interrupted_session(&driver);
            assert!(outcome.interrupted_checkpoint);
            assert!(outcome.needed_restore);
            assert!(outcome.restored);
            assert!(outcome.already_ok);
            // Активная схема снова исходная.
            assert_eq!(driver.active(), "orig");
            // Чекпоинт помечен восстановленным.
            let cp = load_checkpoint().unwrap();
            assert!(cp.original_restored);
        });
    }

    #[test]
    fn leaves_marker_when_restore_fails() {
        with_clean_checkpoint(|| {
            save_checkpoint(&checkpoint_with(Some("orig"), false)).unwrap();
            let driver = recovery_driver("test-a");
            *driver.fail_restore.borrow_mut() = Some("orig".to_string());
            let outcome = recover_interrupted_session(&driver);
            assert!(outcome.interrupted_checkpoint);
            assert!(outcome.needed_restore);
            assert!(!outcome.restored);
            assert!(outcome.error.is_some(), "ошибка должна сообщить причину");
            // Чекпоинт НЕ помечен — следующий запуск повторит попытку.
            let cp = load_checkpoint().unwrap();
            assert!(!cp.original_restored);
        });
    }

    #[test]
    fn missing_original_guid_is_safe() {
        with_clean_checkpoint(|| {
            save_checkpoint(&checkpoint_with(None, false)).unwrap();
            let outcome = recover_interrupted_session(&recovery_driver("test-a"));
            assert!(outcome.interrupted_checkpoint);
            assert!(outcome.restored);
            assert!(outcome.already_ok);
        });
    }
}
