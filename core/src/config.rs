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
            Profile::Heavy => 1,
            Profile::ResponseBase => 2,
            Profile::ResponseMedium => 3,
            Profile::ResponseMajor => 4,
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
    Heavy,
    Response,
}

impl Phase {
    /// Профиль первого тика фазы (после reset индекс тика в фазе равен 0).
    pub const fn first_profile(self) -> Profile {
        match self {
            Phase::Light => Profile::Light,
            Phase::Heavy => Profile::Heavy,
            Phase::Response => Profile::ResponseBase,
        }
    }
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
/// культура): `Version|Seed_HEX16|EntityCapacity|Supercycle|` затем пять
/// профилей (Light, Heavy, ResponseBase, ResponseMedium, ResponseMajor),
/// каждый — `MainEntityUpdates,VisibilityProbes,AnimationItems,WorkerJobs`.
pub const CONFIG_PAYLOAD: &str =
    "GamingCpuV1|C52A202600000001|131072|256|8192,32768,8192,16|32768,131072,32768,64|16384,65536,16384,32|24576,98304,24576,48|32768,131072,32768,64";

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