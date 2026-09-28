//! Самопроверка готовности системы к сессии.
//!
//! Запускается перед стартом замера и блокирует старт, если окружение не
//! готово: нет прав администратора, нет питания от сети, powercfg недоступен
//! или выбрано < 2 схем, мало свободного места на диске истории.

use crate::checkpoint::data_dir;
use crate::session::SchemeDriver;
use powerbench_windows::disk::{format_bytes, free_space_bytes};

/// Минимальное свободное место на диске истории (сырые сэмплы + HTML-отчёты).
pub const MIN_FREE_BYTES: u64 = 250 * 1024 * 1024;

/// Результат самопроверки: `ok` = старт разрешён; `issues` — понятные причины.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadyReport {
    pub ok: bool,
    pub issues: Vec<String>,
}

impl ReadyReport {
    fn ok() -> Self {
        Self {
            ok: true,
            issues: Vec::new(),
        }
    }
    fn push(&mut self, issue: String) {
        self.ok = false;
        self.issues.push(issue);
    }
}

/// Проверить готовность окружения под сессию с указанным числом схем.
pub fn system_ready(driver: &dyn SchemeDriver, n_selected: usize) -> ReadyReport {
    let mut report = ReadyReport::ok();

    if !driver.is_admin() {
        report.push(
            "процесс не повышен до администратора: применение схем не сработает \
             (права учётной записи недостаточны — запустите приложение \
             «от имени администратора» и согласитесь на UAC)"
                .to_string(),
        );
    }

    match driver.ac_power_online() {
        Ok(true) => {}
        Ok(false) => {
            report.push("замер запрещён на батарее: подключите питание от сети".to_string());
        }
        Err(e) => {
            report.push(format!("не удалось определить питание: {e}"));
        }
    }

    if n_selected < 2 {
        report.push(format!(
            "для сравнения нужно минимум 2 схемы, выбрано {n_selected}"
        ));
    }
    match driver.list_schemes() {
        Ok(list) => {
            if list.is_empty() {
                report.push("powercfg не нашёл ни одной схемы питания".to_string());
            }
        }
        Err(e) => {
            report.push(format!("powercfg недоступен: {e}"));
        }
    }

    match free_space_bytes(&data_dir()) {
        Ok(free) if free < MIN_FREE_BYTES => {
            report.push(format!(
                "мало места на диске истории: свободно {}, нужно минимум {}",
                format_bytes(free),
                format_bytes(MIN_FREE_BYTES)
            ));
        }
        Ok(_) => {}
        Err(e) => {
            report.push(format!("не удалось проверить место на диске: {e}"));
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoisyDriver;
    impl SchemeDriver for NoisyDriver {
        fn list_schemes(&self) -> Result<Vec<powerbench_windows::powercfg::PowerScheme>, String> {
            Err("powercfg сломан".into())
        }
        fn set_active(&self, _guid: &str) -> Result<(), String> {
            Ok(())
        }
        fn ac_power_online(&self) -> Result<bool, String> {
            Ok(false)
        }
        fn is_admin(&self) -> bool {
            false
        }
    }

    #[test]
    fn ready_report_collects_everything() {
        let r = system_ready(&NoisyDriver, 1);
        assert!(!r.ok, "всё сломано — старт запрещён");
        assert!(r.issues.iter().any(|i| i.contains("администратор")));
        assert!(r.issues.iter().any(|i| i.contains("батарее")));
        assert!(r.issues.iter().any(|i| i.contains("powercfg")));
        assert!(r.issues.iter().any(|i| i.contains("2 схемы")));
    }

    #[test]
    fn ready_report_pass_with_healthy_mock() {
        struct Healthy;
        impl SchemeDriver for Healthy {
            fn list_schemes(
                &self,
            ) -> Result<Vec<powerbench_windows::powercfg::PowerScheme>, String> {
                Ok(vec![
                    powerbench_windows::powercfg::PowerScheme {
                        guid: "a".into(),
                        name: "A".into(),
                        active: true,
                    },
                    powerbench_windows::powercfg::PowerScheme {
                        guid: "b".into(),
                        name: "B".into(),
                        active: false,
                    },
                    powerbench_windows::powercfg::PowerScheme {
                        guid: "c".into(),
                        name: "C".into(),
                        active: false,
                    },
                ])
            }
            fn set_active(&self, _guid: &str) -> Result<(), String> {
                Ok(())
            }
            fn ac_power_online(&self) -> Result<bool, String> {
                Ok(true)
            }
            fn is_admin(&self) -> bool {
                true
            }
        }
        let r = system_ready(&Healthy, 2);
        assert!(r.ok, "ожидали готовность: {:?}", r.issues);
    }
}
