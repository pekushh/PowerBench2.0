//! Детерминированный генератор случайных чисел xorshift64* — единственный
//! источник случайности ядра. Внутри тика запрещены любые иные источники.

/// xorshift64*: единственный источник случайности, инициализируется от Seed.
pub struct XorShift64 {
    state: u64,
}

const MULTIPLIER: u64 = 0x2545F4914F6CDD1D;

impl XorShift64 {
    /// Создать генератор от заданного seed.
    #[inline]
    pub fn new(state: u64) -> Self {
        Self { state }
    }

    /// Следующее 64-битное значение.
    #[inline]
    pub fn next(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(MULTIPLIER)
    }
}

/// `UnitBits(v) = (v >> 11) & 0x001F_FFFF_FFFF_FFFF` — 53 бита.
#[inline]
pub const fn unit_bits(v: u64) -> u64 {
    (v >> 11) & 0x001F_FFFF_FFFF_FFFF
}

/// `UnitSigned(v) = UnitBits(v) * (1.0 / 2^52) - 1.0` — равномерно в [-1, 1).
#[inline]
pub fn unit_signed(v: u64) -> f64 {
    unit_bits(v) as f64 * (1.0 / 4_503_599_627_370_496.0) - 1.0
}

/// `WrapPosition(p)`: вернуть p в [-1, 1), циклически.
#[inline]
pub fn wrap_position(p: f64) -> f64 {
    if p > 1.0 {
        p - 2.0
    } else if p < -1.0 {
        p + 2.0
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_signed_is_in_range() {
        let _g = crate::tests::lock();
        for v in [0u64, 1, 0xFFFF_FFFF_FFFF_FFFF, u64::MAX] {
            let s = unit_signed(v);
            assert!(s >= -1.0 && s < 1.0, "unit_signed({v:#x}) = {s}");
        }
    }

    #[test]
    fn wrap_position_keeps_unit_range() {
        let _g = crate::tests::lock();
        for p in [-2.0, -1.1, -1.0, -0.5, 0.0, 0.5, 1.0, 1.1, 1.99, -1.99] {
            let w = wrap_position(p);
            assert!(
                w >= -1.0 && w <= 1.0,
                "wrap({p}) = {w} (одношаговый перенос, как в спецификации)"
            );
        }
        // Внутри диапазона значение не меняется.
        assert_eq!(wrap_position(0.25), 0.25);
        assert_eq!(wrap_position(-0.75), -0.75);
        assert_eq!(wrap_position(2.0), 0.0);
        assert_eq!(wrap_position(-2.0), 0.0);
    }

    #[test]
    fn prng_is_deterministic() {
        let _g = crate::tests::lock();
        let mut a = XorShift64::new(0xC52A202600000001);
        let mut b = XorShift64::new(0xC52A202600000001);
        for _ in 0..1000 {
            assert_eq!(a.next(), b.next());
        }
    }
}