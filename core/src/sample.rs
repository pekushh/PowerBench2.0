//! Буфер сэмплов времени тиков.
//!
//! Ёмкость: `ceil(длительность_фазы_сек * 32000)`, ограничена снизу 4096 и
//! сверху 4 000 000; для фазы «Отклик» округляется ВВЕРХ до кратности 256
//! (но не выше 4 000 000). Заполнение полного буфера раньше конца фазы — ошибка
//! `SampleCapacityReached`, а не тихое обрезание.

use crate::config::{ESTIMATED_MAX_TICKS_PER_SEC, Phase};
use crate::config::{MAX_SAMPLE_CAPACITY, MIN_SAMPLE_CAPACITY, RESPONSE_SUPERCYCLE};

/// Буфер сэмплов времени тиков в миллисекундах.
#[derive(Debug)]
pub struct SampleBuffer {
    data: Vec<f64>,
    len: usize,
}

impl SampleBuffer {
    /// Создать буфер под заданную длительность фазы.
    pub fn new(phase: Phase, duration_secs: u64) -> Self {
        Self::with_capacity(capacity_for(phase, duration_secs))
    }

    /// Создать буфер с явной ёмкостью (например, под прогон по числу тиков).
    pub fn with_capacity(capacity: usize) -> Self {
        let mut data = Vec::with_capacity(capacity);
        data.resize(capacity, 0.0);
        Self { data, len: 0 }
    }

    /// Записать время тика (выделений нет: слот предвыделен).
    #[inline]
    pub fn push(&mut self, ms: f64) {
        debug_assert!(self.len < self.data.len(), "буфер сэмплов переполнен");
        self.data[self.len] = ms;
        self.len += 1;
    }

    /// Число записанных сэмплов.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Буфер пуст.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Записанные сэмплы как срез (для сбора статистики после прогона).
    #[inline]
    pub fn as_slice(&self) -> &[f64] {
        &self.data[..self.len]
    }

    /// Ёмкость буфера.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    /// Буфер заполнен.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len >= self.data.len()
    }

    /// Очистить буфер под новый измеряемый прогон (без выделений).
    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

/// Ёмкость буфера сэмплов под длительность фазы (правила спецификации).
///
/// Умножение насыщающее: `duration_secs * 32000` переполнялось на больших
/// значениях (паника в debug, wrap в release с последующим неверным размером
/// буфера). Верхняя граница и так достигается гораздо раньше переполнения.
pub fn capacity_for(phase: Phase, duration_secs: u64) -> usize {
    let max_secs = (MAX_SAMPLE_CAPACITY as u64) / ESTIMATED_MAX_TICKS_PER_SEC + 2;
    let seconds = duration_secs.min(max_secs);
    let raw = seconds.saturating_mul(ESTIMATED_MAX_TICKS_PER_SEC);
    let clamped = (raw as usize).clamp(MIN_SAMPLE_CAPACITY, MAX_SAMPLE_CAPACITY);
    if phase == Phase::Response {
        // Округление ВВЕРХ до кратности 256, но не выше максимума.
        let up = clamped.div_ceil(RESPONSE_SUPERCYCLE as usize) * RESPONSE_SUPERCYCLE as usize;
        up.min(MAX_SAMPLE_CAPACITY)
    } else {
        clamped
    }
}

/// Ёмкость буфера, достаточная для `ticks` тиков (для прогонов по числу тиков).
pub fn capacity_for_ticks(phase: Phase, ticks: u64) -> usize {
    if ticks == 0 {
        return MIN_SAMPLE_CAPACITY;
    }
    let ticks = ticks.min(MAX_SAMPLE_CAPACITY as u64);
    let seconds = ticks.div_ceil(ESTIMATED_MAX_TICKS_PER_SEC).max(1);
    let c = capacity_for(phase, seconds);
    // Гарантия: capacity >= ticks (за счёт оценки сверху 32000/+1 секунды).
    debug_assert!(c as u64 >= ticks);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_clamped_and_rounded_for_response() {
        let _g = crate::tests::lock();
        // Режим Response: округление вверх до 256.
        let c1 = capacity_for(Phase::Response, 0);
        assert_eq!(c1, MIN_SAMPLE_CAPACITY);
        let small = capacity_for(Phase::Response, 1);
        assert_eq!(small % 256, 0);
        assert!(small >= 32000);
        // Не-Response фазы: просто зажатые в [4096, 4 000 000].
        assert_eq!(capacity_for(Phase::Light, 1), 32_000);
        assert_eq!(capacity_for(Phase::Light, 0), MIN_SAMPLE_CAPACITY);
        // Верхняя граница не превышается.
        assert_eq!(capacity_for(Phase::Heavy, 1_000_000), MAX_SAMPLE_CAPACITY);
        assert!(capacity_for(Phase::Response, 1_000_000) <= MAX_SAMPLE_CAPACITY);
    }

    /// Регресс: огромные длительности не должны переполнять расчёт ёмкости.
    #[test]
    fn capacity_survives_absurd_durations() {
        let _g = crate::tests::lock();
        for secs in [u64::MAX, u64::MAX / 2, 1 << 40, 10_000_000] {
            let c = capacity_for(Phase::Light, secs);
            assert_eq!(c, MAX_SAMPLE_CAPACITY, "секунд={secs}");
            let r = capacity_for(Phase::Response, secs);
            assert!(r <= MAX_SAMPLE_CAPACITY);
        }
        // По числу тиков — тоже без паники.
        let c = capacity_for_ticks(Phase::Response, u64::MAX);
        assert!(c <= MAX_SAMPLE_CAPACITY);
    }

    #[test]
    fn capacity_for_ticks_is_sufficient() {
        let _g = crate::tests::lock();
        for (phase, ticks) in [
            (Phase::Light, 4096u64),
            (Phase::Heavy, 4097),
            (Phase::Response, 4097),
            (Phase::Response, 256),
            (Phase::Light, 100_000),
            (Phase::Response, 100_001),
        ] {
            let c = capacity_for_ticks(phase, ticks);
            assert!(c >= ticks as usize, "phase={phase:?} ticks={ticks} cap={c}");
            if phase == Phase::Response {
                assert_eq!(c % 256, 0);
            }
        }
    }
}
