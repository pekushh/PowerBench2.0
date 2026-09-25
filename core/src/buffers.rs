//! Предвыделенные буферы сущностей.
//!
//! Создаются один раз из Seed и переиспользуются всегда. Распределение задач по
//! диапазонам записи не пересекается по индексам, что позволяет потокам ядра
//! работать с разными диапазонами одного массива без гонок (инварианты из
//! спецификации: задача j владеет слотом j и своим диапазоном анимации).

use crate::config::{ENTITY_CAPACITY, SEED};
use crate::prng::{XorShift64, unit_signed};

/// Буферы сущностей: позиции, скорости, флаги, глубина, анимация и
/// предвычисленный порядок обхода (Fisher-Yates от Seed).
#[derive(Debug)]
pub struct EntityBuffers {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub z: Vec<f64>,
    pub vx: Vec<f64>,
    pub vy: Vec<f64>,
    pub vz: Vec<f64>,
    pub flags: Vec<u64>,
    pub depth: Vec<f64>,
    pub anim: Vec<f64>,
    /// Предвычисленный порядок обхода сущностей (перемешанный Fisher-Yates).
    pub order: Vec<u32>,
}

/// Обёртка разделяемого доступа к буферам: инварианты неразделяемого записи
/// по диапазонам соблюдаются алгоритмом тика (см. модуль выше).
///
/// # Безопасность
///
/// `UnsafeCell` даёт сырой доступ на запись из нескольких потоков. Корректность
/// гарантируется исключительно непересекающимися диапазонами записи:
/// - главный поток пишет только x/y/z/vx/vy/vz/flags;
/// - воркер j пишет только anim в своём выделенном диапазоне (задача j);
/// - слоты контрольных сумм задач отделены от буферов сущностей.
///
/// Правки буферов не должны нарушать эти неизменяемые инварианты.
#[derive(Debug)]
pub struct RawShared<T> {
    inner: std::cell::UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for RawShared<T> {}

impl<T> RawShared<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: std::cell::UnsafeCell::new(value),
        }
    }

    /// Сырой указатель на содержимое (для доступа внутри ядра).
    #[inline]
    pub fn get(&self) -> *mut T {
        self.inner.get()
    }
}

/// Нормализация значения PRNG в [0, 1]: `((v >> 11) & 0xFFFF) / 65535.0`.
#[inline]
fn normalized(v: u64) -> f64 {
    ((v >> 11) & 0xFFFF) as f64 / 65_535.0
}

impl EntityBuffers {
    /// Заполнить буферы детерминированно из Seed.
    fn fill(&mut self, seed: u64) {
        let mut rng = XorShift64::new(seed);
        for i in 0..ENTITY_CAPACITY {
            self.x[i] = unit_signed(rng.next_u64());
            self.y[i] = unit_signed(rng.next_u64());
            self.z[i] = unit_signed(rng.next_u64());
            self.vx[i] = unit_signed(rng.next_u64());
            self.vy[i] = unit_signed(rng.next_u64());
            self.vz[i] = unit_signed(rng.next_u64());
            self.flags[i] = rng.next_u64();
            self.depth[i] = normalized(rng.next_u64());
            self.anim[i] = normalized(rng.next_u64());
        }
    }

    /// Создать буферы из Seed (выделение памяти происходит один раз здесь).
    pub fn from_seed(seed: u64) -> Self {
        let mut b = Self {
            x: vec![0.0; ENTITY_CAPACITY],
            y: vec![0.0; ENTITY_CAPACITY],
            z: vec![0.0; ENTITY_CAPACITY],
            vx: vec![0.0; ENTITY_CAPACITY],
            vy: vec![0.0; ENTITY_CAPACITY],
            vz: vec![0.0; ENTITY_CAPACITY],
            flags: vec![0u64; ENTITY_CAPACITY],
            depth: vec![0.0; ENTITY_CAPACITY],
            anim: vec![0.0; ENTITY_CAPACITY],
            order: Vec::with_capacity(ENTITY_CAPACITY),
        };
        b.order.extend(0..ENTITY_CAPACITY as u32);
        b.fill(seed);
        let mut rng = XorShift64::new(seed);
        // Fisher-Yates: случайность ТОЛЬКО из PRNG.
        for i in (1..ENTITY_CAPACITY).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            b.order.swap(i, j);
        }
        b
    }

    /// Reset: вернуть буферы к состоянию, построенному из Seed
    /// (детерминированное перезаполнение по PRNG).
    pub fn reset(&mut self) {
        self.fill(SEED);
        self.order.clear();
        self.order.extend(0..ENTITY_CAPACITY as u32);
        let mut rng = XorShift64::new(SEED);
        for i in (1..ENTITY_CAPACITY).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            self.order.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SEED;

    #[test]
    fn reset_restores_seed_state() {
        let _g = crate::tests::lock();
        let mut a = EntityBuffers::from_seed(SEED);
        let b = EntityBuffers::from_seed(SEED);
        // Мутируем a, затем reset — состояния обязаны совпасть.
        for i in 0..64 {
            a.x[i] = 123.0;
            a.flags[i] = 0xDEAD_BEEF;
            a.anim[i] = 0.5;
        }
        a.reset();
        assert_eq!(a.x, b.x);
        assert_eq!(a.y, b.y);
        assert_eq!(a.z, b.z);
        assert_eq!(a.vx, b.vx);
        assert_eq!(a.vy, b.vy);
        assert_eq!(a.vz, b.vz);
        assert_eq!(a.flags, b.flags);
        assert_eq!(a.depth, b.depth);
        assert_eq!(a.anim, b.anim);
        assert_eq!(a.order, b.order);
    }

    #[test]
    fn order_is_a_permutation() {
        let _g = crate::tests::lock();
        let b = EntityBuffers::from_seed(SEED);
        let mut seen = vec![false; ENTITY_CAPACITY];
        for &idx in &b.order {
            assert!(idx < ENTITY_CAPACITY as u32);
            let slot = &mut seen[idx as usize];
            assert!(!*slot, "индекс {idx} повторяется");
            *slot = true;
        }
        assert!(seen.iter().all(|&s| s));
    }
}
