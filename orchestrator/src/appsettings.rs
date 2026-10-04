//! `appsettings.json` — настройки приложения (Этап 6): секция benchmark
//! (порог фоновой нагрузки), секция appearance (тема, режим, reduce motion,
//! sidebar), списки GUID избранных и исключённых из теста схем, веса
//! скоринга. Автосохранение при изменениях, запись атомарная (временный файл
//! + перемещение).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::checkpoint::data_dir;

/// Имя файла настроек в каталоге данных.
pub const APPSETTINGS_FILE_NAME: &str = "appsettings.json";

/// Порог фоновой нагрузки по умолчанию (%).
pub const DEFAULT_BACKGROUND_THRESHOLD_PERCENT: f64 = 5.0;

/// Настройки сценария (секция `benchmark`).
///
/// Длительность, разогрев, охлаждение и повторы отсюда убраны: они задаются
/// пресетами режима (`config::QUICK_PRESET` / `DETAILED_PRESET`), а не
/// персистентными настройками. Держать вторую копию этих чисел в
/// `appsettings.json` было прямой дорогой к расхождению — интерфейс брал
/// значения из файла, а пресет на экране обещал другие. Старые файлы с этими
/// ключами по-прежнему читаются: serde молча игнорирует неизвестные поля.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BenchmarkSettings {
    pub background_threshold_percent: f64,
}

impl Default for BenchmarkSettings {
    fn default() -> Self {
        Self {
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
    /// Плотность интерфейса: «compact» / «normal» / «roomy».
    ///
    /// Множитель отступов, высот контролов и размера иконок меню: 0.92 / 1 /
    /// 1.1. Живёт в CSS как `--k`, см. `styles.css`.
    pub density: String,
    /// Масштаб текста: «s» / «m» / «l».
    ///
    /// Множитель только кеглей, геометрия не трогается: 0.94 / 1 / 1.08. В CSS
    /// это `--kt`. Настройки независимы — «крупный текст + компактная
    /// плотность» валидная комбинация.
    pub text_scale: String,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "Graphite".to_string(),
            mode: "Dark".to_string(),
            reduce_motion: false,
            sidebar_collapsed: true,
            density: "normal".to_string(),
            text_scale: "m".to_string(),
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
        // Держим в согласии с `result::default_score_weights`, откуда берутся
        // веса при пустой/битой секции `scoring`.
        Self {
            performance: 40.0,
            stability: 30.0,
            worst_second: 30.0,
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
    /// Своими словами о настройках железа и BIOS: разгон, андерволт,
    /// отключённые функции, планки памяти, второй GPU.
    ///
    /// Поле существует потому, что разгон **невозможно определить
    /// программно**. Windows отдаёт только текущий разрешённый потолок
    /// частоты, а в него одинаково входят и штатный буст, и ручная настройка
    /// в BIOS. Номинальная частота из реестра тоже не помогает: любой
    /// современный процессор превышает её на бусту. Поэтому единственный
    /// источник сведений — сам пользователь, и отчёт для поддержки обязан
    /// его показывать: без этого невозможно понять, почему две сессии на
    /// одной и той же машине дали разный результат.
    ///
    /// Пустая строка означает «разгона нет, стоковая конфигурация».
    pub cpu_notes: String,
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
        Self::load_checked(path).unwrap_or_default()
    }

    /// Загрузить, различая «нет файла» и «файл повреждён».
    ///
    /// Регресс H39: повреждённый файл раньше читался как дефолты, и первая же
    /// следующая запись (`update_locked`) перезаписывала его — все настройки
    /// пользователя исчезали безвозвратно. Вызывающий обязан узнать о
    /// повреждении и сказать об этом, а не заменить файл дефолтами.
    pub fn load_checked(path: &Path) -> Result<Self, String> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("не удалось прочитать настройки: {e}")),
        };
        crate::storage::parse_user_file(&bytes, path)
            .map(|opt| opt.unwrap_or_default())
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
    ///
    /// Повреждённый файл **не перезаписывается** (регресс H39). Раньше он
    /// разбирался как дефолты, и первая же правка стирала всё, что в нём было:
    /// настройки пользователя исчезали безвозвратно, а интерфейс показывал
    /// «всё сброшено» и не говорил почему. Теперь приходит `Err` с диагнозом,
    /// файл остаётся на диске, а команда интерфейса сообщает пользователю, куда
    /// смотреть.
    pub fn update_locked<F: FnOnce(&mut Self)>(f: F) -> io::Result<()> {
        Self::update_locked_to(&appsettings_path(), f)
    }

    /// То же для произвольного пути (тесты и отчёт для поддержки).
    pub fn update_locked_to<F: FnOnce(&mut Self)>(path: &Path, f: F) -> io::Result<()> {
        // `Option` позволяет вызвать замыкание ровно один раз: `update_file`
        // гарантирует единственный вызов, но компилятор этого не знает.
        let mut f = Some(f);
        // `update_file_checked` запрещает запись поверх повреждённого содержимого,
        // поэтому запасной путь «записать дефолты» больше не нужен вовсе: он
        // был ровно тем местом, где стирались данные пользователя.
        crate::storage::update_file_checked(path, |cur| {
            let mut s: Self = crate::storage::parse_user_file(cur, path)
                .map_err(|msg| io::Error::new(io::ErrorKind::InvalidData, msg))?
                .unwrap_or_default();
            if let Some(apply) = f.take() {
                apply(&mut s);
            }
            serde_json::to_vec_pretty(&s)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        })?;
        if let Some(apply) = f {
            // Замыкание не вызвано: файл не читался вовсе. Пишем настройки с
            // правкой, но только если читать было нечего.
            let mut s = Self::default();
            apply(&mut s);
            s.save_to(path)?;
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
        assert_eq!(s.benchmark.background_threshold_percent, 5.0);
        assert_eq!(s.appearance.theme, "Graphite");
        assert_eq!(s.appearance.mode, "Dark");
        assert!(!s.appearance.reduce_motion);
        assert!(s.appearance.sidebar_collapsed);
        assert!(s.favorite_schemes.is_empty());
        assert!(s.excluded_schemes.is_empty());
        // Пустая строка означает «разгона нет»: иначе в отчёте для поддержки
        // появилось бы утверждение, которого никто не подтверждал.
        assert!(s.cpu_notes.is_empty());
    }

    /// Заметка о железе обязана переживать перезапуск: это единственный
    /// носитель сведений о разгоне, который программа не может выяснить сама.
    #[test]
    fn cpu_notes_survive_a_restart() {
        let dir = tmp_dir("notes");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        let s = AppSettings {
            cpu_notes: "PBO +200 МГц, андерволт -30 на CPU".to_string(),
            ..AppSettings::default()
        };
        s.save_to(&path).unwrap();
        assert_eq!(AppSettings::load_from(&path).cpu_notes, s.cpu_notes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip_preserves_all_sections() {
        let dir = tmp_dir("roundtrip");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        let mut s = AppSettings::default();
        s.benchmark.background_threshold_percent = 12.5;
        s.appearance.theme = "Ocean".to_string();
        s.appearance.mode = "Light".to_string();
        s.appearance.reduce_motion = true;
        s.favorite_schemes.push("guid-1".to_string());
        s.excluded_schemes.push("guid-2".to_string());
        s.cpu_notes = "разгон памяти".to_string();
        s.save_to(&path).unwrap();
        let loaded = AppSettings::load_from(&path);
        assert_eq!(loaded, s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Файл, написанный прошлой версией, где секция `benchmark` хранила ещё и
    /// длительность с повторами. Приложение обязано его прочитать, а не
    /// отвергнуть: иначе правка пресетов обнулила бы настройки пользователя.
    #[test]
    fn legacy_benchmark_keys_are_ignored_not_fatal() {
        let dir = tmp_dir("legacy");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        std::fs::write(
            &path,
            r#"{"benchmark":{"duration_seconds":60,"warmup_seconds":9,"cooling_seconds":7,"repetitions":4,"background_threshold_percent":8.0}}"#,
        )
        .unwrap();
        let s = AppSettings::load_from(&path);
        assert_eq!(s.benchmark.background_threshold_percent, 8.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let dir = tmp_dir("defaults");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        // Старый/частичный файл: только секция benchmark с одним ключом.
        std::fs::write(
            &path,
            r#"{"benchmark":{"background_threshold_percent":7.5},"favorite_schemes":["x"]}"#,
        )
        .unwrap();
        let s = AppSettings::load_from(&path);
        // Отсутствующие поля и секции дополняются дефолтами.
        assert_eq!(s.benchmark.background_threshold_percent, 7.5);
        assert_eq!(s.appearance.theme, "Graphite");
        assert_eq!(s.favorite_schemes, vec!["x"]);
        assert!(s.excluded_schemes.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Регресс H39: усечённый файл настроек обязан быть виден, а не стёрт.
    ///
    /// Раньше он читался как дефолты, и первая же автосохранённая правка
    /// перезаписывала его целиком: заметка о разгоне, избранное, исключённые
    /// схемы исчезали безвозвратно, а интерфейс показывал «всё сброшено».
    #[test]
    fn a_corrupt_settings_file_is_reported_and_not_overwritten() {
        let dir = tmp_dir("corrupt");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        let broken = "{\"cpu_notes\": \"андерволт -30\"";
        std::fs::write(&path, broken).unwrap();

        // Чтение обязано сообщить о повреждении, а не выдать дефолты.
        let err = AppSettings::load_checked(&path)
            .expect_err("повреждённые настройки прочитаны как дефолты");
        assert!(err.contains("повреждён"), "{err}");
        // Для мест, где отказ хуже, мягкий вариант остаётся — но тихо.
        assert!(AppSettings::load_from(&path).cpu_notes.is_empty());

        // Автосохранение обязано отказать и оставить файл как есть.
        let saved = AppSettings::update_locked_to(&path, |s| {
            s.appearance.theme = "Ocean".to_string();
        });
        assert!(
            saved.is_err(),
            "правка повреждённого файла прошла успехом: пользователь потерял данные"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            broken,
            "повреждённый файл перезаписан — данные пользователя уничтожены"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Целый файл по-прежнему обновляется, а отсутствующий — создаётся.
    #[test]
    fn update_locked_creates_and_updates_a_missing_file() {
        let dir = tmp_dir("locked-missing");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        AppSettings::update_locked_to(&path, |s| {
            s.cpu_notes = "разгон памяти".to_string();
        })
        .unwrap();
        assert_eq!(
            AppSettings::load_from(&path).cpu_notes,
            "разгон памяти",
            "файл создан не с той правкой"
        );

        // Вторая правка применяется поверх первой, а не вместо неё.
        AppSettings::update_locked_to(&path, |s| {
            s.favorite_schemes.push("guid-1".to_string());
        })
        .unwrap();
        let loaded = AppSettings::load_from(&path);
        assert_eq!(loaded.cpu_notes, "разгон памяти", "прежняя правка потеряна");
        assert_eq!(loaded.favorite_schemes, vec!["guid-1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_autosaves_changes() {
        let dir = tmp_dir("update");
        let path = dir.join(APPSETTINGS_FILE_NAME);
        let mut s = AppSettings::default();
        s.update_to(&path, |s| {
            s.benchmark.background_threshold_percent = 9.0;
            s.appearance.sidebar_collapsed = true;
            s.excluded_schemes.push("g".to_string());
        })
        .unwrap();
        assert_eq!(
            AppSettings::load_from(&path)
                .benchmark
                .background_threshold_percent,
            9.0
        );
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
