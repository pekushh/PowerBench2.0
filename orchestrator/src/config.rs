//! Конфигурация сессии, пресеты, деление длительности на фазы и план раундов.

/// Пресет «Быстрый»: длительность 9 с, разогрев 2 с, охлаждение 1 с, повторов 1.
pub const QUICK_PRESET: Preset = Preset {
    duration_seconds: 9,
    warmup_seconds: 2,
    cooling_seconds: 1,
    repetitions: 1,
};

/// Пресет «Рекомендуемый» (значения по умолчанию): 30 с, 6 с, 5 с, повторов 3.
pub const DETAILED_PRESET: Preset = Preset {
    duration_seconds: 30,
    warmup_seconds: 6,
    cooling_seconds: 5,
    repetitions: 3,
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
    /// Длительность измеряемой части (сумма трёх фаз). Минимум 9 с.
    pub duration_seconds: u64,
    /// Разогрев профилем «Отклик». Минимум 2 с.
    pub warmup_seconds: u64,
    /// Пауза охлаждения между прогонами. 0..=60 с.
    pub cooling_seconds: u64,
    /// Число раундов (повторов). 1..=9.
    pub repetitions: u32,
    /// Порог фоновой нагрузки в % на ядро (по умолчанию 5.0; 0.5..=100).
    pub background_threshold_percent: f64,
    /// Число воркеров пула (None — значение по умолчанию движка).
    pub worker_count: Option<usize>,
    /// Идентификаторы выбранных схем (GUID), в порядке предпочтения пользователя.
    pub scheme_ids: Vec<String>,
    /// Идентификатор плана — попадает в ключи прогонов `"{round}:{plan-guid}"`.
    pub plan_guid: String,
}

/// Ограничения настроек (спецификация).
pub const MIN_DURATION_SECONDS: u64 = 9;
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

/// Деление длительности на измеряемые фазы (целочисленно, спецификация):
///
/// ```text
/// total = max(9, DurationSeconds);  extra = total - 9
/// light    = 3 + extra * 3 / 10
/// response = 3 + extra * 3 / 10
/// heavy    = total - light - response
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseDurations {
    pub light_seconds: u64,
    pub heavy_seconds: u64,
    pub response_seconds: u64,
}

/// Длительности измеряемых фаз по общей длительности.
pub const fn phase_durations(total: u64) -> PhaseDurations {
    let total = if total < MIN_DURATION_SECONDS {
        MIN_DURATION_SECONDS
    } else {
        total
    };
    let extra = total - MIN_DURATION_SECONDS;
    let side = MIN_DURATION_SECONDS / 3 + extra.saturating_mul(3) / 10;
    PhaseDurations {
        light_seconds: side,
        response_seconds: side,
        heavy_seconds: total - 2 * side,
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

    fn cfg(preset: Preset, threshold: f64) -> SessionConfig {
        SessionConfig {
            duration_seconds: preset.duration_seconds,
            warmup_seconds: preset.warmup_seconds,
            cooling_seconds: preset.cooling_seconds,
            repetitions: preset.repetitions,
            background_threshold_percent: threshold,
            worker_count: None,
            scheme_ids: vec!["a".to_string()],
            plan_guid: "plan-1".to_string(),
        }
    }

    #[test]
    fn default_preset_is_detailed() {
        assert_eq!(DETAILED_PRESET.repetitions, 3);
        assert_eq!(DETAILED_PRESET.duration_seconds, 30);
    }

    #[test]
    fn quick_preset_values() {
        assert_eq!(
            QUICK_PRESET,
            Preset {
                duration_seconds: 9,
                warmup_seconds: 2,
                cooling_seconds: 1,
                repetitions: 1
            }
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
        // 9 с: 3/3/3.
        let p9 = phase_durations(9);
        assert_eq!(
            p9,
            PhaseDurations {
                light_seconds: 3,
                heavy_seconds: 3,
                response_seconds: 3
            }
        );
        // 30 с: extra 21 → light=3+6=9, response=9, heavy=30-18=12.
        let p30 = phase_durations(30);
        assert_eq!(
            p30,
            PhaseDurations {
                light_seconds: 9,
                heavy_seconds: 12,
                response_seconds: 9
            }
        );
        // Сумма всегда равна total (и total максимуется до 9).
        for total in [0u64, 1, 9, 10, 15, 21, 30, 59, 60] {
            let d = phase_durations(total);
            let sum = d.light_seconds + d.heavy_seconds + d.response_seconds;
            assert_eq!(sum, total.max(MIN_DURATION_SECONDS), "total={total}");
        }
        assert_eq!(phase_durations(1), phase_durations(9));
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
