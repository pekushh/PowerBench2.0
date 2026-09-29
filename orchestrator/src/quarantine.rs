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

use crate::checkpoint::data_dir;

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
///
/// Числа не «из воздуха», а следуют из того, что именно мы хотим отсечь:
///
/// * [`MIN_RUNS_FOR_JUDGMENT`] — меньше трёх прогонов медиана статистически
///   неустойчива: один выброс (фоновый антивирус, драйвер) целиком определяет
///   результат. Смотреть на такие данные и браковать нельзя.
/// * [`UNSTABLE_MAD_LIMIT`] — разброс прогонов в 25% и выше означает, что
///   схема даёт нестабильный результат: пользователю нельзя на неё полагаться.
///   Метрика — среднее абсолютное отклонение (MAD) в процентах от медианы:
///   устойчива к выбросам, в отличие от σ, которую сами же выбросы раздувают.
/// * [`DEGRADED_SHARE_LIMIT`] — схема медленнее 20% от лучшей медианы сессии
///   практически бесполезна на этой машине. 20% (а не 10%) оставлены с
///   запасом, чтобы не срезать схему, которая просто слабее, но рабочая.
/// * [`MIN_MARGIN_PERCENT`] — минимальный перевес, ради которого вообще
///   имеет смысл что-то различать: 1% — это граница, ниже которой
///   преимущество не воспроизводимо от прогона к прогону.
pub const MIN_RUNS_FOR_JUDGMENT: usize = 3;
/// Нестабильность: MAD в % от медианы, выше которой схема в карантин.
pub const UNSTABLE_MAD_LIMIT: f64 = 25.0;
/// Деградация: доля от лучшей медианы, ниже которой схема в карантин.
pub const DEGRADED_SHARE_LIMIT: f64 = 0.20;
/// Деградация проверяется начиная с этих прогонов (меньше — рано).
pub const DEGRADED_MIN_RUNS: usize = 2;
/// Катастрофическая деградация: доля от лучшей медианы, ниже которой схема
/// бракуется уже по одному прогону, без ожидания повторов.
///
/// Порог в 5 % — это «схема почти не работает»: двадцатикратное отставание не
/// бывает шумом измерения, в отличие от 20 % из [`DEGRADED_SHARE_LIMIT`].
pub const CATASTROPHIC_SHARE_LIMIT: f64 = 0.05;
/// Сравнение с лидером имеет смысл только при заметном перевесе.
pub const MIN_MARGIN_PERCENT: f64 = 1.0;
/// Доля от лучшего результата ЭТОЙ ЖЕ МАШИНЫ, ниже которой схема считается
/// негодной.
///
/// Применяется только там, где сравнивать не с чем (сессия с одной схемой), и
/// только при наличии истории той же конфигурации. Константа в тиках в секунду
/// здесь была бы ошибкой: 100 тик/с — это много для ноутбука и мало для
/// станции, и на медленной машине она браковала бы здоровые схемы.
pub const MACHINE_COLLAPSE_SHARE: f64 = 0.20;
/// Провал относительно собственной истории машины.
///
/// Единственное место, где решению нужен абсолютный ориентир: сессия с одной
/// схемой сравнивать не с чем. Ориентир — не константа, а лучший результат
/// этой же машины при той же конфигурации, и без него решение не принимается:
/// иначе на медленной машине healthy-схема была бы объявлена поломкой.
pub fn below_machine_baseline(median: f64, baseline_best_median: Option<f64>) -> bool {
    let Some(best) = baseline_best_median else {
        return false;
    };
    if !(median.is_finite() && best.is_finite() && best > 0.0) {
        return false;
    }
    median > 0.0 && median / best < MACHINE_COLLAPSE_SHARE
}

/// Провал фазы по худшему окну: у схемы P1 ниже доли лидера той же фазы.
///
/// Сравнение идёт **внутри одной фазы и одного замера**, поэтому медленная
/// машина взаимно сокращается из обеих сторон и на решение не влияет. Именно
/// это отличает правило от прочих: медиана плохой схемы может быть втрое ниже
/// лидерской и всё равно не означать поломки, а вот P1 в разы ниже при живой
/// медиане — это и есть «схема проваливает такты».
///
/// Порогов в тиках в секунду здесь нет намеренно. Абсолютная константа была бы
/// свойством машины, а не схемы: сто тиков — много для ноутбука и мало для
/// станции, и на медленной машине она браковала бы здоровые схемы. Если все
/// схемы одинаково медленные, то ни у одной нет малой доли от лидера и
/// браковки не происходит — это и есть защита от «все тут медленные».
///
/// Один прогон здесь достаточен: сравнение одновременное, а не статистическое.
pub fn phase_floor_collapse(own_p1: f64, leader_p1: f64) -> bool {
    if !(own_p1.is_finite() && leader_p1.is_finite() && leader_p1 > 0.0) {
        return false;
    }
    own_p1 / leader_p1 < DEGRADED_SHARE_LIMIT
}

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
    crate::storage::read_json(&quarantine_path()).unwrap_or_default()
}

/// Схема в карантине? Сравнение по GUID, регистронезависимое.
pub fn is_quarantined(entries: &[QuarantineEntry], scheme_id: &str) -> bool {
    entries
        .iter()
        .any(|e| e.scheme_id.eq_ignore_ascii_case(scheme_id))
}

/// Добавить схему в карантин. Возвращает `true`, если запись новая,
/// `false` — если схема уже в карантине (идемпотентно).
///
/// Всё «прочитать-проверить-записать» выполняется под блокировкой файла:
/// вызывающих одновременно несколько (фоновый поток сессии, команды
/// интерфейса, восстановление при старте), и без блокировки правки терялись.
pub fn quarantine_add(
    scheme_id: &str,
    scheme_name: Option<&str>,
    kind: QuarantineKind,
    reason: &str,
    plan_guid: &str,
) -> io::Result<bool> {
    let path = quarantine_path();
    let scheme_id = scheme_id.to_string();
    let entry = QuarantineEntry {
        scheme_id: scheme_id.clone(),
        scheme_name: scheme_name.map(str::to_string),
        kind,
        reason: reason.to_string(),
        at_ns: now_ns(),
        plan_guid: plan_guid.to_string(),
    };
    let mut added = false;
    crate::storage::update_file(&path, |cur| {
        let mut entries: Vec<QuarantineEntry> = serde_json::from_slice(cur).unwrap_or_default();
        if entries
            .iter()
            .any(|e| e.scheme_id.eq_ignore_ascii_case(&scheme_id))
        {
            return serde_json::to_vec_pretty(&entries).unwrap_or_default();
        }
        entries.push(entry.clone());
        added = true;
        serde_json::to_vec_pretty(&entries).unwrap_or_default()
    })?;
    Ok(added)
}

/// Убрать схему из карантина (возврат пользователем). `true` — была запись.
pub fn quarantine_remove(scheme_id: &str) -> io::Result<bool> {
    let path = quarantine_path();
    let scheme_id = scheme_id.to_string();
    let mut removed = false;
    crate::storage::update_file(&path, |cur| {
        let mut entries: Vec<QuarantineEntry> = serde_json::from_slice(cur).unwrap_or_default();
        let before = entries.len();
        entries.retain(|e| !e.scheme_id.eq_ignore_ascii_case(&scheme_id));
        removed = entries.len() != before;
        serde_json::to_vec_pretty(&entries).unwrap_or_default()
    })?;
    Ok(removed)
}

/// Записать маркер активного прогона (после успешного `set_active`).
pub fn write_testing_marker(marker: &TestingMarker) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(marker)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    crate::storage::atomic_write(&testing_marker_path(), &bytes)
}

/// Стереть маркер (прогон завершён записью результата или браковкой).
/// Отсутствие файла — не ошибка.
pub fn clear_testing_marker() {
    let _ = crate::storage::remove_file_if_exists(&testing_marker_path());
}

/// Прочитать маркер незавершённого прогона, если он остался с прошлого запуска.
pub fn load_testing_marker() -> Option<TestingMarker> {
    crate::storage::read_json(&testing_marker_path())
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

/// Медиана отсортированной копии (пустое — 0).
fn median_of(values: &[f64]) -> f64 {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Правило нестабильности: разброс прогонов как **MAD в % от медианы**.
///
/// Почему не σ: стандартное отклонение неустойчиво — два выброса из десяти
/// прогонов дают σ больше, чем у схемы с настоящим разбросом, и схема проходит
/// незамеченной. Медианное абсолютное отклонение игнорирует крайние значения,
/// поэтому «один плохой прогон» не помечает схему как нестабильную, а
/// «каждый прогон разный» — помечает.
///
/// Возвращает `Some(mad_percent)`, если данных достаточно и правило сработало.
pub fn unstable_spread(values: &[f64], min_runs: usize, limit_percent: f64) -> Option<f64> {
    let valid: Vec<f64> = values
        .iter()
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect();
    if valid.len() < min_runs.max(2) {
        return None;
    }
    let med = median_of(&valid);
    if med <= 0.0 {
        return None;
    }
    let deviations: Vec<f64> = valid.iter().map(|v| (v - med).abs()).collect();
    let mad = median_of(&deviations);
    // MAD → «процент разброса». 1.4826 приводит MAD к масштабу σ для
    // нормального распределения; для пары-тройки прогонов это избыточно,
    // поэтому ограничиваем снизу, чтобы не делить слишком мелкий ноль.
    let spread_percent = if mad > 0.0 {
        mad / med * 100.0
    } else {
        // Все прогоны совпали — разброс нулевой, нестабильности нет.
        0.0
    };
    (spread_percent > limit_percent).then_some(spread_percent)
}

/// Правило деградации: медиана схемы заметно ниже лучшей медианы сессии.
///
/// `best` — медиана лучшей схемы той же сессии. Сравнение медиан (а не средних)
/// устойчиво к выбросам; минимальный перевес [`MIN_MARGIN_PERCENT`] не даёт
/// браковать схемы, которые просто чуть-чуть отстают.
pub fn degraded_share(median: f64, best_median: f64, share_limit: f64) -> bool {
    if !(median.is_finite() && best_median.is_finite() && best_median > 0.0 && median >= 0.0) {
        return false;
    }
    // Два независимых условия:
    //  1) доля от лидера ниже порога;
    //  2) сам перевес не меньше минимального (защищает от браковки схем,
    //     отстающих на доли процента, где разница — шум измерения).
    let share = median / best_median;
    let margin_percent = (best_median - median) / best_median * 100.0;
    share < share_limit && margin_percent >= MIN_MARGIN_PERCENT
}

/// Правило катастрофической деградации: схема даёт ничтожную долю тиков
/// лидера той же сессии.
///
/// Отдельное от [`degraded_share`] правило, потому что у него **другое число
/// пронов**. Обычная деградация требует [`DEGRADED_MIN_RUNS`] прогонов: разница
/// в 20 % на одном прогоне — это шум. Но когда схема выдаёт 0,3 % тиков лидера
/// (10 тик/с против 3000), никакой шум это не объясняет, и ждать второго прогона
/// незачем: пользователь запускает «Быстро» (один раунд) именно затем, чтобы
/// быстро отсеять заведомо плохую схему. Без этого правила режим скрининга
/// браковал бы ровно то, ради чего запущен.
pub fn catastrophic_share(median: f64, best_median: f64) -> bool {
    if !(median.is_finite() && best_median.is_finite() && best_median > 0.0 && median >= 0.0) {
        return false;
    }
    median / best_median < CATASTROPHIC_SHARE_LIMIT
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
    fn unstable_spread_needs_enough_runs() {
        // Разброс огромный, но прогонов мало — не браковать.
        assert_eq!(unstable_spread(&[100.0, 300.0], 3, UNSTABLE_MAD_LIMIT), None);
        assert_eq!(unstable_spread(&[100.0], 3, UNSTABLE_MAD_LIMIT), None);
        // Стабильная схема — не браковать.
        assert_eq!(
            unstable_spread(&[500.0, 505.0, 498.0], 3, UNSTABLE_MAD_LIMIT),
            None
        );
        // Полностью совпадающие прогоны — разброс нулевой.
        assert_eq!(
            unstable_spread(&[500.0, 500.0, 500.0], 3, UNSTABLE_MAD_LIMIT),
            None
        );
    }

    /// Регресс против σ-метрики: пара выбросов среди нормальных прогонов —
    /// это «шум», а не нестабильность. Настоящая нестабильность — когда
    /// каждый прогон отличается.
    #[test]
    fn unstable_spread_ignores_lone_outliers() {
        let with_outlier = [500.0, 502.0, 498.0, 500.0, 900.0];
        assert_eq!(
            unstable_spread(&with_outlier, 3, UNSTABLE_MAD_LIMIT),
            None,
            "один выброс не должен означать нестабильность"
        );
        let really_unstable = [300.0, 500.0, 800.0, 400.0, 900.0];
        assert!(
            unstable_spread(&really_unstable, 3, UNSTABLE_MAD_LIMIT).is_some(),
            "разброс в каждом прогоне пропущен"
        );
    }

    #[test]
    fn degraded_share_respects_minimum_margin() {
        // Явная деградация.
        assert!(degraded_share(50.0, 1000.0, DEGRADED_SHARE_LIMIT));
        // Отставание меньше порога — не браковать.
        assert!(!degraded_share(500.0, 1000.0, DEGRADED_SHARE_LIMIT));
        // На малых абсолютных величинах перевес в 1% — тоже прощаем.
        assert!(!degraded_share(9.5, 10.0, DEGRADED_SHARE_LIMIT));
        assert!(degraded_share(0.5, 10.0, DEGRADED_SHARE_LIMIT));
        // Вырожденные значения.
        assert!(!degraded_share(f64::NAN, 1000.0, DEGRADED_SHARE_LIMIT));
        assert!(!degraded_share(50.0, 0.0, DEGRADED_SHARE_LIMIT));
    }

    #[test]
    fn catastrophic_share_catches_a_broken_scheme_in_one_run() {
        // Реальный случай пользователя: 10 тик/с против 3000 у рабочей схемы.
        assert!(catastrophic_share(10.0, 3000.0));
        assert!(catastrophic_share(9.0, 2000.0));
        // Умеренное отставание — не катастрофа, его ловит degraded_share после
        // двух прогонов.
        assert!(!catastrophic_share(1500.0, 3000.0));
        assert!(!catastrophic_share(500.0, 3000.0));
        // Вырожденные значения не должны ничего браковать.
        assert!(!catastrophic_share(f64::NAN, 3000.0));
        assert!(!catastrophic_share(10.0, 0.0));
        // Нулевая медиана — не «неизвестно», а самый сильный признак поломки.
        assert!(catastrophic_share(0.0, 3000.0));
    }

    #[test]
    fn phase_floor_collapse_catches_a_stuttering_scheme() {
        // Числа из реального прогона: EverythingTech против Velo в одной фазе.
        // Медиана была 111 против 272 — втрое ниже, но живая. А вот худшее
        // окно 14 против 217: такты схема роняет.
        assert!(phase_floor_collapse(14.0, 217.0));
        // Лёгкая фаза того же прогона: 11.7 % от лидера — тоже проваливает такты.
        assert!(phase_floor_collapse(115.0, 982.0));
        // Частичная фаза — 25.7 % от лидера: это обычное отставание, а не провал
        // тактов, и браковать по ней нельзя. Правило опирается на самую
        // глубокую фазу, а не на любую.
        assert!(!phase_floor_collapse(79.0, 307.0));
        // Нормальное отставание хуже в десять раз браковать нельзя.
        assert!(!phase_floor_collapse(217.0, 217.0));
        assert!(!phase_floor_collapse(150.0, 217.0));
    }

    #[test]
    fn phase_floor_collapse_stays_quiet_when_everyone_is_slow() {
        // Все схемы одинаково медленные: ни у одной нет малой доли от лидера.
        assert!(!phase_floor_collapse(40.0, 60.0));
        assert!(!phase_floor_collapse(50.0, 0.0));
        assert!(!phase_floor_collapse(f64::NAN, 217.0));
    }

    #[test]
    fn phase_floor_collapse_is_machine_independent() {
        // Главный довод против абсолютных порогов: на медленной машине здоровая
        // схема даёт сотни тиков, и константа «ниже 100 — бракуем» объявила бы
        // её поломкой. Правило сравнивает доли, поэтому машина не важна.
        let slow_leader = phase_floor_collapse(12.0, 60.0);
        let fast_leader = phase_floor_collapse(1200.0, 6000.0);
        assert_eq!(
            slow_leader, fast_leader,
            "на медленной и быстрой машине решение должно быть одинаковым"
        );
        // А вот здоровая схема рядом со схемой того же класса не бракуется нигде.
        assert!(!phase_floor_collapse(900.0, 1000.0));
        assert!(!phase_floor_collapse(9000.0, 10000.0));
    }

    #[test]
    fn machine_baseline_guards_the_single_scheme_case() {
        // Есть история машины: 336 против её лучших 3000 — бракуем.
        assert!(below_machine_baseline(336.0, Some(3000.0)));
        // Истории нет — решения не принимаем вовсе, даже при жалких числах.
        assert!(!below_machine_baseline(10.0, None));
        // Медленная машина: её собственная норма — 400, и 200 это половина, а не
        // поломка. Именно этот случай ломала константа.
        assert!(!below_machine_baseline(200.0, Some(400.0)));
        assert!(below_machine_baseline(40.0, Some(400.0)));
        // Мусор и ноль не проходят валидацию.
        assert!(!below_machine_baseline(0.0, Some(3000.0)));
        assert!(!below_machine_baseline(f64::NAN, Some(3000.0)));
        assert!(!below_machine_baseline(10.0, Some(0.0)));
    }
}
