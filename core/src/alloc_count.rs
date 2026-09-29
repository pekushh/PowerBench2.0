//! Счётчик аллокаций ядра нагрузочного цикла.
//!
//! Счётчик включается только тестом крейта: аллокатор ставится под
//! #[cfg(test)] в lib.rs, и в обычной сборке COUNT всегда ноль. Поэтому
//! проверка в powerbench-cli, которая читает COUNT, проходит вакуумно и
//! ничего не говорит об аллокациях в цикле тика: в релизной сборке там стоит
//! System. Проверять ноль аллокаций нужно в тестах крейта, где аллокатор
//! действительно установлен.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Включено ли отслеживание на текущий момент.
pub static ENABLED: AtomicBool = AtomicBool::new(false);

/// Количество аллокаций в активном окне отслеживания.
pub static COUNT: AtomicUsize = AtomicUsize::new(0);

pub struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        track(ptr);
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        track(new_ptr);
        new_ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        track(ptr);
        ptr
    }
}

/// Учесть аллокацию, если отслеживание включено.
fn track(ptr: *mut u8) {
    if ENABLED.load(Ordering::Relaxed) && !ptr.is_null() {
        COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Инвариант тестовой среды: в тестовом бинаре установлен считающий аллокатор.
    #[test]
    fn counting_allocator_is_active() {
        let _g = crate::tests::lock();
        ENABLED.store(true, Ordering::Relaxed);
        COUNT.store(0, Ordering::Relaxed);
        let _ = Vec::<u8>::with_capacity(16);
        assert!(
            COUNT.load(Ordering::Relaxed) >= 1,
            "глобальный аллокатор не зарегистрирован"
        );
        ENABLED.store(false, Ordering::Relaxed);
    }
}
