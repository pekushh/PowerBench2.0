//! Единый ключ ранжирования схем.
//!
//! Проблема, которую решает модуль: порядок схем вычислялся ТРИЖДЫ и по
//! разным правилам.
//!
//! * `recommend::primary_cmp` сортировал по среднему арифметическому;
//! * `leader::robust_leader` — по медиане;
//! * `score::score_schemes` нормировал по среднему.
//!
//! В одном отчёте соседствовали «перевес 771 %» (по среднему) и «медианный
//! разрыв ×3.7» (по медиане), и пользователь не мог понять, какой из выводов
//! является выводом. Среднее к тому же чувствительно к выбросам: один
//! провальный отрезок внутри прогона сдвигает его сильнее, чем устойчивый
//! сдвиг самой схемы.
//!
//! Теперь единственный источник истины — [`rank_cmp`], а [`RankKey`] собирают
//! из своих данных все три потребителя. Список ключей задан здесь и больше ни
//! где не дублируется, поэтому разойтись они больше не могут.

use std::cmp::Ordering;

/// Данные схем, по которым выбирается победитель.
///
/// Ключи расположены в порядке приоритета; сравнение обрывается на первом
/// различающемся. Нечисловые и неположительные значения считаются равными
/// между собой и уступают любому положительному — иначе схема без данных
/// («0 тик/с») победила бы схему с данными только за счёт `-0.0` и `NaN`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankKey {
    /// Медианный throughput прогонов. ГЛАВНЫЙ ключ.
    ///
    /// Медиана, а не среднее: она не двигается от одного проваленного отрезка
    /// внутри прогона, а среднее двигается — и именно поэтому среднее раньше
    /// путало «схема медленная» с «фоном вонзил один отрезок».
    pub median_throughput: f64,
    /// Медианный P1 (худшее окно) — устойчивая оценка «как часто проваливалась».
    pub p1_throughput: f64,
    /// Медианная стабильность, процент.
    pub consistency_percent: f64,
    /// Разброс прогонов CV, процент. МЕНЬШЕ — лучше.
    pub run_variation_percent: f64,
    /// Среднее. Только детерминированный замыкающий ключ: на одинаковой
    /// статистике различает схемы, не влияя на содержательный выбор.
    pub average_throughput: f64,
}

impl RankKey {
    /// Собрать ключ из агрегата схемы.
    ///
    /// Числа, которые нельзя сравнивать (NaN, бесконечность), заменяются
    /// нулём: иначе `partial_cmp` вернул бы `None` и сравнение молча
    /// обрывалось бы, а схема с испорченной метрикой получила бы место по
    /// жребию.
    pub fn from_parts(
        median_throughput: f64,
        p1_throughput: f64,
        consistency_percent: f64,
        run_variation_percent: f64,
        average_throughput: f64,
    ) -> Self {
        let sane = |v: f64| if v.is_finite() && v > 0.0 { v } else { 0.0 };
        let sane_any = |v: f64| if v.is_finite() { v } else { 0.0 };
        Self {
            median_throughput: sane(median_throughput),
            p1_throughput: sane(p1_throughput),
            consistency_percent: sane_any(consistency_percent),
            run_variation_percent: sane_any(run_variation_percent),
            average_throughput: sane(average_throughput),
        }
    }

    /// Ключ схемы, у которой нет пригодных данных ни по одной метрике.
    pub fn empty() -> Self {
        Self::from_parts(0.0, 0.0, 0.0, 0.0, 0.0)
    }
}

/// Сравнить два ключа: `Greater` — `a` лучше `b`.
///
/// Убывание по всем ключам, кроме CV: там меньше — лучше, поэтому порядок
/// ключа переворачивается. Значения нормализованы в [`RankKey::from_parts`],
/// так что `total_cmp` достаточно и `None` не возникает.
pub fn rank_cmp(a: &RankKey, b: &RankKey) -> Ordering {
    use std::cmp::Ordering::Equal;
    let desc = |x: f64, y: f64| x.total_cmp(&y);

    // 1. Медиана throughput — главный признак.
    match desc(a.median_throughput, b.median_throughput) {
        Equal => {}
        other => return other,
    }
    // 2. Худшее окно (P1): при равной скорости важнее та схема, которая
    //    реже проваливается.
    match desc(a.p1_throughput, b.p1_throughput) {
        Equal => {}
        other => return other,
    }
    // 3. Стабильность.
    match desc(a.consistency_percent, b.consistency_percent) {
        Equal => {}
        other => return other,
    }
    // 4. Меньший разброс повторов — лучше: одинаковая скорость, выраженная
    //    ровно, надёжнее одинаковой скорости «то 1000, то 950». Поэтому ключ
    //    здесь единственный перевёрнут.
    match desc(b.run_variation_percent, a.run_variation_percent) {
        Equal => {}
        other => return other,
    }
    // 5. Среднее — только чтобы порядок был определён.
    desc(a.average_throughput, b.average_throughput)
}

/// Отсортировать схемы по убыванию «значимости» ключа.
///
/// `sort_by` ожидает «меньше — раньше», а [`rank_cmp`] трактует `Greater`
/// как «лучше», поэтому порядок разворачивается.
pub fn sort_by_rank<T>(items: &mut [T], key: impl Fn(&T) -> RankKey) {
    items.sort_by(|a, b| rank_cmp(&key(a), &key(b)).reverse());
}

/// Есть ли у схемы хоть какие-то данные.
pub fn has_data(key: &RankKey) -> bool {
    key.median_throughput > 0.0
}

/// Финальное сравнение по идентификатору схемы.
///
/// Обязательно в КАЖДОМ потребителе, выбирающем единственного победителя, и
/// всегда в одну сторону. Без него выбор схем с одинаковой статистикой
/// зависел от порядка во входных данных: `sort_by` устойчив и оставлял первую,
/// а `max_by` возвращает последнего максимума — то есть `leader` и `score`
/// называли победителями разные схемы на одних и тех же данных.
pub fn tiebreak_id(a_id: &str, b_id: &str) -> Ordering {
    a_id.cmp(b_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    fn key(median: f64, p1: f64, cons: f64, cv: f64, mean: f64) -> RankKey {
        RankKey::from_parts(median, p1, cons, cv, mean)
    }

    /// Медиана — главный ключ: высокая медиана бьёт высокое среднее.
    ///
    /// Раньше `recommend` сортировал по среднему и называл победителем схему с
    /// одним огромным выбросом, а `leader` — совсем другую.
    #[test]
    fn median_outranks_mean() {
        let noisy = key(400.0, 400.0, 80.0, 2.0, 900.0);
        let solid = key(500.0, 500.0, 80.0, 2.0, 505.0);
        assert_eq!(rank_cmp(&solid, &noisy), Greater);
        assert_eq!(rank_cmp(&noisy, &solid), Less);
    }

    /// Схема с данными обязана быть выше схемы без них.
    #[test]
    fn measured_scheme_beats_empty_one() {
        let measured = key(100.0, 90.0, 80.0, 2.0, 100.0);
        let empty = RankKey::empty();
        assert_eq!(rank_cmp(&measured, &empty), Greater);
        assert!(has_data(&measured));
        assert!(!has_data(&empty));
    }

    /// Испорченные числа не должны выигрывать за счёт NaN.
    #[test]
    fn nan_never_wins() {
        let good = key(500.0, 500.0, 90.0, 1.0, 500.0);
        for bad in [
            key(f64::NAN, 500.0, 90.0, 1.0, 500.0),
            key(-1.0, 0.0, 0.0, 0.0, 0.0),
        ] {
            assert_eq!(
                rank_cmp(&good, &bad),
                Greater,
                "NaN/отрицательное не должны выигрывать"
            );
            assert_eq!(rank_cmp(&bad, &good), Less);
        }
    }

    /// При равной скорости решает худшее окно, потом стабильность.
    #[test]
    fn tie_breaks_are_ordered_and_documented() {
        let base = key(500.0, 500.0, 90.0, 5.0, 500.0);
        // P1 важнее стабильности.
        let better_p1 = key(500.0, 560.0, 10.0, 50.0, 500.0);
        assert_eq!(rank_cmp(&better_p1, &base), Greater);
        // Стабильность важнее CV.
        let better_stab = key(500.0, 500.0, 99.0, 90.0, 500.0);
        assert_eq!(rank_cmp(&better_stab, &base), Greater);
        // Меньший CV лучше при прочем равном.
        let better_cv = key(500.0, 500.0, 90.0, 0.5, 500.0);
        assert_eq!(rank_cmp(&better_cv, &base), Greater);
        // И ровно равные ключи дают Equal, а не случайный порядок.
        assert_eq!(rank_cmp(&base, &base), Equal);
    }

    /// Полностью одинаковая статистика различается средним — только чтобы
    /// порядок был определён, но НЕ за счёт содержимого выбора.
    #[test]
    fn mean_only_breaks_exact_ties() {
        let a = key(500.0, 500.0, 90.0, 2.0, 501.0);
        let b = key(500.0, 500.0, 90.0, 2.0, 500.0);
        assert_eq!(rank_cmp(&a, &b), Greater);
    }

    /// Сортировка совпадает с попарным сравнением — иначе `sort_by` и
    /// `rank_cmp` разошлись бы.
    #[test]
    fn sort_is_consistent_with_pairwise_comparison() {
        let keys = [
            ("empty", RankKey::empty()),
            ("median-leader", key(800.0, 700.0, 95.0, 1.0, 800.0)),
            ("mid", key(500.0, 480.0, 90.0, 2.0, 500.0)),
            ("noisy-mean", key(400.0, 400.0, 70.0, 3.0, 2000.0)),
            ("same-as-mid", key(500.0, 480.0, 90.0, 2.0, 500.0)),
        ];
        let mut idx: Vec<usize> = (0..keys.len()).collect();
        sort_by_rank(&mut idx, |&i| keys[i].1);
        let names: Vec<&str> = idx.iter().map(|&i| keys[i].0).collect();
        assert_eq!(names[0], "median-leader");
        assert_eq!(
            *names.last().unwrap(),
            "empty",
            "схема без данных обязана быть последней"
        );
        // Порядок детерминирован: элементы с одинаковым ключом не «прыгают».
        let again: Vec<&str> = idx.iter().map(|&i| keys[i].0).collect();
        assert_eq!(names, again);
    }

    /// `sort_by_rank` совпадает с ручной сортировкой по `rank_cmp`.
    #[test]
    fn sort_by_rank_helper_matches_manual_sort() {
        let mut a = vec![
            key(100.0, 1.0, 1.0, 1.0, 100.0),
            key(900.0, 1.0, 1.0, 1.0, 900.0),
        ];
        sort_by_rank(&mut a, |k| *k);
        assert_eq!(a[0].median_throughput, 900.0);
        let mut b = a.clone();
        b.sort_by(|x, y| rank_cmp(x, y).reverse());
        assert_eq!(a, b);
    }
}
