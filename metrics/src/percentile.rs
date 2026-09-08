//! Перцентиль — единственный разрешённый метод: линейная интерполяция.

/// Перцентиль отсортированного по возрастанию массива.
///
/// Формула (спецификация, раздел «Перцентиль»):
/// `pos = p*(n-1); lower = floor(pos); upper = ceil(pos);`
/// если `lower == upper` → `sorted[lower]`, иначе линейная интерполяция.
///
/// Возвращает `NaN` для пустого массива.
pub fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return f64::NAN;
    }
    let pos = p * (n - 1) as f64;
    let lower = pos.floor();
    let upper = pos.ceil();
    if lower == upper {
        return sorted[lower as usize];
    }
    let li = lower as usize;
    let ui = upper as usize;
    sorted[li] + (sorted[ui] - sorted[li]) * (pos - lower)
}

/// Перцентиль произвольного массива (копирует и сортирует по возрастанию).
pub fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    percentile_sorted(&sorted, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Руками посчитанный набор: равномерная последовательность 1..10.
    #[test]
    fn percentile_is_linear_interpolation() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        // pos = 0.5*9 = 4.5 → sorted[4] + (sorted[5]-sorted[4])*0.5 = 6 - 0.5 = 5.5
        assert_eq!(percentile_sorted(&v, 0.5), 5.5);
        // pos = 0.25*9 = 2.25 → 3 + 1*0.25 = 3.25
        assert!((percentile_sorted(&v, 0.25) - 3.25).abs() < 1e-12);
        assert_eq!(percentile_sorted(&v, 0.0), 1.0);
        assert_eq!(percentile_sorted(&v, 1.0), 10.0);
        // Один элемент: pos = 0 независимо от p.
        assert_eq!(percentile_sorted(&[7.0], 0.37), 7.0);
        // Два элемента: p = 0.5 → pos = 0.5 → 10 + 10*0.5 = 15.
        assert_eq!(percentile_sorted(&[10.0, 20.0], 0.5), 15.0);
        // Несортированный вход — функция сама сортирует.
        assert_eq!(percentile(&[10.0, 1.0, 7.0], 0.5), 7.0);
        // Пустой массив.
        let empty: &[f64] = &[];
        assert!(percentile_sorted(empty, 0.5).is_nan());
    }
}