//! Параметры спецификации GamingCpuV1 — неизменяемые константы.
//!
//! Все значения зафиксированы дословно в спецификации. Менять их запрещено:
//! они влияют на хэш конфигурации, контрольные суммы и сопоставимость результатов.

use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// Версия ядра нагрузки.
pub const VERSION: &str = "GamingCpuV1";

/// Seed генератора случайных чисел (u64, 16-рично).
pub const SEED: u64 = 0xC52A202600000001;

/// Максимум сущностей.
pub const ENTITY_CAPACITY: usize = 131_072;

/// Тиков в суперцикле фазы «Отклик».
pub const RESPONSE_SUPERCYCLE: u64 = 256;

/// Максимум задач воркерам на тик.
pub const MAXIMUM_JOBS: usize = 64;

/// Базовое значение цепочки контрольных сумм (FNV-подобное).
pub const HASH_OFFSET: u64 = 14_695_981_039_346_656_037;

/// Простое число цепочки контрольных сумм.
pub const HASH_PRIME: u64 = 1_099_511_628_211;

/// Оценочный максимум тиков в секунду (для расчёта буфера сэмплов).
pub const ESTIMATED_MAX_TICKS_PER_SEC: u64 = 32_000;

/// Фиксированный шаг симуляции внутри тика (детерминированный; спецификация
/// не фиксирует величину — выбрана константа «как 60 кадров игры»).
pub const DT_SECONDS: f64 = 1.0 / 60.0;

/// Минимальная ёмкость буфера сэмплов.
pub const MIN_SAMPLE_CAPACITY: usize = 4_096;

/// Максимальная ёмкость буфера сэмплов.
pub const MAX_SAMPLE_CAPACITY: usize = 4_000_000;

/// Профили нагрузки (объём работы на ОДИН тик).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Profile {
    Light,
    /// Частичная нагрузка: половина воркеров занята.
    Partial,
    Heavy,
    ResponseBase,
    ResponseMedium,
    ResponseMajor,
}

impl Profile {
    /// Индекс профиля как usize (для таблиц эталонных сумм).
    pub const fn index(self) -> usize {
        match self {
            Profile::Light => 0,
            Profile::Partial => 1,
            Profile::Heavy => 2,
            Profile::ResponseBase => 3,
            Profile::ResponseMedium => 4,
            Profile::ResponseMajor => 5,
        }
    }
}

/// Параметры одного профиля на один тик.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProfileParams {
    pub main_entity_updates: usize,
    pub visibility_probes: usize,
    pub animation_items: usize,
    pub worker_jobs: usize,
}

/// Параметры профиля — константы версии.
pub const fn profile_params(p: Profile) -> ProfileParams {
    match p {
        Profile::Light => ProfileParams {
            main_entity_updates: 8_192,
            visibility_probes: 32_768,
            animation_items: 8_192,
            worker_jobs: 16,
        },
        // Половина работы профиля Heavy при половине занятых ядер: половина
        // воркеров получает задачи, вторая парковка простаивает.
        Profile::Partial => ProfileParams {
            main_entity_updates: 24_576,
            visibility_probes: 98_304,
            animation_items: 24_576,
            worker_jobs: 32,
        },
        Profile::Heavy => ProfileParams {
            main_entity_updates: 32_768,
            visibility_probes: 131_072,
            animation_items: 32_768,
            worker_jobs: 64,
        },
        Profile::ResponseBase => ProfileParams {
            main_entity_updates: 16_384,
            visibility_probes: 65_536,
            animation_items: 16_384,
            worker_jobs: 32,
        },
        Profile::ResponseMedium => ProfileParams {
            main_entity_updates: 24_576,
            visibility_probes: 98_304,
            animation_items: 24_576,
            worker_jobs: 48,
        },
        Profile::ResponseMajor => ProfileParams {
            main_entity_updates: 32_768,
            visibility_probes: 131_072,
            animation_items: 32_768,
            worker_jobs: 64,
        },
    }
}

/// Измеряемая фаза сценария.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Light,
    /// Половина пула занята: режим, где различие схем питания максимально.
    Partial,
    Heavy,
    Response,
}

/// Канонический порядок фаз: он же порядок индексов в отчёте и в JSON.
pub const PHASE_ORDER: [Phase; 4] = [Phase::Light, Phase::Partial, Phase::Heavy, Phase::Response];

impl Phase {
    /// Профиль первого тика фазы (после reset индекс тика в фазе равен 0).
    pub const fn first_profile(self) -> Profile {
        match self {
            Phase::Light => Profile::Light,
            Phase::Partial => Profile::Partial,
            Phase::Heavy => Profile::Heavy,
            Phase::Response => Profile::ResponseBase,
        }
    }

    /// Индекс фазы — он же порядковый номер в `StoredRun::phases`.
    pub const fn index(self) -> u8 {
        match self {
            Phase::Light => 0,
            Phase::Partial => 1,
            Phase::Heavy => 2,
            Phase::Response => 3,
        }
    }

    /// Фаза по индексу; `None` — индекс неизвестен (старый или чужой JSON).
    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Phase::Light),
            1 => Some(Phase::Partial),
            2 => Some(Phase::Heavy),
            3 => Some(Phase::Response),
            _ => None,
        }
    }

    /// Подпись фазы в интерфейсе и отчёте.
    pub const fn label(self) -> &'static str {
        match self {
            Phase::Light => "Лёгкая",
            Phase::Partial => "Частичная",
            Phase::Heavy => "Тяжёлая",
            Phase::Response => "Отклик",
        }
    }

    /// Доля занятых воркеров в этой фазе (в долях единицы, знаменатель 100).
    ///
    /// Только фаза частичной нагрузки занимает половину пула. Остальные фазы
    /// держат все воркеры: различать схемы питания имеет смысл и при полной
    /// загрузке, а вот *средняя* нагрузка — это как раз тот режим, где
    /// различие между схемами максимально (минимальное состояние процессора,
    /// агрессивность разгона, политика охлаждения проявляются сильнее всего).
    /// Подробнее — в `Phase::PARTIAL_WORKER_PERCENT`.
    pub const fn active_worker_percent(self) -> u32 {
        match self {
            Phase::Partial => Phase::PARTIAL_WORKER_PERCENT,
            _ => 100,
        }
    }

    /// Процент воркеров, занятых в фазе частичной нагрузки.
    pub const PARTIAL_WORKER_PERCENT: u32 = 50;
}

/// Тип тика фазы «Отклик» определяется ТОЛЬКО индексом тика внутри фазы
/// (`index mod 256`): 63/127/191 → ResponseMedium, 255 → ResponseMajor,
/// все прочие → ResponseBase.
pub const fn response_profile(tick_index_in_phase: u64) -> Profile {
    match tick_index_in_phase % RESPONSE_SUPERCYCLE {
        63 | 127 | 191 => Profile::ResponseMedium,
        255 => Profile::ResponseMajor,
        _ => Profile::ResponseBase,
    }
}

/// Payload хэша конфигурации (UTF-8, без разделителей тысяч, инвариантная
/// культура): `Version|Seed_HEX16|EntityCapacity|Supercycle|` затем шесть
/// профилей (Light, Partial, Heavy, ResponseBase, ResponseMedium,
/// ResponseMajor), каждый — `MainEntityUpdates,VisibilityProbes,AnimationItems,WorkerJobs`.
///
/// Профиль `Partial` добавлен вместе с фазой частичной нагрузки, поэтому
/// `config_hash` изменился: сессии до и после этого честно считаются
/// несопоставимыми, а не сравниваются как будто мерили то же самое.
pub const CONFIG_PAYLOAD: &str = "GamingCpuV1|C52A202600000001|131072|256|8192,32768,8192,16|24576,98304,24576,32|32768,131072,32768,64|16384,65536,16384,32|24576,98304,24576,48|32768,131072,32768,64";

/// SHA-256 от payload конфигурации, hex-строка ВЕРХНИМ регистром.
/// Вычисляется один раз и кэшируется.
pub fn config_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let digest = Sha256::digest(CONFIG_PAYLOAD.as_bytes());
        let mut hex = String::with_capacity(64);
        for b in digest {
            hex.push_str(&format!("{:02X}", b));
        }
        hex
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Критерий приёмки (в): расписание суперцикла «Отклика».
    #[test]
    fn supercycle_schedule_matches_spec() {
        let _g = crate::tests::lock();
        for i in 0..RESPONSE_SUPERCYCLE {
            let expected = match i {
                63 | 127 | 191 => Profile::ResponseMedium,
                255 => Profile::ResponseMajor,
                _ => Profile::ResponseBase,
            };
            assert_eq!(response_profile(i), expected, "индекс {i}");
        }
        // Сдвиг на суперциклы не меняет класс тика.
        for k in 0..4u64 {
            assert_eq!(response_profile(256 * k), Profile::ResponseBase);
            assert_eq!(response_profile(256 * k + 63), Profile::ResponseMedium);
            assert_eq!(response_profile(256 * k + 255), Profile::ResponseMajor);
        }
    }

    /// Профильные объёмы делятся нацело на число задач воркеров.
    #[test]
    fn profile_work_splits_evenly_between_jobs() {
        let _g = crate::tests::lock();
        for p in [
            Profile::Light,
            Profile::Heavy,
            Profile::ResponseBase,
            Profile::ResponseMedium,
            Profile::ResponseMajor,
        ] {
            let params = profile_params(p);
            assert!(params.worker_jobs > 0);
            assert_eq!(params.visibility_probes % params.worker_jobs, 0);
            assert_eq!(params.animation_items % params.worker_jobs, 0);
        }
    }

    #[test]
    fn config_hash_is_uppercase_hex_of_sha256() {
        let _g = crate::tests::lock();
        let digest = Sha256::digest(CONFIG_PAYLOAD.as_bytes());
        let mut expected = String::new();
        for b in digest {
            expected.push_str(&format!("{:02X}", b));
        }
        assert_eq!(config_hash(), &expected);
        assert_eq!(config_hash().len(), 64);
    }
}
