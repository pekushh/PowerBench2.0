//! Адаптивная ранняя остановка сессии.
//!
//! Пресеты обещают её пользователю: подсказка в интерфейсе объясняет, сколько
//! повторов нужно, чтобы накопленный перевес лидера стал статистически
//! значимым (формула `2√2·t(0.975,k−1)·cv·100/√k`), и на пяти повторах обещает
//! заметную экономию. Кода ранней остановки не было: `early_stop_reason`
//! заполнялся просто «сессия прервана», то есть обещание выполнялось только
//! на словах.
//!
//! Здесь правило вынесено в чистую функцию, которую можно проверить без
//! часового прогона: решение принимается по t-распределению накопленных
//! прогонов, а не по числу тиков.
//!
//! ## Почему t, а не «перевес больше N %»
//!
//! Пять прогонов с разбросом 10 % дают широкую доверительную зону: перевес в
//! 3 % при таком разбросе ничего не доказывает, а 30 % доказывает при любом.
//! Порог «N %» был бы либо слишком робким (ждать полного плана на шумной
//! машине), либо слишком смелым (останавливаться на разнице, которую нельзя
//! отличить от шума). t-критерий даёт в обоих случаях верный ответ.

use std::collections::BTreeMap;

use powerbench_metrics::t95;

/// Минимум прогонов у лидера и у второго места, без которых решение не
/// принимается.
///
/// Три — потому что при двух прогонах степени свободы один, и t-критерий
/// превращается в требование «перевес больше разброса», то есть почти
/// никогда. Это же число используется в правилах карантина
/// (`MIN_RUNS_FOR_JUDGMENT`), чтобы «когда можно судить» было одно.
pub const EARLY_STOP_MIN_RUNS: usize = 3;

/// Решение о досрочной остановке.
#[derive(Debug, Clone, PartialEq)]
pub struct EarlyStopDecision {
    /// Нужно ли останавливать сессию.
    pub stop: bool,
    /// Наблюдённый перевес лидера над вторым местом, %.
    pub actual_percent: f64,
    /// Перевес, который считается статистически значимым, %.
    pub required_percent: f64,
    /// Человекочитаемое объяснение (пусто — решение не принято).
    pub reason: String,
    /// Сколько раундов ещё осталось в плане.
    pub remaining_rounds: u32,
}

/// Итог без ранней остановки.
fn no_stop() -> EarlyStopDecision {
    EarlyStopDecision {
        stop: false,
        actual_percent: 0.0,
        required_percent: f64::INFINITY,
        reason: String::new(),
        remaining_rounds: 0,
    }
}

/// Среднее и выборочное стандартное отклонение (k − 1).
fn mean_std(values: &[f64]) -> (f64, f64) {
    let k = values.len();
    if k == 0 {
        return (0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / k as f64;
    if k < 2 {
        return (mean, 0.0);
    }
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (k - 1) as f64;
    (mean, var.sqrt())
}

/// Решить, можно ли остановить сессию досрочно.
///
/// `by_scheme` — средний throughput по каждой прогонённой схеме (допускается
/// разное число прогонов: схема, забракованная посреди сессии, их имеет
/// меньше, и это не повод судить по ней).
///
/// `rounds_completed` / `rounds_planned` — состояние плана. Остановка при
/// `rounds_completed == rounds_planned` бессмысленна: идти больше некуда.
pub fn early_stop_decision(
    by_scheme: &BTreeMap<String, Vec<f64>>,
    rounds_completed: u32,
    rounds_planned: u32,
) -> EarlyStopDecision {
    let remaining = rounds_planned.saturating_sub(rounds_completed);
    if remaining == 0 {
        return no_stop();
    }

    // Кандидаты с достаточным числом прогонов и конечными величинами.
    let mut candidates: Vec<(String, f64, f64, usize)> = by_scheme
        .iter()
        .filter_map(|(id, runs)| {
            let values: Vec<f64> = runs.iter().copied().filter(|v| v.is_finite()).collect();
            if values.len() < EARLY_STOP_MIN_RUNS {
                return None;
            }
            let (mean, std) = mean_std(&values);
            if !mean.is_finite() || mean <= 0.0 {
                return None;
            }
            Some((id.clone(), mean, std, values.len()))
        })
        .collect();
    if candidates.len() < 2 {
        return no_stop();
    }
    // Сортировка по среднему убывающей; при равенстве — по идентификатору,
    // чтобы решение не зависело от порядка входа.
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let (_, m_lead, s_lead, n_lead) = &candidates[0];
    let (_, m_run, s_run, n_run) = &candidates[1];

    // Перевес относительно второго места.
    if *m_run <= 0.0 {
        return no_stop();
    }
    let actual_percent = (*m_lead - *m_run) / *m_run * 100.0;
    if actual_percent <= 0.0 {
        return no_stop();
    }

    // Значимый перевес по t-критерию Уэлча: комбинированная стандартная ошибка
    // разности средних. Схемы меряются по очереди в одном раунде, поэтому
    // разделённых оценок здесь не делаем: они делят одну и ту же машину в одно
    // и то же время.
    let df = (*n_lead).min(*n_run) - 1;
    let se = s_lead / (*n_lead as f64).sqrt() + s_run / (*n_run as f64).sqrt();
    if !se.is_finite() || se <= 0.0 {
        // Разброс нулевой: оба набора прогонов совпали до последнего знака.
        // Значимости при нулевом разбросе не существует, и объявлять перевес
        // доказанным на пустом разбросе нельзя — это была бы уверенность из
        // ничего. План доигрывается.
        return EarlyStopDecision {
            stop: false,
            actual_percent,
            required_percent: 0.0,
            reason: String::new(),
            remaining_rounds: remaining,
        };
    }
    let required_percent = t95(df) * se / *m_run * 100.0;

    if actual_percent < required_percent {
        return EarlyStopDecision {
            stop: false,
            actual_percent,
            required_percent,
            reason: String::new(),
            remaining_rounds: remaining,
        };
    }

    EarlyStopDecision {
        stop: true,
        actual_percent,
        required_percent,
        reason: format!(
            "перевес {actual_percent:.1} % превышает статистически значимые {required_percent:.1} % \
             (t₀.₉₇₅, {} прогонов, разброс {:.1} % / {:.1} %) — оставшиеся {remaining} раундов \
             не изменят вывод",
            n_lead.min(n_run),
            s_lead / *m_lead * 100.0,
            s_run / *m_run * 100.0,
        ),
        remaining_rounds: remaining,
    }
}

/// Накопить средние прогоны из контрольной точки для решения об остановке.
///
/// Собираются только схемы, признанные годными: у забракованных прогонов
/// средние считать нельзя, а невыполненные раунды не должны влиять на
/// статистику лидера.
pub fn leader_inputs(
    runs: &[crate::checkpoint::StoredRun],
    rejected: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<f64>> {
    let mut out: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for r in runs {
        let id = r.scheme_id.to_ascii_lowercase();
        if rejected.contains_key(&id) || rejected.contains_key(&r.scheme_id) {
            continue;
        }
        let value = r.combined.average_throughput;
        if value.is_finite() && value > 0.0 {
            out.entry(r.scheme_id.clone()).or_default().push(value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Многозначный сценарий лидера.
    fn map(pairs: &[(&str, &[f64])]) -> BTreeMap<String, Vec<f64>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_vec()))
            .collect()
    }

    /// Разгромный перевес при малом разбросе → останавливаемся.
    #[test]
    fn decisive_leadership_stops_early() {
        let m = map(&[
            ("a", &[1000.0, 1010.0, 990.0, 1005.0, 995.0]),
            ("b", &[800.0, 810.0, 790.0, 805.0, 795.0]),
        ]);
        let d = early_stop_decision(&m, 2, 5);
        assert!(
            d.stop,
            "перевес 25 % при разбросе 1 % обязан остановить сессию"
        );
        assert!(d.actual_percent > 24.0 && d.actual_percent < 26.0);
        assert!(d.required_percent < d.actual_percent);
        assert!(!d.reason.is_empty());
        assert_eq!(d.remaining_rounds, 3);
    }

    /// Шумный перевес: 5 % при разбросе 10 % ничего не доказывает.
    ///
    /// Именно этот случай «фиксированный порог в N %» обработал бы неверно в
    /// одну из сторон, а t-критерий говорит «ждать дальше».
    #[test]
    fn noisy_small_lead_does_not_stop() {
        let m = map(&[
            // Разброс ~10 % вокруг 1000.
            ("a", &[1100.0, 900.0, 1050.0, 950.0, 1000.0]),
            ("b", &[950.0, 1050.0, 900.0, 1000.0, 990.0]),
        ]);
        let d = early_stop_decision(&m, 2, 5);
        assert!(!d.stop, "перевес 2 % при разбросе 10 % не доказан");
        assert!(
            d.required_percent > d.actual_percent,
            "требуемый перевес {} должен превышать наблюдаемый {}",
            d.required_percent,
            d.actual_percent
        );
    }

    /// Двух прогонов мало: при k = 2 критерий превращается в «перевес больше
    /// разброса», и остановка была бы самообманом.
    #[test]
    fn two_runs_are_not_enough() {
        let m = map(&[("a", &[1000.0, 1000.0]), ("b", &[100.0, 100.0])]);
        let d = early_stop_decision(&m, 1, 5);
        assert!(
            !d.stop,
            "двух прогонов недостаточно для сколь-нибудь вывода"
        );
    }

    /// Одна схема — сравнивать не с чем.
    #[test]
    fn single_candidate_never_stops() {
        let m = map(&[("a", &[1000.0, 1000.0, 1000.0, 1000.0])]);
        assert!(!early_stop_decision(&m, 3, 5).stop);
    }

    /// План доигран: останавливаться не на чем.
    #[test]
    fn nothing_to_skip_when_plan_is_complete() {
        let m = map(&[
            ("a", &[1000.0, 1010.0, 990.0]),
            ("b", &[800.0, 810.0, 790.0]),
        ]);
        let d = early_stop_decision(&m, 5, 5);
        assert!(!d.stop);
        assert_eq!(d.remaining_rounds, 0);
    }

    /// Нулевой разброс обоих наборов: значимости при нём не существует.
    ///
    /// Объявлять перевес доказанным на пустом разбросе — уверенность из
    /// ничего: одинаковые идеально повторяющиеся прогоны так не бывают, а
    /// если случилось, это скорее артефакт.
    #[test]
    fn zero_variance_does_not_manufacture_confidence() {
        let m = map(&[
            ("a", &[1000.0, 1000.0, 1000.0]),
            ("b", &[500.0, 500.0, 500.0]),
        ]);
        let d = early_stop_decision(&m, 1, 5);
        assert!(
            !d.stop,
            "нулевой разброс не даёт права объявлять значимость"
        );
        assert_eq!(d.actual_percent, 100.0);
    }

    /// Решение не зависит от порядка схем во входе.
    #[test]
    fn decision_is_independent_of_input_order() {
        let a = map(&[
            ("a", &[1000.0, 1010.0, 990.0]),
            ("b", &[800.0, 810.0, 790.0]),
        ]);
        let b = map(&[
            ("b", &[800.0, 810.0, 790.0]),
            ("a", &[1000.0, 1010.0, 990.0]),
        ]);
        assert_eq!(
            early_stop_decision(&a, 1, 5).stop,
            early_stop_decision(&b, 1, 5).stop
        );
    }

    /// Перевес растёт с числом прогонов при той же разнице средних: при
    /// сходимости разброс падает, и порог значимости снижается. Это и есть
    /// механизм, ради которого повторы вообще нужны.
    ///
    /// Первый набор — перевес 2 % при разбросе 10 %: очевидно не доказан. Второй —
    /// тот же перевес при разбросе 0,4 %: при шести прогонах он уже значим.
    #[test]
    fn more_runs_lower_the_significance_bar() {
        let three = map(&[
            ("a", &[1000.0, 1010.0, 990.0]),
            ("b", &[980.0, 990.0, 970.0]),
        ]);
        let d3 = early_stop_decision(&three, 1, 5);
        assert!(!d3.stop, "перевес 2 % при разбросе 10 % не доказан");

        let a_clean: Vec<f64> = (0..6)
            .map(|i| 1000.0 + if i % 2 == 0 { 2.0 } else { -2.0 })
            .collect();
        let b_clean: Vec<f64> = (0..6)
            .map(|i| 980.0 + if i % 2 == 0 { 2.0 } else { -2.0 })
            .collect();
        let six = map(&[("a", &a_clean), ("b", &b_clean)]);
        let d6 = early_stop_decision(&six, 3, 9);
        assert!(
            d6.stop,
            "перевес 2 % при разбросе 0,4 % и шести прогонах доказан: {} < {}",
            d6.actual_percent, d6.required_percent
        );
        assert!(
            d6.required_percent < d3.required_percent,
            "при вдвое большем числе прогонов порог значимости должен упасть: {} против {}",
            d6.required_percent,
            d3.required_percent
        );
    }
}
