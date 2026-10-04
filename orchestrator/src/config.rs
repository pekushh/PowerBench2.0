//! Конфигурация сессии, пресеты, деление длительности на фазы и план раундов.
//!
//! Здесь же — единственный источник правды про длительность сессии:
//! [`estimate_run_seconds`] и [`format_estimate`], которыми пользуются и
//! оркестратор, и интерфейс, поэтому оценка времени не расходится с фактом.

use crate::session::{BACKGROUND_MEASURE_MS, BACKGROUND_RETRY_PAUSE_MS, BACKGROUND_RUN_ATTEMPTS};

/// Пресет «Быстрый»: длительность 30 с, разогрев 3 с, охлаждение 3 с, 1 повтор.
///
/// Раньше здесь было 9/2/1/1, а девять секунд — это фазы по три. Трёх секунд
/// тяжёлой фазы не хватает, чтобы замер вообще что-то различал: всё это время
/// процессор ещё разгоняется, схемы питания ведут себя одинаково, а разница
/// между ними тонется в шуме. При одном повторе доверительный интервал не
/// строится вовсе, поэтому режим честно называется скринингом: он годен, чтобы
/// отсеять явно слабые схемы, но не для выбора победителя.
///
/// 30 секунд на четыре фазы (8/6/8/8) — минимум, при котором у каждой фазы
/// набирается выборка для перцентилей.
pub const QUICK_PRESET: Preset = Preset {
    duration_seconds: 30,
    warmup_seconds: 3,
    cooling_seconds: 3,
    repetitions: 1,
};

/// Пресет «Детально» (значения по умолчанию): 60 с, 8 с, 5 с, повторов 5.
///
/// 60 секунд — фазы 17/12/19/12. Девятнадцать секунд тяжёлой фазы измеряют
/// уже установившийся режим под нагрузкой, а не разгон, и там политика схемы
/// (максимальное состояние процессора, агрессивность разгона, охлаждение)
/// уже вступила в силу. Двенадцать секунд частичной нагрузки закрывают режим,
/// где различие между схемами максимально.
///
/// Пять повторов — не «больше точности вообще», а минимальное число, при
/// котором адаптивная остановка вообще срабатывает: перевес, достаточный для
/// раннего выхода, падает с ~35 % на трёх повторах до ~18 % на пяти. Три
/// повтора обещали экономию времени, которой на практике не происходило —
/// сессия всё равно шла до конца.
pub const DETAILED_PRESET: Preset = Preset {
    duration_seconds: 60,
    warmup_seconds: 8,
    cooling_seconds: 5,
    repetitions: 5,
};

/// Пресет сценария (параметры без списка схем и GUID плана).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub duration_seconds: u64,
    pub warmup_seconds: u64,
    pub cooling_seconds: u64,
    pub repetitions: u32,
}

/// Параметры сценария сессии.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionConfig {
    /// Длительность измеряемой части (сумма четырёх фаз). Минимум 16 с.
    pub duration_seconds: u64,
    /// Разогрев профилем «Отклик». Минимум 2 с.
    pub warmup_seconds: u64,
    /// Пауза охлаждения между прогонами. 0..=60 с.
    pub cooling_seconds: u64,
    /// Число раундов (повторов). 1..=9.
    pub repetitions: u32,
    /// Порог фоновой нагрузки в % на ядро (по умолчанию 5.0; 0.5..=100).
    pub background_threshold_percent: f64,
    /// Замер разрешено начать при загруженной фоне («продолжить с риском»).
    ///
    /// Кнопка в интерфейсе была всегда, но флаг не доходил до плана: кнопка
    /// лишь прятала себя, а гейт в `run_session` по-прежнему пропускал все
    /// прогоны. Теперь выбор доходит до гейта и означает ровно одно — не
    /// откладывать прогон из-за фона. Остальные причины остановки (нет
    /// админа, нет сети, потеря схемы, watchdog) флаг не отменяет: скомпрометированный
    /// замер нельзя отличить от хорошего, и «с риском» их не лечит.
    ///
    /// `serde(default)` — старые чекпоинты и результаты без поля читаются как
    /// «риск не принимали», то есть как раньше: без отложенного прогона.
    #[serde(default)]
    pub accept_dirty_background: bool,
    /// Число воркеров пула (None — значение по умолчанию движка).
    pub worker_count: Option<usize>,
    /// Идентификаторы выбранных схем (GUID), в порядке предпочтения пользователя.
    pub scheme_ids: Vec<String>,
    /// Схема-эталон, измеряемая один раз в каждом раунде (GUID).
    ///
    /// Её прогоны не участвуют в ранжировании, но дают две вещи, которых
    /// иначе нет: оценку дрейфа машины за сессию (разброс по раундам) и
    /// «перевес против эталона» — утверждение, не зависящее от того, что
    /// остальные схемы мерились в другие моменты. `None` — режим без эталона.
    #[serde(default)]
    pub reference_scheme_id: Option<String>,
    /// Идентификатор плана — попадает в ключи прогонов `"{round}:{plan-guid}"`.
    pub plan_guid: String,
}

/// Ограничения настроек (спецификация).
/// Минимум — четыре фазы по 4 с: меньше фаза не набирает выборки для
/// перцентилей, а перцентили и есть основной результат.
pub const MIN_DURATION_SECONDS: u64 = 16;
/// Потолок длительности измеряемой части: защита от переполнения расчёта
/// ёмкости буфера сэмплов (`duration * ticks_per_second`).
pub const MAX_DURATION_SECONDS: u64 = 3600;
pub const MIN_WARMUP_SECONDS: u64 = 2;
pub const MIN_COOLING_SECONDS: u64 = 0;
pub const MAX_COOLING_SECONDS: u64 = 60;
pub const MIN_REPETITIONS: u32 = 1;
pub const MAX_REPETITIONS: u32 = 9;
/// Потолок числа воркеров (движок всё равно упирается в число ядер).
pub const MAX_WORKER_COUNT: usize = 256;
/// Порог фоновой нагрузки по умолчанию.
pub const DEFAULT_BACKGROUND_PERCENT: f64 = 5.0;
pub const MIN_BACKGROUND_PERCENT: f64 = 0.5;
pub const MAX_BACKGROUND_PERCENT: f64 = 100.0;

/// Пауза после применения схемы питания (секунды). Ждём, чтобы Windows
/// переключила профиль питания до начала замера.
///
/// Это НИЖНЯЯ граница ожидания: схема считается готовой не по прошедшему
/// времени, а когда разрешённый процессором потолок частот (`MaxMhz`) держится
/// одинаковым в двух замерах подряд И активная схема подтверждена. Ожидание
/// поэтому либо короче (ОС переключила мгновенно), либо длиннее (плановая
/// запись задержалась) — вместо того чтобы всегда ждать одно и то же число.
pub const SCHEME_STABILIZE_MIN_SECS: u64 = 1;
/// Потолок ожидания стабилизации: дальше ждать бессмысленно, и замер надо
/// начинать — иначе пользователь решит, что приложение зависло.
pub const SCHEME_STABILIZE_MAX_SECS: u64 = 10;
/// Оценка типичного ожидания для расчёта времени сессии (между минимумом и
/// максимумом; на практике схема стабилизируется за 1-2 с).
pub const SCHEME_STABILIZE_ESTIMATE_SECS: f64 = 2.0;
/// Стабилизационная пауза после каждой измеряемой фазы (секунды): даёт
/// затухнуть эффекту только что прошедшей нагрузки.
pub const STABILIZATION_SECS: u64 = 2;
/// Пауза охлаждения между прогонами по умолчанию (секунды).
pub const DEFAULT_COOLING_SECS: u64 = 5;
/// Число измеряемых фаз в прогоне (Лёгкая, Частичная, Тяжёлая, Отклик).
pub const PHASES_PER_RUN: u64 = 4;
/// Оценка времени проверки фоновой нагрузки: одна проба, а при грязном фоне —
/// до трёх с паузами между ними.
pub const BACKGROUND_CHECK_SECS: f64 = BACKGROUND_MEASURE_MS as f64 / 1000.0
    + (BACKGROUND_RUN_ATTEMPTS as f64 - 1.0) * BACKGROUND_RETRY_PAUSE_MS as f64 / 1000.0;

/// Оценка полного времени сессии в секундах.
///
/// Считает все стадии прогона одной схемы, а не только измеряемое время:
///
/// ```text
/// на прогон = длительность фаз + разогрев
///            + пауза после схемы + стабилизация после каждой фазы
///            + проверка фона + охлаждение
/// сессия    = сумма прогонов по всем схемам × повторы
/// ```
///
/// Это ровно то, что делает `run_session_loop`, поэтому оценка не врёт про
/// «лишние» минуты. Публичная функция — тем же считает и интерфейс.
pub fn estimate_run_seconds(
    duration_seconds: u64,
    warmup_seconds: u64,
    cooling_seconds: u64,
    repetitions: u32,
    scheme_count: usize,
) -> f64 {
    if scheme_count == 0 || repetitions == 0 {
        return 0.0;
    }
    let per_run = duration_seconds as f64
        + warmup_seconds as f64
        + SCHEME_STABILIZE_ESTIMATE_SECS
        + STABILIZATION_SECS as f64 * PHASES_PER_RUN as f64
        + BACKGROUND_CHECK_SECS;
    // Охлаждение идёт после каждого прогона, кроме последнего в плане, то есть
    // `cooling * (всего прогонов − 1)`. Формула вида «на раунд минус одна схема»
    // занижала оценку на `cooling` для каждого раунда: при 2 схемах и 5 раундах
    // обещание отличалось от реальности на четыре охлаждения.
    let total_runs = scheme_count as f64 * repetitions as f64;
    let cooling_total = cooling_seconds as f64 * (total_runs - 1.0).max(0.0);
    per_run * total_runs + cooling_total
}

/// Человекочитаемая оценка времени: «около 2 ч 15 мин», «около 40 с».
///
/// Разделяет округление на минуты и десятки секунд: оценка вида «~14 с» или
/// «~~1 мин 41 с» выглядит как мусор и обесценивает обещание пользователю.
pub fn format_estimate(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "—".to_string();
    }
    let total = seconds.round() as u64;
    if total < 60 {
        return format!("около {total} с");
    }
    let minutes = total / 60;
    let secs = total % 60;
    if minutes < 60 {
        return if secs == 0 {
            format!("около {minutes} мин")
        } else {
            format!("около {minutes} мин {secs} с")
        };
    }
    let hours = minutes / 60;
    let rem = minutes % 60;
    if rem == 0 {
        format!("около {hours} ч")
    } else {
        format!("около {hours} ч {rem} мин")
    }
}

/// Валидация параметров сценария. Возвращает человекочитаемую причину
/// невалидности, либо `None` при валидных значениях.
pub fn validate_config(cfg: &SessionConfig) -> Option<String> {
    if cfg.duration_seconds < MIN_DURATION_SECONDS {
        return Some(format!(
            "длительность должна быть не меньше {} с, задано {}",
            MIN_DURATION_SECONDS, cfg.duration_seconds
        ));
    }
    // Верхняя граница: `capacity_for` умножает длительность на оценку тиков в
    // секунду, и без предела это переполнение (паника в debug, wrap в release).
    if cfg.duration_seconds > MAX_DURATION_SECONDS {
        return Some(format!(
            "длительность должна быть не больше {} с, задано {}",
            MAX_DURATION_SECONDS, cfg.duration_seconds
        ));
    }
    if cfg.warmup_seconds < MIN_WARMUP_SECONDS {
        return Some(format!(
            "разогрев должен быть не меньше {} с, задано {}",
            MIN_WARMUP_SECONDS, cfg.warmup_seconds
        ));
    }
    if !(MIN_COOLING_SECONDS..=MAX_COOLING_SECONDS).contains(&cfg.cooling_seconds) {
        return Some(format!(
            "охлаждение должно быть в диапазоне {}..{} с, задано {}",
            MIN_COOLING_SECONDS, MAX_COOLING_SECONDS, cfg.cooling_seconds
        ));
    }
    if !(MIN_REPETITIONS..=MAX_REPETITIONS).contains(&cfg.repetitions) {
        return Some(format!(
            "повторов должно быть {}..{}, задано {}",
            MIN_REPETITIONS, MAX_REPETITIONS, cfg.repetitions
        ));
    }
    if !(MIN_BACKGROUND_PERCENT..=MAX_BACKGROUND_PERCENT)
        .contains(&cfg.background_threshold_percent)
    {
        return Some(format!(
            "порог фоновой нагрузки должен быть в диапазоне {}..{} %, задано {}",
            MIN_BACKGROUND_PERCENT, MAX_BACKGROUND_PERCENT, cfg.background_threshold_percent
        ));
    }
    if cfg.scheme_ids.is_empty() {
        return Some("выберите хотя бы одну схему питания".to_string());
    }
    // `worker_count == Some(0)` доходил до `Pool::new` и ронял поток паникой
    // «пул воркеров не может быть пустым».
    if let Some(w) = cfg.worker_count {
        if w == 0 {
            return Some("число воркеров должно быть не меньше 1".to_string());
        }
        if w > MAX_WORKER_COUNT {
            return Some(format!(
                "число воркеров должно быть не больше {}, задано {}",
                MAX_WORKER_COUNT, w
            ));
        }
    }
    if cfg.plan_guid.trim().is_empty() {
        return Some("идентификатор плана пуст".to_string());
    }
    None
}

/// Деление длительности на измеряемые фазы (целочисленно).
///
/// Фаз стало четыре: кроме «Лёгкой», «Тяжёлой» и «Отклика» появилась
/// «Частичная» — половина пула занята. Это единственный режим, где различие
/// между схемами питания максимально: при полной загрузке процессор упирается в
/// лимиты мощности, и все схемы выглядят одинаково, а при частичной работает
/// политика разгона и минимального состояния.
///
/// Доли: база по 3 с на фазу, остаток — 30 % лёгкой, 20 % частичной, 35 %
/// тяжёлой, 15 % «Отклика». Тяжёлой отдаётся больше всех сознательно: это
/// единственная фаза, где измеряется установившаяся производительность, и её
/// результат — самый устойчивый из четырёх.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseDurations {
    pub light_seconds: u64,
    pub partial_seconds: u64,
    pub heavy_seconds: u64,
    pub response_seconds: u64,
}

/// Деление длительности на измеряемые фазы.
pub const fn phase_durations(total: u64) -> PhaseDurations {
    let total = if total < MIN_DURATION_SECONDS {
        MIN_DURATION_SECONDS
    } else {
        total
    };
    let extra = total - MIN_DURATION_SECONDS;
    let base = MIN_DURATION_SECONDS / 4;
    let light = base + extra.saturating_mul(30) / 100;
    let partial = base + extra.saturating_mul(20) / 100;
    let heavy = base + extra.saturating_mul(35) / 100;
    let response = total.saturating_sub(light + partial + heavy);
    PhaseDurations {
        light_seconds: light,
        partial_seconds: partial,
        heavy_seconds: heavy,
        response_seconds: response,
    }
}

/// Канонический порядок схем для плана: **активная схема всегда первая**,
/// остальные — строго в порядке, заданном пользователем.
///
/// Активная идёт первой по практической причине: её почти всегда ставят
/// первой вручную, и когда она попадала в середину, первый раунд начинал
/// замер с чужой схемы, а восстановление после прерывания возвращало неё —
/// то есть система возвращала пользователя к тому, что и так стояло.
/// Заодно видимый пользователем порядок становится равным порядку выполнения.
pub fn canonical_scheme_order(scheme_ids: &[String], active: Option<&str>) -> Vec<String> {
    let Some(active) = active else {
        return scheme_ids.to_vec();
    };
    let mut first: Option<String> = None;
    let mut rest = Vec::with_capacity(scheme_ids.len());
    for id in scheme_ids {
        if id.eq_ignore_ascii_case(active) {
            // Каноническое написание берём из плана (регистр GUID от powercfg).
            if first.is_none() {
                first = Some(id.clone());
            }
        } else {
            rest.push(id.clone());
        }
    }
    match first {
        Some(head) => {
            let mut out = Vec::with_capacity(scheme_ids.len());
            out.push(head);
            out.extend(rest);
            out
        }
        // Активной схемы нет в выборе (пользователь её снял) — порядок как есть.
        None => scheme_ids.to_vec(),
    }
}

/// Порядок схем в раунде: ротация против «кто первый» (спецификация).
///
/// ```text
/// cycle  = round / число_схем
/// source = если cycle чётный → исходный список, иначе → перевёрнутый
/// shift  = round mod число_схем
/// порядок = source[shift:] + source[:shift]
/// ```
pub fn round_order(scheme_ids: &[String], round: u32) -> Vec<String> {
    if scheme_ids.is_empty() {
        return Vec::new();
    }
    let n = scheme_ids.len();
    let cycle = round / n as u32;
    let shift = (round % n as u32) as usize;
    // MSRV 1.85: `is_multiple_of` стабилизирован в Rust 1.87.
    #[allow(clippy::manual_is_multiple_of)]
    let source: Vec<String> = if cycle % 2 == 0 {
        scheme_ids.to_vec()
    } else {
        scheme_ids.iter().rev().cloned().collect()
    };
    let mut order = Vec::with_capacity(n);
    order.extend_from_slice(&source[shift..]);
    order.extend_from_slice(&source[..shift]);
    order
}

/// Ключ прогона: `"{round}:{plan-guid}"` (строго по спецификации; имя/guid схемы
/// в ключ не входят — уникальность внутри раунда даёт набор записей прогонов).
pub fn run_key(round: u32, plan_guid: &str) -> String {
    format!("{round}:{plan_guid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Оценка времени учитывает стабилизацию после схемы, стабилизации после фаз,
    /// проверку фона и охлаждение — иначе она систематически занижена.
    #[test]
    fn estimate_covers_every_stage() {
        let per_run = 30.0
            + 6.0
            + SCHEME_STABILIZE_ESTIMATE_SECS
            + STABILIZATION_SECS as f64 * PHASES_PER_RUN as f64
            + BACKGROUND_CHECK_SECS;
        // Охлаждение идёт после каждого прогона, кроме последнего в плане:
        // при 2 схемах и 3 раундах это 6 прогонов и 5 охлаждений, а не «по
        // одному охлаждению на раунд». Прежняя формула обещала на 10 с меньше.
        let total_runs = 2.0 * 3.0;
        let expected = per_run * total_runs + 5.0 * (total_runs - 1.0);
        let s = estimate_run_seconds(30, 6, 5, 3, 2);
        assert!((s - expected).abs() < 1.0, "оценка {s} != {expected}");
        assert!(estimate_run_seconds(30, 6, 5, 3, 3) > s);
        assert!(estimate_run_seconds(30, 6, 5, 4, 2) > s);
        assert_eq!(estimate_run_seconds(30, 6, 5, 3, 0), 0.0);
        assert_eq!(estimate_run_seconds(30, 6, 5, 0, 2), 0.0);
        // Одной схемы и одного раунда хватает для ненулевой оценки, и
        // охлаждения в ней нет вовсе.
        let one = estimate_run_seconds(30, 6, 5, 1, 1);
        assert!((one - per_run).abs() < 1.0, "оценка {one} != {per_run}");
    }

    /// Оценка читаема: без тильд и «~~», с понятными единицами.
    #[test]
    fn estimate_formatting_is_readable() {
        assert_eq!(format_estimate(45.0), "около 45 с");
        assert_eq!(format_estimate(0.0), "—");
        assert_eq!(format_estimate(-5.0), "—");
        assert_eq!(format_estimate(f64::NAN), "—");
        assert_eq!(format_estimate(60.0), "около 1 мин");
        assert_eq!(format_estimate(101.0), "около 1 мин 41 с");
        assert_eq!(format_estimate(3600.0), "около 1 ч");
        assert_eq!(format_estimate(5400.0), "около 1 ч 30 мин");
        for v in [1.0, 14.0, 59.0, 119.0, 3599.0, 7200.0] {
            let s = format_estimate(v);
            assert!(!s.contains('~'), "лишняя тильда: {s}");
        }
    }

    /// Активная схема всегда первая, остальные — в порядке пользователя.
    #[test]
    fn active_scheme_is_moved_to_front() {
        let ids: Vec<String> = ["b", "a", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            canonical_scheme_order(&ids, Some("c")),
            vec!["c", "b", "a"],
            "активная схема не перенесена в начало"
        );
        // Регистр GUID игнорируется.
        assert_eq!(canonical_scheme_order(&ids, Some("A")), vec!["a", "b", "c"]);
        // Активной среди выбранных нет — порядок не трогаем.
        assert_eq!(canonical_scheme_order(&ids, Some("zzz")), ids);
        assert_eq!(canonical_scheme_order(&ids, None), ids);
        // Пустой выбор.
        assert!(canonical_scheme_order(&[], Some("a")).is_empty());
    }

    /// Первый раунд идёт ровно в каноническом порядке плана.
    #[test]
    fn first_round_follows_canonical_order() {
        let plan: Vec<String> = ["z", "y", "x"].iter().map(|s| s.to_string()).collect();
        let ordered = canonical_scheme_order(&plan, Some("y"));
        assert_eq!(round_order(&ordered, 0), ordered);
    }

    fn cfg(preset: Preset, threshold: f64) -> SessionConfig {
        SessionConfig {
            duration_seconds: preset.duration_seconds,
            warmup_seconds: preset.warmup_seconds,
            cooling_seconds: preset.cooling_seconds,
            repetitions: preset.repetitions,
            background_threshold_percent: threshold,
            accept_dirty_background: false,
            worker_count: None,
            scheme_ids: vec!["a".to_string()],
            plan_guid: "plan-1".to_string(),
            reference_scheme_id: None,
        }
    }

    #[test]
    fn default_preset_is_detailed() {
        assert_eq!(DETAILED_PRESET.repetitions, 5);
        assert_eq!(DETAILED_PRESET.duration_seconds, 60);
    }

    /// Регресс M38: таблица пресетов в README обязана совпадать с кодом.
    ///
    /// README обещал `quick` = 9 с / 2 с / 1 с / 1 раунд и `detailed` =
    /// 30 с / 6 с / 5 с / 3 раунда, а в коде было 30/3/3/1 и 60/8/5/5. Человек,
    /// планирующий время сессии по README, получал замер втрое короче
    /// обещанного — а «детальный» пресет с тремя раундами на практике не
    /// позволял адаптивной остановке сработать, то есть обещанной экономии не
    /// происходило. Никакой проверки связи документации с кодом не было, и
    /// расхождение жило годами.
    ///
    /// Читаем таблицу README и сверяем каждое число с константами: правка
    /// пресета без правки README (или наоборот) теперь ломает сборку тестов.
    #[test]
    fn readme_preset_table_matches_the_code() {
        let readme = include_str!("../../README.md");
        let row = |preset: &str| -> String {
            readme
                .lines()
                .find(|l| l.starts_with(&format!("| `{preset}` |")))
                .unwrap_or_else(|| {
                    panic!(
                        "в README нет строки таблицы пресетов для `{preset}` — \
                         таблица разъехалась с кодом"
                    )
                })
                .to_string()
        };

        for (name, preset) in [("quick", QUICK_PRESET), ("detailed", DETAILED_PRESET)] {
            let line = row(name);
            for (label, value) in [
                ("длительность", preset.duration_seconds),
                ("разогрев", preset.warmup_seconds),
                ("охлаждение", preset.cooling_seconds),
                ("раундов", u64::from(preset.repetitions)),
            ] {
                // В таблице длительности пишутся с единицей измерения, а число
                // раундов — само по себе.
                let wanted = if label == "раундов" {
                    value.to_string()
                } else {
                    format!("{value} с")
                };
                assert!(
                    line.contains(&wanted),
                    "в README пресет `{name}`: {label} = {value}, а в таблице \
                     строка «{line}». Правь README или константу пресета."
                );
            }
        }
    }

    /// Регресс M38: README не должен обещать несуществующие флаги.
    ///
    /// В репозитории лежал `bench.example.toml` с ключами `duration_seconds`,
    /// `export_raw_samples`, `out_path` и флагом `--config` — CLI не читал ни
    /// одного из них и никогда не имел `--config`. Пользователь правил файл и
    /// ничего не получал.
    #[test]
    fn documentation_does_not_promise_unsupported_options() {
        let readme = include_str!("../../README.md");
        let example = include_str!("../../bench.example.toml");

// `--config` не существует ни в CLI, ни в справочнике флагов. Ищем именно
        // ветку разбора (`"--config" =>`), а не любое упоминание: иначе тест
        // находил бы сам себя — он тоже пишет про этот флаг.
let cli_src = include_str!("../../cli/src/main.rs");
        let bench_src = include_str!("../../cli/src/bench.rs");
        assert!(
            !bench_src.contains("\"--config\" =>") && !cli_src.contains("\"--config\" =>"),
            "появился флаг --config — тогда его надо описать, а не отрицать"
        );
        for (name, text, comment_prefix) in [
            ("README.md", readme, ">"),
            ("bench.example.toml", example, "#"),
        ] {
            // `--config` может упоминаться в ПРОЗЕ («раньше был флаг, теперь
            // его нет») — это полезно и честно. Запрещено другое: строка, которая
            // читается как инструкция выполнить команду с этим флагом.
            for (i, line) in text.lines().enumerate() {
                let is_prose = line.trim_start().starts_with(comment_prefix);
                if !is_prose && line.contains("--config") {
                    panic!(
                        "{name}:{i}: строка предлагает `--config`, которого нет \
                         в CLI:\n  {line}"
                    );
                }
            }
        }

        // Ключи, которых CLI не читает, не должны выглядеть как рабочие.
        for key in ["export_raw_samples", "out_path"] {
            assert!(
                !example.contains(&format!("{key} =")),
                "bench.example.toml снова предлагает ключ `{key}`, которого \
                 программа не читает"
            );
        }
    }

    /// Чекпоинт, записанный до появления флага, должен читаться как «риск не
    /// принимали». Без `serde(default)` продолжение старой сессии падало бы с
    /// ошибкой десериализации — то есть отменялось само продолжение.
    #[test]
    fn plan_without_risk_flag_loads_as_no_risk() {
        let mut json = serde_json::to_value(cfg(QUICK_PRESET, 5.0)).expect("в json");
        json.as_object_mut()
            .expect("объект")
            .remove("accept_dirty_background");
        let back: SessionConfig = serde_json::from_value(json).expect("читается без поля");
        assert!(!back.accept_dirty_background);
    }

    /// Обратное направление: согласие на риск не должно теряться при записи
    /// чекпоинта — иначе `resume` продолжил бы сессию уже с включённым гейтом.
    #[test]
    fn risk_consent_survives_checkpoint_round_trip() {
        let mut plan = cfg(QUICK_PRESET, 5.0);
        plan.accept_dirty_background = true;
        let back: SessionConfig =
            serde_json::from_str(&serde_json::to_string(&plan).expect("в json")).expect("из json");
        assert!(back.accept_dirty_background);
    }

    #[test]
    fn quick_preset_values() {
        assert_eq!(
            QUICK_PRESET,
            Preset {
                duration_seconds: 30,
                warmup_seconds: 3,
                cooling_seconds: 3,
                repetitions: 1
            }
        );
    }

    /// Тяжёлая фаза пресета — это и есть тот замер, по которому сравнивают
    /// схемы. Слишком короткая фаза (3 с у прежних 9 с) не даёт сигнала,
    /// поэтому ниже фиксируем минимумы, а не «как получится».
    #[test]
    fn heavy_phase_is_long_enough_to_separate_schemes() {
        for (name, p, min_heavy) in [
            ("quick", QUICK_PRESET, 8),
            ("detailed", DETAILED_PRESET, 19),
        ] {
            let d = phase_durations(p.duration_seconds);
            assert!(
                d.heavy_seconds >= min_heavy,
                "{name}: тяжёлая фаза {} с, ожидалось не меньше {min_heavy}",
                d.heavy_seconds
            );
        }
    }

    /// При одном повторе доверительный интервал не считается (k − 1 = 0), и
    /// Контракт пресетов по числу повторов: детальный обязан считать
    /// доверительный интервал, быстрый — сознательно не считает (режим
    /// «Скрининг»: один прогон, k − 1 = 0, интервала не существует).
    #[test]
    fn only_detailed_preset_allips_a_confidence_interval() {
        // Константы проверяются на неизменность: `clippy` справедливо ругается
        // на сравнение с литералом, но смысл теста именно в «не отвлекаемся».
        #[allow(clippy::assertions_on_constants)]
        {
            assert!(
                DETAILED_PRESET.repetitions >= 3,
                "детальный: {} повторов — интервал шумный",
                DETAILED_PRESET.repetitions
            );
            assert_eq!(
                QUICK_PRESET.repetitions, 1,
                "быстрый режим обязан остаться скринингом, иначе он обещает \
                 доверительный интервал, которого не существует"
            );
        }
    }

    /// Адаптивная остановка обязана быть практичной у детального пресета:
    /// подсказка под полем повторов обещает её пользователю.
    #[test]
    fn detailed_preset_allips_early_stop() {
        // earlyStopNeed повторяет формулу подсказки в интерфейсе:
        // 2*sqrt(2)*t(0.975, k-1)*cv*100 / sqrt(k), cv = 5 %.
        let need = |k: u64| {
            let t = [12.706, 4.303, 3.182, 2.776][(k as usize - 2).min(3)];
            (2.0 * 2f64.sqrt() * t * 0.05 * 100.0) / (k as f64).sqrt()
        };
        assert!(
            need(DETAILED_PRESET.repetitions as u64) <= 20.0,
            "нужный перевес {} % — ранняя остановка почти не срабатывает",
            need(DETAILED_PRESET.repetitions as u64)
        );
    }

    #[test]
    fn validation_rejects_out_of_range_values() {
        let mut c = cfg(DETAILED_PRESET, 5.0);
        assert!(validate_config(&c).is_none());

        c.duration_seconds = 8;
        assert!(validate_config(&c).is_some());
        c.duration_seconds = 30;

        c.warmup_seconds = 1;
        assert!(validate_config(&c).is_some());
        c.warmup_seconds = 6;

        c.cooling_seconds = 61;
        assert!(validate_config(&c).is_some());
        c.cooling_seconds = 5;

        c.repetitions = 10;
        assert!(validate_config(&c).is_some());
        c.repetitions = 3;

        c.background_threshold_percent = 200.0;
        assert!(validate_config(&c).is_some());
        c.background_threshold_percent = 5.0;

        c.scheme_ids.clear();
        assert!(validate_config(&c).is_some());
    }

    #[test]
    fn phase_division_follows_the_spec_formula() {
        // Минимум 16 с: по 4 с на фазу.
        let p16 = phase_durations(16);
        assert_eq!(
            p16,
            PhaseDurations {
                light_seconds: 4,
                partial_seconds: 4,
                heavy_seconds: 4,
                response_seconds: 4
            }
        );
        // 45 с: extra 29 → light 4+8=12, partial 4+5=9, heavy 4+10=14,
        // «Отклик» добирает остаток 10.
        let p45 = phase_durations(45);
        assert_eq!(
            p45,
            PhaseDurations {
                light_seconds: 12,
                partial_seconds: 9,
                heavy_seconds: 14,
                response_seconds: 10
            }
        );
        // Ни одна фаза не короче 4 с и сумма равна total (для total ≥
        // MIN_DURATION_SECONDS, иначе значение поднимается до минимума).
        // Значения ниже минимума — обязательная часть проверки: раньше здесь
        // стояло «total ≥ 9», то есть остатки модели трёх фаз по 3 с, которых в
        // коде нет уже давно (четыре фазы, минимум 16 с).
        for total in [0u64, 1, 9, 15, 16, 20, 30, 45, 60, 120] {
            let d = phase_durations(total);
            let sum = d.light_seconds + d.partial_seconds + d.heavy_seconds + d.response_seconds;
            assert_eq!(sum, total.max(MIN_DURATION_SECONDS), "total={total}");
            for s in [
                d.light_seconds,
                d.partial_seconds,
                d.heavy_seconds,
                d.response_seconds,
            ] {
                assert!(s >= 4, "total={total}: фаза короче 4 с ({s})");
            }
        }
        assert_eq!(phase_durations(1), phase_durations(MIN_DURATION_SECONDS));
    }

    #[test]
    fn round_rotation_rotates_and_reverses_even_cycles() {
        let ids: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        // cycle 0 (четный): исходный, shift = round mod 3.
        assert_eq!(round_order(&ids, 0), vec!["a", "b", "c"]);
        assert_eq!(round_order(&ids, 1), vec!["b", "c", "a"]);
        assert_eq!(round_order(&ids, 2), vec!["c", "a", "b"]);
        // cycle 1 (нечётный): перевёрнутый список [c,b,a].
        assert_eq!(round_order(&ids, 3), vec!["c", "b", "a"]);
        assert_eq!(round_order(&ids, 4), vec!["b", "a", "c"]);
        assert_eq!(round_order(&ids, 5), vec!["a", "c", "b"]);
        // За полный период каждый ID появляется на каждой позиции ровно раз.
        let all: Vec<Vec<String>> = (0..6).map(|r| round_order(&ids, r)).collect();
        for id in &ids {
            let mut positions: Vec<usize> = Vec::new();
            for order in &all {
                positions.push(order.iter().position(|x| x == id).unwrap());
            }
            positions.sort();
            positions.dedup();
            assert_eq!(
                positions.len(),
                ids.len(),
                "{id} должен побывать на каждой позиции"
            );
        }
    }

    #[test]
    fn run_key_format_is_round_colon_plan() {
        assert_eq!(run_key(2, "guid-1"), "2:guid-1");
    }

    #[test]
    fn empty_scheme_list_round_stays_empty() {
        assert!(round_order(&[], 0).is_empty());
    }
}
