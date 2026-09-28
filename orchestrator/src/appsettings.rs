//! `appsettings.json` — настройки приложения (Этап 6): секция benchmark
//! (длительность, разогрев, охлаждение, повторы, порог фона), секция
//! appearance (тема, режим, reduce motion, sidebar), списки GUID избранных
//! и исключённых из теста схем. Автосохранение при изменениях, запись
//! атомарная (временный файл + перемещение).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::checkpoint::data_dir;

/// Имя файла настроек в каталоге данных.
pub const APPSETTINGS_FILE_NAME: &str = "appsettings.json";

/// Параметры сценария по умолчанию из спецификации «Хранение данных».
pub const DEFAULT_DURATION_SECONDS: u32 = 30;
pub const DEFAULT_WARMUP_SECONDS: u32 = 6;
pub const DEFAULT_COOLING_SECONDS: u32 = 5;
pub const DEFAULT_REPETITIONS: u32 = 3;
pub const DEFAULT_BACKGROUND_THRESHOLD_PERCENT: f64 = 5.0;

/// Настройки сценария (секция `benchmark`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BenchmarkSettings {
    pub duration_seconds: u32,
    pub warmup_seconds: u32,
    pub cooling_seconds: u32,
    pub repetitions: u32,
    pub background_threshold_percent: f64,
}

impl Default for BenchmarkSettings {
    fn default() -> Self {
        Self {
            duration_seconds: DEFAULT_DURATION_SECONDS,
            warmup_seconds: DEFAULT_WARMUP_SECONDS,
            cooling_seconds: DEFAULT_COOLING_SECONDS,
            repetitions: DEFAULT_REPETITIONS,
            background_threshold_percent: DEFAULT_BACKGROUND_THRESHOLD_PERCENT,
        }
    }
}

/// Оформление (секция `appearance`). Палитры/режимы — строки, которые UI
/// Этапа 7 сопоставит с дизайн-системой; здесь только персистентность.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    /// Тема: «Graphite» / «Ocean» / «Violet».
    pub theme: String,
    /// Режим: «Dark» / «Light».
    pub mode: String,
    pub reduce_motion: bool,
    pub sidebar_collapsed: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "Graphite".to_string(),
            mode: "Dark".to_string(),
            reduce_motion: false,
            sidebar_collapsed: true,
        }
    }
}

/// Настройки скоринга (секция `scoring`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoringSettings {
    pub performance: f64,
    pub stability: f64,
    pub worst_second: f64,
}

impl Default for ScoringSettings {
    fn default() -> Self {
        Self {
            performance: 50.0,
            stability: 30.0,
            worst_second: 20.0,
        }
    }
}

/// Настройки удержания данных (секция `retention`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionSettings {
    pub max_sessions: u32,
}

impl Default for RetentionSettings {
    fn default() -> Self {
        Self { max_sessions: 200 }
    }
}

/// Все настройки файла `appsettings.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub benchmark: BenchmarkSettings,
    pub appearance: AppearanceSettings,
    pub scoring: ScoringSettings,
    pub retention: RetentionSettings,
    /// GUID избранных схем (для UI «Схемы питания»).
    pub favorite_schemes: Vec<String>,
    /// GUID схем, исключённых из теста.
    pub excluded_schemes: Vec<String>,
}

/// Путь к `appsettings.json` в каталоге данных.
pub fn appsettings_path() -> PathBuf {
    data_dir().join(APPSETTINGS_FILE_NAME)
}

impl AppSettings {
    /// Загрузить настройки; при отсутствии файла или ошибке разбора — значения
    /// по умолчанию (старые/неполные файлы дополняются дефолтами, без паники).
    pub fn load() -> Self {
        Self::load_from(&appsettings_path())
    }

    /// Загрузить из конкретного пути.
    pub fn load_from(path: &Path) -> Self {
        crate::storage::read_json(path).unwrap_or_default()
    }

    /// Сохранить настройки по стандартному пути (атомарно).
    pub fn save(&self) -> io::Result<()> {
        self.save_to(&appsettings_path())
    }

    /// Сохранить настройки в указанный путь (атомарно, каталоги создаются).
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        crate::storage::atomic_write(path, &bytes)
    }

    /// Изменить настройки через замыкание и автосохранить («автосохранение
    /// при изменениях» по спецификации) в стандартном файле.
    pub fn update<F: FnOnce(&mut Self)>(&mut self, f: F) -> io::Result<()> {
        self.update_to(&appsettings_path(), f)
    }

    /// Изменить настройки и автосохранить в указанный путь (для тестов).
    pub fn update_to<F: FnOnce(&mut Self)>(&mut self, path: &Path, f: F) -> io::Result<()> {
        f(self);
        self.save_to(path)
    }

    /// Изменить настройки по стандартному пути **под блокировкой файла**.
    ///
    /// Настройки меняются из нескольких мест одновременно: команды интерфейса
    /// (избранное, исключение схем, внешний вид) идут на пуле потоков Tauri,
    /// а `set_settings` целиком перезаписывает файл. Без блокировки параллельные
    /// правки затирали друг друга — пользователь терял, например, отметку
    /// «избранное», сохранённую секундой раньше.
    pub fn update_locked<F: FnOnce(&mut Self)>(f: F) -> io::Result<()> {
        let path = appsettings_path();
        // `Option` позволяет вызвать замыкание ровно один раз: `update_file`
        // гарантирует единственный вызов, но компилятор этого не знает.
        let mut f = Some(f);
        let mut applied = false;
        crate::storage::update_file(&path, |cur| {
            let mut s: Self = serde_json::from_slice(cur).unwrap_or_default();
            if let Some(apply) = f.take() {
                apply(&mut s);
            }
            applied = true;
            serde_json::to_vec_pretty(&s).unwrap_or_default()
        })?;
        if let Some(apply) = f {
            // Файл не читался и остался пустым — записываем дефолты с правкой.
            let mut s = Self::default();
            apply(&mut s);
            s.save_to(&path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Уникальный временный каталог для теста (избегаем гонок в параллели).
    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("powerbench-appsettings-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_match_storage_spec() {
        let s = AppSettings::default();
        assert_eq!(s.benchmark.duration_seconds, 30);
        assert_eq!(s.benchmark.warmup_seconds, 6);
        assert_eq!(s.benchmark.cooling_seconds, 5);
        assert_eq!(s.benchmark.repetitions, 3);
        assert_eq!(s.benchmark.background_threshold_percent, 5.0);
        assert_eq!(s.appearance.theme, "Graphite");
        assert_eq!(s.appearance.mode, "Dark");
        assert!(!s.appearance.reduce_motion);
        assert!(s.appearance.sidebar_collapsed);
        assert!(s.favorite_schemes.is_empty());
        assert!(s.excluded_schemes.is_empty());
    }

    #[test]
    fn roundtrip_preserves_all_sections() {
        let dir = tmp_dir("roundtrip");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        let mut s = AppSettings::default();
        s.benchmark.duration_seconds = 60;
        s.appearance.theme = "Ocean".to_string();
        s.appearance.mode = "Light".to_string();
        s.appearance.reduce_motion = true;
        s.favorite_schemes.push("guid-1".to_string());
        s.excluded_schemes.push("guid-2".to_string());
        s.save_to(&path).unwrap();
        let loaded = AppSettings::load_from(&path);
        assert_eq!(loaded, s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let dir = tmp_dir("defaults");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        // Старый/частичный файл: только секция benchmark без Duration-ключа.
        std::fs::write(
            &path,
            r#"{"benchmark":{"warmup_seconds":9},"favorite_schemes":["x"]}"#,
        )
        .unwrap();
        let s = AppSettings::load_from(&path);
        // Отсутствующие поля и секции дополняются дефолтами.
        assert_eq!(s.benchmark.duration_seconds, 30);
        assert_eq!(s.benchmark.warmup_seconds, 9);
        assert_eq!(s.appearance.theme, "Graphite");
        assert_eq!(s.favorite_schemes, vec!["x"]);
        assert!(s.excluded_schemes.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_autosaves_changes() {
        let dir = tmp_dir("update");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        let mut s = AppSettings::default();
        s.update_to(&path, |s| {
            s.benchmark.repetitions = 5;
            s.appearance.sidebar_collapsed = true;
            s.excluded_schemes.push("g".to_string());
        })
        .unwrap();
        assert_eq!(AppSettings::load_from(&path).benchmark.repetitions, 5);
        assert!(AppSettings::load_from(&path).appearance.sidebar_collapsed);
        assert_eq!(AppSettings::load_from(&path).excluded_schemes, vec!["g"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_leaves_no_tmp_artifacts() {
        let dir = tmp_dir("atomic");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        AppSettings::default().save_to(&path).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("json.tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "остались временные файлы: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
