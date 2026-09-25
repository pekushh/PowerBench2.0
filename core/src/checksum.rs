//! Цепочка контрольных сумм (побитовое детерминированное смешивание).

use crate::config::{HASH_OFFSET, HASH_PRIME, SEED};

/// `Mix(hash, value) = (hash XOR value).wrapping_mul(HashPrime)`.
#[inline]
pub const fn mix(hash: u64, value: u64) -> u64 {
    (hash ^ value).wrapping_mul(HASH_PRIME)
}

/// Инициализация контрольной суммы запуска: `Mix(HashOffset, Seed)`.
#[inline]
pub const fn start_run_checksum() -> u64 {
    mix(HASH_OFFSET, SEED)
}

/// Финализация контрольной суммы тика — небольшой фиксированный пакет
/// Mix-операций над уже собранной суммой и индексом тика (детерминированный).
#[inline]
pub fn finalize_tick(checksum: u64, tick_index: u64) -> u64 {
    let mut c = checksum;
    c = mix(c, tick_index);
    c = c.rotate_left(17).wrapping_mul(HASH_PRIME);
    c = mix(c, tick_index ^ 0x9E37_79B9_7F4A_7C15);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_baseline_equals_mix() {
        let _g = crate::tests::lock();
        assert_eq!(start_run_checksum(), mix(HASH_OFFSET, SEED));
    }

    #[test]
    fn mix_is_deterministic() {
        let _g = crate::tests::lock();
        assert_eq!(mix(1, 2), mix(1, 2));
        assert_ne!(mix(1, 2), mix(4, 1));
    }
}
