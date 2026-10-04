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
    // Маркер незавершённого прогона: прошлый запуск не записал ни результат,
    // ни браковку — процесс убит, ПК завис или был ребут прямо во время замера.
    // Виновная схема уходит в карантин, маркер стирается.
    let freeze_note = match crate::quarantine::load_testing_marker() {
        Some(marker) => {
            let reason =
                "прогон не завершился (убийство процесса, зависание ПК или ребут во время замера)";
            let _ = crate::quarantine::quarantine_add(
                &marker.scheme_id,
                marker.scheme_name.as_deref(),
                crate::quarantine::QuarantineKind::HardFreeze,
                reason,
                &marker.plan_guid,
            );
            crate::quarantine::clear_testing_marker();
            Some(format!(
                "схема «{}» отправлена в карантин ({}); верните её вручную, если зависание было случайным",
                marker.scheme_id, reason
            ))
        }
        None => None,
    };

    let loaded = load_checkpoint();
    let mut checkpoint = match loaded {
        Ok(Some(cp)) => cp,
        // Битая контрольная точка — это проблема, а не «сессии не было»:
        // молча продолжить с нуля значит потерять раунды и не сказать об этом.
        Err(e) => {
            return RecoveryOutcome {
                interrupted_checkpoint: false,
                needed_restore: false,
                restored: false,
                already_ok: false,
                error: Some(format!(
                    "контрольная точка не читается ({e}); сохраните файл и удалите его, чтобы начать заново"
                )),
            };
        }
        Ok(None) => {
            // Контрольной точки нет, но схема могла остаться переключённой.
            return match freeze_note {
                Some(note) => RecoveryOutcome {
                    interrupted_checkpoint: false,
                    needed_restore: false,
                    restored: true,
                    already_ok: false,
                    error: Some(note),
                },
                None => RecoveryOutcome::nothing(),
            };
        }
    };

    if checkpoint.original_restored {
        return with_freeze_note(
            RecoveryOutcome {
                interrupted_checkpoint: true,
                needed_restore: false,
                restored: true,
                already_ok: true,
                error: None,
            },
            freeze_note,
        );
    }

    let Some(original) = checkpoint.original_scheme_guid.clone() else {
        // Исходная схема не зафиксирована (прерывание случилось до первого
        // прогона) — активная схема и есть исходная.
        return with_freeze_note(
            RecoveryOutcome {
                interrupted_checkpoint: true,
                needed_restore: false,
                restored: true,
                already_ok: true,
                error: None,
            },
            freeze_note,
        );
    };

    let schemes = match driver.list_schemes() {
        Ok(list) => list,
        Err(cause) => {
            return with_freeze_note(
                RecoveryOutcome {
                    interrupted_checkpoint: true,
                    needed_restore: true,
                    restored: false,
                    already_ok: false,
                    error: Some(format!("не удалось получить список схем: {cause:?}")),
                },
                freeze_note,
            );
        }
    };

    // Никакого «уже активна — значит всё в порядке» по флажку `*` из
    // `/list`: это тот же список, на котором раньше строился вывод
    // «восстановлено». Источник истины один — подтверждение от ОС через
    // `active_scheme()` внутри `restore_verified`.
    let name = schemes
        .iter()
        .find(|s| s.guid.eq_ignore_ascii_case(&original))
        .map(|s| s.name.clone());

    // Возврат с подтверждением от ОС. Раньше стоял «голый» `set_active`:
    // `powercfg /setactive` возвращает код 0 и при этом может ничего не
    // переключить (конфликт доменной политики, OEM-агент). Точка при этом
    // помечалась восстановленной навсегда, и ни этот, ни следующий запуск
    // уже не пытались вернуть пользователю его схему.
    if let Err(cause) = crate::session::restore_verified(driver, &original) {
        let label = name
            .as_deref()
            .map(|n| format!(" «{n}»"))
            .unwrap_or_default();
        return with_freeze_note(
            RecoveryOutcome {
                interrupted_checkpoint: true,
                needed_restore: true,
                restored: false,
                already_ok: false,
                error: Some(format!(
                    "не удалось восстановить исходную схему{label}: {cause}"
                )),
            },
            freeze_note,
        );
    }

    with_freeze_note(
        RecoveryOutcome::after_restore(&mut checkpoint, None),
        freeze_note,
    )
}

/// Добавить сообщение о карантине по маркеру к итогу восстановления.
/// Без маркера итог возвращается как есть (существующие тесты не меняются).
fn with_freeze_note(mut outcome: RecoveryOutcome, freeze_note: Option<String>) -> RecoveryOutcome {
    if let Some(note) = freeze_note {
        outcome.error = Some(match outcome.error.take() {
            Some(e) => format!("{e} {note}"),
            None => note,
        });
        outcome.already_ok = false;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{DATA_DIR_LOCK, checkpoint_path, save_checkpoint};
    use crate::config::SessionConfig;
    use powerbench_windows::powercfg::PowerScheme;

    fn clear_checkpoint() {
        let _ = std::fs::remove_file(checkpoint_path());
    }

    /// Минимальный драйвер для тестов восстановления.
    struct FakeDriver {
        schemes: Vec<String>,
        active: std::cell::RefCell<String>,
        fail_restore: std::cell::RefCell<Option<String>>,
        /// `set_active` возвращает `Ok`, но схему НЕ переключает — поведение
        /// `powercfg /setactive` под доменной политикой.
        lie_on_set: std::cell::RefCell<bool>,
    }

    impl FakeDriver {
        fn new(schemes: &[&str], active: &str) -> Self {
            Self {
                schemes: schemes.iter().map(|s| s.to_string()).collect(),
                active: std::cell::RefCell::new(active.to_string()),
                fail_restore: std::cell::RefCell::new(None),
                lie_on_set: std::cell::RefCell::new(false),
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
            if !*self.lie_on_set.borrow() {
                *self.active.borrow_mut() = guid.to_string();
            }
            Ok(())
        }
        fn active_scheme(&self) -> Result<String, String> {
            Ok(self.active.borrow().clone())
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
            accept_dirty_background: false,
            worker_count: None,
            scheme_ids: vec!["test-a".to_string()],
            plan_guid: "recovery-plan".to_string(),
            reference_scheme_id: None,
        }
    }

    fn checkpoint_with(active: Option<&str>, restored: bool) -> crate::checkpoint::Checkpoint {
        let mut cp = crate::checkpoint::Checkpoint::new(plan());
        cp.original_scheme_guid = active.map(|s| s.to_string());
        cp.original_restored = restored;
        cp
    }

    fn with_clean_checkpoint<T>(f: impl FnOnce() -> T) -> T {
        let _guard = DATA_DIR_LOCK
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
            let cp = load_checkpoint()
                .expect("читается")
                .expect("есть контрольная точка");
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
            let cp = load_checkpoint()
                .expect("читается")
                .expect("есть контрольная точка");
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

    /// Регресс C8: «голый» `set_active` здесь возвращал код 0, и этого
    /// хватало, чтобы пометить точку восстановленной навсегда — при схеме,
    /// которая так и не вернулась пользователю. Теперь возврат идёт через
    /// подтверждение от ОС, и молчаливый отказ остаётся отказом.
    #[test]
    fn silent_set_active_success_does_not_mark_the_checkpoint_restored() {
        with_clean_checkpoint(|| {
            save_checkpoint(&checkpoint_with(Some("orig"), false)).unwrap();
            let driver = recovery_driver("test-a");
            *driver.lie_on_set.borrow_mut() = true;
            let outcome = recover_interrupted_session(&driver);
            assert!(outcome.interrupted_checkpoint);
            assert!(outcome.needed_restore);
            assert!(!outcome.restored, "молчаливый отказ принят за успех");
            assert!(!outcome.already_ok);
            let cause = outcome.error.expect("причина отказа обязана быть названа");
            assert!(
                cause.contains("orig"),
                "в ошибке нет исходной схемы: {cause}"
            );
            // Точка остаётся невосстановленной: следующий запуск повторит.
            let cp = load_checkpoint()
                .expect("читается")
                .expect("есть контрольная точка");
            assert!(
                !cp.original_restored,
                "точка помечена восстановленной без подтверждения от ОС"
            );
            assert_eq!(
                driver.active(),
                "test-a",
                "тестовая схема осталась активной — и это должно быть видно"
            );
        });
    }
}
