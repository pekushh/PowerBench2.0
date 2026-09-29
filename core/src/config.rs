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

/// Все профили нагрузки в порядке индексов.
///
/// Единственный список на workspace: пока профили перечислялись вручную в
/// тестах, профиль «Частичная» в них не попал — а именно на нём был баг с
/// раздачей задач, который проверки инвариантов должны были поймать.
pub const ALL_PROFILES: [Profile; 6] = [
    Profile::Light,
    Profile::Partial,
    Profile::Heavy,
    Profile::ResponseBase,
    Profile::ResponseMedium,
    Profile::ResponseMajor,
];

impl Profile {
    /// Порядковый номер профиля (совпадает с позицией в [`ALL_PROFILES`]).
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

/// Затухание скорости сущностей за тик и шаг разгона.
pub const VELOCITY_DAMPING: f64 = 0.999;
pub const VELOCITY_STEP: f64 = 0.000_5;
/// Шаг анимации за тик (то же значение, что и в пуле).
pub const ANIMATION_STEP: f64 = 0.000_1;
/// Знаменатель нормализации PRNG в [0, 1].
pub const PRNG_NORMALIZE: f64 = 65535.0;
/// Множитель XorShift64.
pub const PRNG_MULTIPLIER: u64 = 0x2545F4914F6CDD1D;
/// Константа смешивания в задачах видимости.
pub const VISIBILITY_MIX_CONSTANT: u64 = 0x9E37_79B9;
/// Константа `finalize_tick` и сдвиг в нём.
pub const FINALIZE_TICK_CONSTANT: u64 = 0x9E37_79B9_7F4A_7C15;
pub const FINALIZE_TICK_ROTATE: u32 = 17;

/// Payload для хэша конфигурации (UTF-8, порядок фиксирован): версия, seed,
/// ёмкость, суперцикл, параметры всех профилей и **все константы, влияющие на
/// контрольные суммы**.
///
/// Раньше это был строковый литерал, и в него попадали не все константы:
/// `DT_SECONDS`, множители хэша, константы `finalize_tick`, множитель PRNG,
/// нормализация и коэффициенты затухания в него не входили. Правка любой из них
/// не меняла `config_hash`, и несопоставимые прогоны получали сообщение
/// «контрольная сумма различается между повторами» вместо «изменилась
/// конфигурация нагрузки». Теперь payload собирается из самих констант, и
/// разъехаться с ними он не может.
pub fn config_payload() -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(320);
    let _ = write!(s, "GamingCpuV1|{SEED:016X}|{ENTITY_CAPACITY}|{RESPONSE_SUPERCYCLE}");
    for p in ALL_PROFILES {
        let params = profile_params(p);
        let _ = write!(
            s,
            "|{},{},{},{}",
            params.main_entity_updates,
            params.visibility_probes,
            params.animation_items,
            params.worker_jobs
        );
    }
    let _ = write!(s, "|mix:{HASH_OFFSET}:{HASH_PRIME}");
    let _ = write!(s, "|vis:{VISIBILITY_MIX_CONSTANT}");
    let _ = write!(s, "|fin:{FINALIZE_TICK_CONSTANT}:{FINALIZE_TICK_ROTATE}");
    let _ = write!(s, "|prng:{PRNG_MULTIPLIER}:{PRNG_NORMALIZE:.1}");
    let _ = write!(s, "|dt:{DT_SECONDS:.12}");
    let _ = write!(
        s,
        "|vel:{VELOCITY_DAMPING:.6}:{VELOCITY_STEP:.6}:{ANIMATION_STEP:.6}"
    );
    s
}

/// SHA-256 от payload конфигурации, hex-строка ВЕРХНИМ регистром.
/// Вычисляется один раз и кэшируется.
pub fn config_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let digest = Sha256::digest(config_payload().as_bytes());
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
        for p in ALL_PROFILES {
            let params = profile_params(p);
            assert!(params.worker_jobs > 0);
            assert_eq!(params.visibility_probes % params.worker_jobs, 0);
            assert_eq!(params.animation_items % params.worker_jobs, 0);
        }
    }

    #[test]
    fn config_hash_is_uppercase_hex_of_sha256() {
        let _g = crate::tests::lock();
        let digest = Sha256::digest(config_payload().as_bytes());
        let mut expected = String::with_capacity(64);
        for b in digest {
            expected.push_str(&format!("{:02X}", b));
        }
        assert_eq!(config_hash(), &expected);
        assert_eq!(config_hash().len(), 64);
    }

    /// Payload обязан содержать каждую константу, влияющую на контрольные суммы.
    ///
    /// Строковый литерал payload раньше их не содержал, и правка константы не
    /// меняла `config_hash`: несопоставимые прогоны получали диагноз «контрольная
    /// сумма различается между повторами» вместо «изменилась конфигурация
    /// нагрузки». Тест ловит и удаление сегмента, и подмену его значения.
    #[test]
    fn config_payload_covers_checksum_constants() {
        let _g = crate::tests::lock();
        let payload = config_payload();
        for (name, value) in [
            ("mix offset", HASH_OFFSET.to_string()),
            ("mix prime", HASH_PRIME.to_string()),
            ("visibility", VISIBILITY_MIX_CONSTANT.to_string()),
            ("finalize constant", FINALIZE_TICK_CONSTANT.to_string()),
            ("finalize rotate", FINALIZE_TICK_ROTATE.to_string()),
            ("prng multiplier", PRNG_MULTIPLIER.to_string()),
            ("prng normalize", format!("{PRNG_NORMALIZE:.1}")),
            ("velocity damping", format!("{VELOCITY_DAMPING:.6}")),
            ("velocity step", format!("{VELOCITY_STEP:.6}")),
            ("animation step", format!("{ANIMATION_STEP:.6}")),
            ("dt", format!("{DT_SECONDS:.12}")),
            ("seed", format!("{SEED:016X}")),
            ("entity capacity", ENTITY_CAPACITY.to_string()),
            ("supercycle", RESPONSE_SUPERCYCLE.to_string()),
        ] {
            assert!(
                payload.contains(&value),
                "payload не содержит {name} = {value}: {payload}"
            );
        }
        // Профиль «Частичная» тоже обязан попасть в payload: без него прогоны
        // разных фаз смешивались бы при неизменном config_hash.
        assert!(
            payload.contains(&format!("{},{}", {
                let p = profile_params(Profile::Partial);
                p.main_entity_updates
            }, {
                let p = profile_params(Profile::Partial);
                p.worker_jobs
            })),
            "payload не содержит параметров профиля «Частичная»"
        );
    }

    /// Хэш обязан быть функцией payload: иначе две сборки с разными константами
    /// выдали бы один и тот же хэш конфигурации.
    #[test]
    fn config_hash_tracks_payload() {
        let _g = crate::tests::lock();
        let first = config_hash().to_string();
        assert_eq!(first, config_hash());
        let mut altered = config_payload();
        altered.push('x');
        let a = Sha256::digest(altered.as_bytes());
        let b = Sha256::digest(config_payload().as_bytes());
        assert_ne!(a[..], b[..], "хэш не зависит от payload");
    }
}
