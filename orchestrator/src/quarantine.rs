//! Карантин схем питания — персистентная браковка «с нуля».
//!
//! Почему отдельным файлом, а не `excluded_schemes` в настройках:
//!  1. `set_settings` перезаписывает `excluded_schemes` из DTO, который UI
//!     считал раньше, — авто-исключение фонового потока сессии тихо терялось.
//!  2. Жёсткое зависание ПК убивает процесс до сохранения настроек — нужен
//!     маркер «сейчас тестируется», переживающий убийство процесса.
//!  3. Префлайт-фильтрация должна работать и для CLI, а не только для UI.
//!
//! Механика:
//!  - `quarantine.json` в каталоге данных: список забракованных схем с
//!    причиной, временем и видом браковки. Запись атомарная.
//!  - `testing-now.json`: маркер активного прогона. Пишется ПОСЛЕ успешного
//!    `set_active`, стирается после записи результата/браковки прогона.
//!    Если маркер найден при старте (recovery) — прошлый прогон не завершился:
//!    схема уходит в карантин как `HardFreeze` (подозрение на зависание ПК).
//!  - Префлайт: план фильтруется от карантинных до первого прогона.
//!  - Пост-сессия: правила нестабильности и деградации по статистике прогонов.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::checkpoint::{atomic_write, data_dir};

/// Имя файла карантина в каталоге данных.
pub const QUARANTINE_FILE_NAME: &str = "powerbench-quarantine.json";
/// Имя файла-маркера активного прогона.
pub const TESTING_MARKER_FILE_NAME: &str = "powerbench-testing-now.json";

/// Вид браковки схемы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuarantineKind {
    /// Прогон не завершился (убийство процесса / зависание ПК / ребут).
    HardFreeze,
    /// Сторожевой таймер: нет прогресса тиков более N секунд.
    NoProgress,
    /// Разброс прогонов CV выше порога при достаточной статистике.
    Unstable,
    /// Медиана схемы — малая доля от лучшей медианы сессии.
    Degraded,
}

impl QuarantineKind {
    /// Человекочитаемая метка для UI/отчёта.
    pub fn label(self) -> &'static str {
        match self {
            QuarantineKind::HardFreeze => "зависание",
            QuarantineKind::NoProgress => "нет прогресса",
            QuarantineKind::Unstable => "нестабильна",
            QuarantineKind::Degraded => "деградация",
        }
    }
}

/// Запись о забракованной схеме.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuarantineEntry {
    /// GUID схемы (канонический идентификатор).
    pub scheme_id: String,
    /// Имя на момент браковки (только для отображения).
    pub scheme_name: Option<String>,
    pub kind: QuarantineKind,
    /// Человекочитаемая причина.
    pub reason: String,
    /// Время браковки, нс Unix.
    pub at_ns: u64,
    /// GUID плана сессии, где схема была забракована.
    pub plan_guid: String,
}

/// Маркер активного прогона (переживает убийство процесса).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestingMarker {
    pub scheme_id: String,
    pub scheme_name: Option<String>,
    pub plan_guid: String,
    pub started_at_ns: u64,
}

/// Пороги правил пост-сессии.
pub const UNSTABLE_MIN_RUNS: usize = 3;
/// CV (%) по средним прогонов, выше которого схема — кандидат в карантин.
pub const UNSTABLE_CV_LIMIT: f64 = 25.0;
/// Минимум прогонов схемы для правила деградации.
pub const DEGRADED_MIN_RUNS: usize = 2;
/// Доля от лучшей медианы: ниже — деградация.
pub const DEGRADED_SHARE: f64 = 0.10;

pub fn quarantine_path() -> PathBuf {
    data_dir().join(QUARANTINE_FILE_NAME)
}

pub fn testing_marker_path() -> PathBuf {
    data_dir().join(TESTING_MARKER_FILE_NAME)
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Загрузить карантин; отсутствующий/битый файл — пустой список, без паники.
pub fn load_quarantine() -> Vec<QuarantineEntry> {
    let text = match std::fs::read_to_string(quarantine_path()) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_quarantine(entries: &[QuarantineEntry]) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(entries)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    atomic_write(&quarantine_path(), &bytes)
}

/// Схема в карантине? Сравнение по GUID, регистронезависимое.
pub fn is_quarantined(entries: &[QuarantineEntry], scheme_id: &str) -> bool {
    entries
        .iter()
        .any(|e| e.scheme_id.eq_ignore_ascii_case(scheme_id))
}

/// Добавить схему в карантин. Возвращает `true`, если запись новая,
/// `false` — если схема уже в карантине (идемпотентно).
pub fn quarantine_add(
    scheme_id: &str,
    scheme_name: Option<&str>,
    kind: QuarantineKind,
    reason: &str,
    plan_guid: &str,
) -> io::Result<bool> {
    let mut entries = load_quarantine();
    if is_quarantined(&entries, scheme_id) {
        return Ok(false);
    }
    entries.push(QuarantineEntry {
        scheme_id: scheme_id.to_string(),
        scheme_name: scheme_name.map(str::to_string),
        kind,
        reason: reason.to_string(),
        at_ns: now_ns(),
        plan_guid: plan_guid.to_string(),
    });
    save_quarantine(&entries)?;
    Ok(true)
}

/// Убрать схему из карантина (возврат пользователем). `true` — была запись.
pub fn quarantine_remove(scheme_id: &str) -> io::Result<bool> {
    let mut entries = load_quarantine();
    let before = entries.len();
    entries.retain(|e| !e.scheme_id.eq_ignore_ascii_case(scheme_id));
    if entries.len() == before {
        return Ok(false);
    }
    save_quarantine(&entries)?;
    Ok(true)
}

/// Записать маркер активного прогона (после успешного `set_active`).
pub fn write_testing_marker(marker: &TestingMarker) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(marker)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    atomic_write(&testing_marker_path(), &bytes)
}

/// Стереть маркер (прогон завершён записью результата или браковкой).
/// Отсутствие файла — не ошибка.
pub fn clear_testing_marker() {
    let _ = std::fs::remove_file(testing_marker_path());
}

/// Прочитать маркер незавершённого прогона, если он остался с прошлого запуска.
pub fn load_testing_marker() -> Option<TestingMarker> {
    let text = std::fs::read_to_string(testing_marker_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Префлайт: убрать карантинные схемы из плана.
/// Возвращает (допущенные, пропущенные с причинами).
pub fn preflight_filter(
    scheme_ids: &[String],
    quarantine: &[QuarantineEntry],
) -> (Vec<String>, Vec<(String, String)>) {
    let mut admitted = Vec::new();
    let mut skipped = Vec::new();
    for id in scheme_ids {
        match quarantine
            .iter()
            .find(|e| e.scheme_id.eq_ignore_ascii_case(id))
        {
            Some(e) => skipped.push((
                id.clone(),
                format!("карантин ({}): {}", e.kind.label(), e.reason),
            )),
            None => admitted.push(id.clone()),
        }
    }
    (admitted, skipped)
}

/// Правило нестабильности: CV (%) по средним throughput прогонов.
/// Возвращает CV, если прогонов достаточно (`>= min_runs`) и CV выше лимита.
pub fn unstable_cv(means: &[f64], min_runs: usize, cv_limit: f64) -> Option<f64> {
    let valid: Vec<f64> = means
        .iter()
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect();
    if valid.len() < min_runs.max(2) {
        return None;
    }
    let mean = valid.iter().sum::<f64>() / valid.len() as f64;
    if mean <= 0.0 {
        return None;
    }
    let var = valid.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / valid.len() as f64;
    let cv = var.sqrt() / mean * 100.0;
    (cv > cv_limit).then_some(cv)
}

/// Правило деградации: медиана схемы ниже `share` от лучшей медианы сессии.
pub fn degraded_share(median: f64, best_median: f64, share: f64) -> bool {
    median.is_finite()
        && best_median.is_finite()
        && best_median > 0.0
        && median >= 0.0
        && median < best_median * share
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_labels_are_russian() {
        assert_eq!(QuarantineKind::HardFreeze.label(), "зависание");
        assert_eq!(QuarantineKind::NoProgress.label(), "нет прогресса");
        assert_eq!(QuarantineKind::Unstable.label(), "нестабильна");
        assert_eq!(QuarantineKind::Degraded.label(), "деградация");
    }

    #[test]
    fn preflight_splits_quarantined_case_insensitively() {
        let q = vec![QuarantineEntry {
            scheme_id: "AAA".to_string(),
            scheme_name: None,
            kind: QuarantineKind::NoProgress,
            reason: "нет прогресса".to_string(),
            at_ns: 1,
            plan_guid: "p".to_string(),
        }];
        let ids = vec!["aaa".to_string(), "bbb".to_string()];
        let (admitted, skipped) = preflight_filter(&ids, &q);
        assert_eq!(admitted, vec!["bbb".to_string()]);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].1.contains("нет прогресса"));
    }

    #[test]
    fn preflight_empty_quarantine_admits_all() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let (admitted, skipped) = preflight_filter(&ids, &[]);
        assert_eq!(admitted, ids);
        assert!(skipped.is_empty());
    }

    #[test]
    fn unstable_cv_needs_enough_runs() {
        // Разброс огромный, но прогонов мало — не браковать.
        assert_eq!(unstable_cv(&[100.0, 300.0], 3, UNSTABLE_CV_LIMIT), None);
        // Три прогона с диким разбросом — браковать.
        let cv = unstable_cv(&[100.0, 300.0, 500.0], 3, UNSTABLE_CV_LIMIT);
        assert!(cv.is_some() && cv.unwrap() > UNSTABLE_CV_LIMIT);
        // Стабильная схема — не браковать.
        assert_eq!(
            unstable_cv(&[500.0, 505.0, 498.0], 3, UNSTABLE_CV_LIMIT),
            None
        );
    }

    #[test]
    fn degraded_share_threshold() {
        assert!(degraded_share(50.0, 1000.0, DEGRADED_SHARE));
        assert!(!degraded_share(150.0, 1000.0, DEGRADED_SHARE));
        assert!(!degraded_share(f64::NAN, 1000.0, DEGRADED_SHARE));
        assert!(!degraded_share(50.0, 0.0, DEGRADED_SHARE));
    }
}
