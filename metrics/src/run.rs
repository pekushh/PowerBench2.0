//! Статистика одного прогона (набора сэмплов времени тика).
//!
//! Каждый завершённый тик даёт один сэмпл: `CompletedWorkUnits = 1`,
//! `ActiveTimeMs = время тика в мс`, throughput сэмпла = `1000/ActiveTimeMs`.
//! Невалидные сэмплы (неположительное или не конечное время) исключаются.

use crate::percentile::{percentile, percentile_sorted};

/// Исключить невалидные сэмплы: время должно быть положительным и конечным.
pub fn filter_valid_times(times_ms: &[f64]) -> Vec<f64> {
    times_ms
        .iter()
        .copied()
        .filter(|t| t.is_finite() && *t > 0.0)
        .collect()
}

/// Среднее арифметическое (0.0 для пустого набора).
pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// Стандартное отклонение генеральной совокупности:
/// `sqrt( Σ(x − mean)² / n )`.
pub fn population_std(values: &[f64]) -> f64 {
    let n = values.len();
    if n == 0 {
        return 0.0;
    }
    let m = mean(values);
    (values.iter().map(|x| (x - m).powi(2)).sum::<f64>() / n as f64).sqrt()
}

/// Медиана (перцентиль 0.50).
pub fn median(values: &[f64]) -> f64 {
    percentile(values, 0.50)
}

/// Score стабильности одной группы сэмплов (фразы спецификации):
/// `variation = population_std(throughput)/mean(throughput)*100`,
/// `score = clamp(100 − variation, 0, 100)`.
pub fn consistency_score(throughput: &[f64]) -> f64 {
    if throughput.is_empty() {
        return 0.0;
    }
    let m = mean(throughput);
    if m == 0.0 {
        return 0.0;
    }
    let variation = population_std(throughput) / m * 100.0;
    (100.0 - variation).clamp(0.0, 100.0)
}

/// Вклад одной группы сэмплов в ConsistencyPercent прогона.
///
/// Отдаёт пару `(score стабильности, вес)`, где вес — число валидных
/// сэмплов группы. Итог — взвешенное среднее score по фазам.
///
/// Считается по НЕУСЕЧЁННЫМ валидным сэмплам: усечение 2,5 % по краям
/// применяется к перцентилям и к `RunStats.consistency_percent` фазы, но
/// сглаживание ConsistencyPercent прогона по ним искажало бы картину.
pub fn consistency_group(times_ms: &[f64]) -> (f64, f64) {
    // Фильтр первым: 1000/0 или 1000/NaN дали бы inf/NaN в скор.
    let throughput: Vec<f64> = filter_valid_times(times_ms)
        .iter()
        .map(|ms| 1000.0 / ms)
        .collect();
    (consistency_score(&throughput), throughput.len() as f64)
}

/// ConsistencyPercent прогона с группами по фазам: в каждой фазе считается
/// score стабильности, итог — взвешенное среднее score по числу сэмплов фаз.
///
/// Группа: `(идентификатор фазы, времена тиков в мс)`. Сэмплы каждой группы
/// приводятся к throughput `1000/ms`.
pub fn consistency_percent(times_by_phase: &[(u32, &[f64])]) -> f64 {
    let mut weighted = 0.0;
    let mut total_weight = 0.0;
    for &(_, times) in times_by_phase {
        let (score, n) = consistency_group(times);
        weighted += n * score;
        total_weight += n;
    }
    if total_weight == 0.0 {
        0.0
    } else {
        weighted / total_weight
    }
}

/// TickJitterP99Ms: перцентиль 0.99 попарных разностей времён соседних тиков.
/// Если данных нет (меньше двух сэмплов) — 0 (спецификация).
///
/// ВАЖНО: на входе ожидается **хронологический** порядок. Разности берутся между
/// соседями во времени, и это единственное, что делает метрику джиттером.
/// Сортированный набор здесь даёт почти нули: разность соседей в отсортированном
/// массиве равна зазору между соседними значениями, то есть плотности выборки, а
/// не скачку времени тика (регресс C10 — занижение на порядки).
pub fn jitter_p99_ms(times_ms: &[f64]) -> f64 {
    let times = filter_valid_times(times_ms);
    if times.len() < 2 {
        return 0.0;
    }
    let diffs: Vec<f64> = times.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    percentile(&diffs, 0.99)
}

/// BurstRetentionPercent = AverageThroughput(Heavy)/AverageThroughput(Light)*100
/// (0, если AverageThroughput(Light) = 0).
pub fn burst_retention_percent(average_heavy: f64, average_light: f64) -> f64 {
    if average_light == 0.0 {
        0.0
    } else {
        average_heavy / average_light * 100.0
    }
}

/// Доля сэмплов, отбрасываемых с каждого края при расчёте устойчивых
/// статистик.
///
/// Замер идёт на Windows, и в поток тика периодически влетают DPC от сетевого
/// стека, прерывания таймера, вытеснение ядрами Hyper-V и редкие страничные
/// ошибки. Каждое такое событие — один сэмпл, в сотни-тысячи раз длиннее
/// остальных. В среднем они почти не весят, но в σ весят очень много: одно
/// событие, растянувшее σ втрое, съедает 20-30 процентных пунктов стабильности,
/// и в отчёте это выглядит как «схема нестабильна», хотя схема тут ни при чём.
pub const TRIM_FRACTION: f64 = 0.025;

/// Минимальное число сэмплов, при котором усечение вообще имеет смысл.
///
/// При 2,5 % с каждого края отбрасывается `floor(n/40)` сэмплов. На сотне
/// сэмплов это по два — уже шум выборки; на тысяче — по 25, то есть усечение
/// отделяет реальные выбросы от обычного разброса.
pub const MIN_TRIM_SAMPLES: usize = 400;

/// Максимальная доля усечения с каждого края.
///
/// При 25 % с каждого края остаётся половина выборки. Больше — это уже не
/// устойчивая оценка, а произвольно выбранный подотрезок: например, 40 % с
/// каждого края оставили бы middle 20 %, где среднее ничего не говорит о
/// хвостах распределения.
pub const MAX_TRIM_FRACTION: f64 = 0.25;

/// Отбросить выбросы с обоих краёв: `floor(n * fraction)` худших и столько же
/// лучших.
///
/// Усечение по **временам тиков**, а не по throughput: среднее считается как
/// `Σwork·1000 / ΣactiveMs`, и чтобы убрать одно плохое событие, надо убрать
/// его время работы из знаменателя. Отбрасывание сэмпла по throughput
/// испортило бы соответствие между числом работы и суммой времени.
///
/// ВАЖНО: результат **отсортирован по возрастанию** — это нужно перцентилям и
/// среднему. Хронологический порядок при этом теряется, поэтому метрики
/// времени по соседям (джиттер) обязаны считаться по исходной выборке, а не по
/// этому массиву.
pub fn trim_outliers(times_ms: &[f64], fraction: f64) -> Vec<f64> {
    let mut sorted = times_ms.to_vec();
    if sorted.len() < MIN_TRIM_SAMPLES || !(0.0..=MAX_TRIM_FRACTION).contains(&fraction) {
        return sorted;
    }
    sorted.sort_by(|a, b| a.total_cmp(b));
    let k = (sorted.len() as f64 * fraction).floor() as usize;
    // Усечение не должно съесть больше половины выборки — иначе это уже не
    // устойчивая оценка, а произвольно выбранный подотрезок.
    if k == 0 || 2 * k >= sorted.len() {
        return sorted;
    }
    sorted[k..sorted.len() - k].to_vec()
}

/// Окно для расчёта P0.1: 100 мс.
///
/// Окно намеренно меньше секунды: P0.1 должен ловить КРАТКОВРЕМЕННУЮ остановку
/// (микрофриз, DPC-шторм, вытеснение), а секундное окно её размазывает.
/// Худшая секунда (`worst_second_throughput`) остаётся крупным окном и отвечает
/// за устойчивое падение — это две разные вещи, и смешивать их нельзя.
pub const P01_WINDOW_MS: u64 = 100;

/// Окно для расчёта худшей секунды.
pub const WORST_SECOND_WINDOW_MS: u64 = 1000;

/// Во сколько раз окно должно быть хуже типичного, чтобы сохранить его без
/// оглядки на число сэмплов (регресс M1).
///
/// Порог в 2 раза — с запасом: речь о заведомо выбросном окне, а не о шуме
/// между соседними окнами одной фазы.
const STALL_KEEP_RATIO: f64 = 0.5;

/// Throughput по скользящим окнам кумулятивного времени.
///
/// Окна идут по НАКОПЛЕННОМУ времени, а не по длительности тика: иначе все
/// тики короче окна попали бы в нулевое окно. Окна с малым числом сэмплов
/// отбрасываются — иначе последнее неполное окно дало бы шумную оценку из
/// двух точек, и «худшая секунда» зависела бы от того, где закончилась фаза.
///
/// Регресс M1: это отбрасывание съедало МИКРОФРИЗЫ, то есть ровно то, ради чего
/// окна и считаются. Задержка длиной в десятки миллисекунд при обычном тике
/// в доли миллисекуды занимает почти всё своё окно целиком: в него попадает
/// один-единственный сэмпл, `min_samples` его отбрасывал, и худший момент
/// фазы исчезал из `p01_throughput` и `worst_second_throughput`. Метрика,
/// обещанная как «ловушка кратковременной остановки», показывала норму.
///
/// Поэтому число сэмплов — не единственный повод оставить окно: если окно
/// заметно ХУЖЕ типичного (вдвое, см. `STALL_KEEP_RATIO`), оно сохраняется
/// всегда.
/// Оценка такого окна шумной не является — её определило время, а не
/// количество точек. Порог относительный, поэтому сам по себе не требует
/// подгонки под конкретную машину или workload.
pub fn windowed_throughput(times_ms: &[f64], window_ms: u64, min_samples: usize) -> Vec<f64> {
    let times = filter_valid_times(times_ms);
    if times.is_empty() || window_ms == 0 {
        return Vec::new();
    }
    let window = window_ms as f64;
    // Ключ окна — миллисекунды кумулятивного времени, делённые на окно.
    let mut buckets: std::collections::BTreeMap<u64, (u64, f64)> =
        std::collections::BTreeMap::new();
    let mut elapsed_ms = 0.0f64;
    for ms in &times {
        elapsed_ms += ms;
        let index = (elapsed_ms / window).floor() as u64;
        let e = buckets.entry(index).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += ms;
    }
    // Оценка throughput каждого окна, у которого есть хоть один валидный
    // сэмпл. Считаем ДО фильтра: типичный уровень нужен, чтобы решить, какие
    // окна являются выбросами.
    let all: Vec<(usize, f64)> = buckets
        .values()
        .filter_map(|(work, active_ms)| {
            if *active_ms <= 0.0 || *work == 0 {
                return None;
            }
            let tp = *work as f64 * 1000.0 / *active_ms;
            tp.is_finite().then_some((*work as usize, tp))
        })
        .collect();
    if all.is_empty() {
        return Vec::new();
    }
    let typical = median(&all.iter().map(|(_, tp)| *tp).collect::<Vec<f64>>());
    all.into_iter()
        .filter_map(|(work, tp)| {
            let keep = work >= min_samples || (typical > 0.0 && tp < typical * STALL_KEEP_RATIO);
            keep.then_some(tp)
        })
        .collect()
}

/// P01Throughput: 0,1-й перцентиль throughput по ОКНАМ в 100 мс.
///
/// Раньше это был 0,001-й перцентиль по ВСЕМ сэмплам, и величина зависела от
/// их количества: при n = 2000 интерполяция попадала на второй с конца сэмпл,
/// при n = 50000 — на 50-й, то есть «худший момент» становился тем лучше,
/// чем длиннее фаза. Сравнивать P01 фаз разной длины было бессмысленно, а он
/// при этом первоклассный выход отчёта и критерий разрешения ничьей.
pub fn p01_throughput(times_ms: &[f64]) -> f64 {
    let windows = windowed_throughput(times_ms, P01_WINDOW_MS, 5);
    if windows.is_empty() {
        // Окна не набрались (короткая или очень медленная фаза) — честно
        // откатываемся к исходной величине по всем сэмплам.
        return percentile(
            &filter_valid_times(times_ms)
                .into_iter()
                .map(|ms| 1000.0 / ms)
                .collect::<Vec<f64>>(),
            0.001,
        );
    }
    percentile(&windows, 0.001)
}

/// Худшая секунда прогона: минимум AverageThroughput по 1-секундным окнам
/// кумулятивного времени (целочисленные границы). Окно без валидных
/// сэмплов не участвует.
pub fn worst_second_throughput(times_ms: &[f64]) -> f64 {
    let windows = windowed_throughput(times_ms, WORST_SECOND_WINDOW_MS, 20);
    if windows.is_empty() {
        0.0
    } else {
        windows.iter().copied().fold(f64::INFINITY, f64::min)
    }
}

/// Статистика одного прогона фазы.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunStats {
    /// Число валидных сэмплов, оставшихся ПОСЛЕ усечения выбросов.
    pub samples: usize,
    /// Сколько валидных сэмплов было до усечения.
    #[serde(default)]
    pub samples_raw: usize,
    /// Доля сэмплов, исключённых из устойчивых статистик, 0..=1.
    ///
    /// Это НЕ доля «найденных выбросов»: усечение безусловно снимает по
    /// `TRIM_FRACTION` с каждого края, поэтому на чистых данных здесь тоже
    /// будет ≈ `2 * TRIM_FRACTION`. Величина показывает, сколько данных не
    /// участвовало в среднем и стабильности — экстремумы (P01, P95, P99,
    /// худшая секунда) считаются по полному набору и сюда не входят.
    #[serde(default)]
    pub excluded_fraction: f64,
    /// Сумма единиц работы (CompletedWorkUnits = 1 на тик).
    pub work_units: u64,
    /// Сумма активных времён тиков в мс.
    pub active_time_ms_total: f64,
    /// AverageThroughput = (Σ work) * 1000 / (Σ activeMs) — взвешенное
    /// среднее, НЕ среднее по-сэмпловых throughput.
    pub average_throughput: f64,
    /// AverageExecutionTimeMs = (Σ activeMs) / (Σ work).
    pub average_execution_time_ms: f64,
    /// MedianThroughput = перцентиль 0.50 от throughput сэмплов.
    ///
    /// Медиана — по УСЕЧЁННОМУ набору: это устойчивая оценка положения центра
    /// распределения, и именно она выбирает лидера.
    pub median_throughput: f64,
    /// P1Throughput = перцентиль 0.01 от throughput сэмплов.
    ///
    /// По ПОЛНОМУ набору валидных сэмплов, как P01/P95/P99 и худшая секунда.
    /// Усечение 2,5 % с каждого края сдвигало бы сюда 3,5-й перцентиль.
    pub p1_throughput: f64,
    /// P01Throughput = 0,1-й перцентиль по ОКНАМ в 100 мс.
    ///
    /// Окна, а не все сэмплы: величина не должна зависеть от длины фазы.
    pub p01_throughput: f64,
    /// P95ExecutionTimeMs = перцентиль 0.95 от времён тиков.
    pub p95_execution_time_ms: f64,
    /// P99ExecutionTimeMs = перцентиль 0.99 от времён тиков.
    pub p99_execution_time_ms: f64,
    /// ConsistencyPercent прогона (сэмплы одной фазы = одна группа).
    pub consistency_percent: f64,
    /// TickJitterP99Ms.
    ///
    /// Считается по полной хронологической выборке: разности берутся между
    /// соседними во времени тиками.
    pub jitter_p99_ms: f64,
    /// Худшая секунда: минимум AverageThroughput среди 1-секундных окон.
    #[serde(default)]
    pub worst_second_throughput: f64,
}

/// Статистика прогона по временам тиков. `None`, если валидных сэмплов нет.
///
/// Расчёт идёт по УСЕЧЁННОМУ набору: 2,5 % худших и столько же лучших
/// сэмплов отбрасываются, и только потом считаются среднее, σ и стабильность.
/// Метрики, смысл которых в экстремуме (P01, P95, P99, джиттер, худшая секунда),
/// считаются по ПОЛНОМУ набору — иначе усечение съело бы ровно то, что они
/// измеряют.
pub fn run_stats(times_ms: &[f64]) -> Option<RunStats> {
    let valid = filter_valid_times(times_ms);
    if valid.is_empty() {
        return None;
    }
    let samples_raw = valid.len();
    let times = trim_outliers(&valid, TRIM_FRACTION);
    let dropped = samples_raw.saturating_sub(times.len());

    let samples = times.len();
    let work_units = samples as u64;
    let active_time_ms_total = times.iter().sum::<f64>();
    let average_throughput = work_units as f64 * 1000.0 / active_time_ms_total;
    let average_execution_time_ms = active_time_ms_total / work_units as f64;

    let mut throughput: Vec<f64> = times.iter().map(|ms| 1000.0 / ms).collect();
    throughput.sort_by(|a, b| a.total_cmp(b));

    // Верхние перцентили ВРЕМЕНИ — по ПОЛНОМУ набору. Усечение снимает самые
    // медленные тики, то есть ровно тот хвост, который p95/p99 и должны
    // показывать; посчитанные по усечённому набору они всегда показывали бы
    // «всё отлично».
    let mut all_times_sorted = valid.clone();
    all_times_sorted.sort_by(|a, b| a.total_cmp(b));

    // P1 — тоже по ПОЛНОМУ набору, и это не опечатка, а смысл метрики.
    //
    // Раньше P1 считался по усечённому набору, а усечение снимает по 2,5 % с
    // каждого края: 1-й перцентиль усечённого распределения — это 3,5-й
    // перцентиль полного. Метрика, обещанная как «худший 1 % тиков», молча
    // показывала 3,5-й процентиль и завышала его тем сильнее, чем короче
    // был набор (усечение не работает вовсе при < 400 сэмплов — там разрыв
    // между полным и усечённым набором максимален).
    let mut all_throughput_sorted: Vec<f64> = valid.iter().map(|ms| 1000.0 / ms).collect();
    all_throughput_sorted.sort_by(|a, b| a.total_cmp(b));

    Some(RunStats {
        samples,
        samples_raw,
        excluded_fraction: if samples_raw == 0 {
            0.0
        } else {
            dropped as f64 / samples_raw as f64
        },
        work_units,
        active_time_ms_total,
        average_throughput,
        average_execution_time_ms,
        median_throughput: percentile_sorted(&throughput, 0.50),
        p1_throughput: percentile_sorted(&all_throughput_sorted, 0.01),
        p01_throughput: p01_throughput(&valid),
        p95_execution_time_ms: percentile_sorted(&all_times_sorted, 0.95),
        p99_execution_time_ms: percentile_sorted(&all_times_sorted, 0.99),
        consistency_percent: consistency_percent(&[(0, &times)]),
        // Джиттер — по ХРОНОЛОГИЧЕСКОЙ выборке `valid`, а не по усечённому
        // `times`: усечение сортирует массив, и разности соседей в нём — это
        // зазоры между соседними значениями, то есть плотность выборки, а не
        // скачки времени тика (регресс C10 — занижение на 3-4 порядка).
        jitter_p99_ms: jitter_p99_ms(&valid),
        worst_second_throughput: worst_second_throughput(&valid),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_samples_are_excluded() {
        let samples = [0.0, -5.0, f64::NAN, f64::INFINITY, 5.0];
        let valid = filter_valid_times(&samples);
        assert_eq!(valid, vec![5.0]);
    }

    #[test]
    fn average_throughput_is_weighted_not_sample_mean() {
        // 2, 2, 4 мс: work = 3, ΣactiveMs = 8 → 3*1000/8 = 375.
        let r = run_stats(&[2.0, 2.0, 4.0]).unwrap();
        assert_eq!(r.average_throughput, 375.0);
        assert!((r.average_execution_time_ms - 8.0 / 3.0).abs() < 1e-12);
        assert_eq!(r.samples, 3);
        assert_eq!(r.work_units, 3);
        // Среднее по-сэмпловых throughput = (500+500+250)/3 ≈ 416.7 ≠ 375.
    }

    #[test]
    fn empty_and_invalid_inputs_produce_none() {
        assert!(run_stats(&[]).is_none());
        assert!(run_stats(&[0.0, f64::NAN, f64::NEG_INFINITY]).is_none());
    }

    /// Полный набор руками посчитанных формул на временах [1, 1, 1, 2] мс.
    #[test]
    fn run_stats_honours_all_formulas() {
        let r = run_stats(&[1.0, 1.0, 1.0, 2.0]).unwrap();
        assert_eq!(r.samples, 4);
        assert_eq!(r.work_units, 4);
        assert_eq!(r.active_time_ms_total, 5.0);
        // AverageThroughput = 4*1000/5 = 800.
        assert_eq!(r.average_throughput, 800.0);
        assert_eq!(r.average_execution_time_ms, 1.25);

        // throughput сэмплов: [500, 1000, 1000, 1000].
        // median (p0.5): pos = 1.5 → 1000.
        assert_eq!(r.median_throughput, 1000.0);
        // p1: pos = 0.01*3 = 0.03 → 500 + 500*0.03 = 515.
        let p1 = 500.0 + 500.0 * 0.03;
        assert!((r.p1_throughput - p1).abs() < 1e-9);
        // p01: pos = 0.001*3 = 0.003 → 500 + 500*0.003 = 501.5.
        let p01 = 500.0 + 500.0 * 0.003;
        assert!((r.p01_throughput - p01).abs() < 1e-9);

        // времена [1,1,1,2]: p95 pos = 0.95*3 = 2.85 → 1 + 1*0.85 = 1.85.
        assert!((r.p95_execution_time_ms - 1.85).abs() < 1e-12);
        // p99 pos = 0.99*3 = 2.97 → 1 + 1*0.97 = 1.97.
        assert!((r.p99_execution_time_ms - 1.97).abs() < 1e-12);

        // Стабильность: population_std([500,1000,1000,1000]) = 216.50635094610965,
        // mean = 875 → score = clamp(100 − 24.74358298…, 0, 100) ≈ 75.256417.
        assert!((r.consistency_percent - 75.256417).abs() < 1e-3);

        // jitter: разности [0, 0, 1]; p99 pos = 0.99*2 = 1.98 → 0 + 1*0.98.
        assert!((r.jitter_p99_ms - 0.98).abs() < 1e-12);
    }

    #[test]
    fn consistency_percent_is_weighted_across_phases() {
        // Фаза A: [1,1] → throughput [1000,1000], variation 0, score 100.
        // Фаза B: [1,3] → throughput [1000, 333.33…], variation 50, score 50.
        // Веса 2 и 2 → (2*100 + 2*50)/4 = 75.
        let a = [1.0, 1.0];
        let b = [1.0, 3.0];
        let score = consistency_percent(&[(0, &a), (1, &b)]);
        assert!((score - 75.0).abs() < 1e-9);
        // Пустые группы → 0.
        assert_eq!(consistency_percent(&[]), 0.0);
    }

    #[test]
    fn burst_retention_and_jitter_edges() {
        assert!((burst_retention_percent(900.0, 800.0) - 112.5).abs() < 1e-9);
        assert_eq!(burst_retention_percent(900.0, 0.0), 0.0);
        // Меньше двух сэмплов — jitter 0.
        assert_eq!(jitter_p99_ms(&[1.0]), 0.0);
        assert_eq!(jitter_p99_ms(&[]), 0.0);
    }

    /// Настоящая рабочая величина замера: тик около 0,2 мс, с редкими
    /// выбросами в 20-50 мс (DPC, вытеснение, микрофриз).
    fn realistic_series(n: usize, outlier_every: usize, outlier_ms: f64) -> Vec<f64> {
        (0..n)
            .map(|i| {
                if outlier_every > 0 && i % outlier_every == 0 {
                    outlier_ms
                } else {
                    0.2
                }
            })
            .collect()
    }

    /// Усечение отделяет выбросы ОС от разброса самой нагрузки.
    ///
    /// Без усечения один выброс в 250 раз раздувает σ, и стабильность падает
    /// с ~100 до единиц — то есть схема выглядит «провальной» из-за события,
    /// к ней никакого отношения не имеющего.
    #[test]
    fn trimming_removes_os_level_outliers_from_stability() {
        let n = 10_000;
        // 0,5 % выбросов по 20 мс среди тиков по 0,2 мс.
        let with_outliers = realistic_series(n, 200, 20.0);
        let stats = run_stats(&with_outliers).expect("есть валидные сэмплы");

        assert_eq!(stats.samples_raw, n);
        assert!(
            stats.samples < n,
            "усечение обязано что-то отбросить: {} из {}",
            stats.samples,
            stats.samples_raw
        );
        assert!(
            stats.excluded_fraction > 0.04 && stats.excluded_fraction < 0.06,
            "ожидалось около 5 % исключённых (2,5 % с каждого края), получено {}",
            stats.excluded_fraction
        );
        // Среднее должно остаться около 1000/0.2 = 5000 тик/с: выбросы
        // отброшены, а не «размазаны».
        assert!(
            (stats.average_throughput - 5000.0).abs() < 60.0,
            "среднее искажено выбросами: {}",
            stats.average_throughput
        );
        assert!(
            stats.consistency_percent > 99.0,
            "стабильность после усечения должна остаться высокой, получено {}",
            stats.consistency_percent
        );
    }

    /// Чистые данные не должны пострадать от усечения.
    ///
    /// Само по себе усечение безусловно снимает 2,5 % с каждого края, поэтому на
    /// чистых данных `excluded_fraction` — те же ≈5 %. Важно другое: среднее и
    /// стабильность от этого не меняются, потому что симметричный разброс не
    /// смещает ни среднее, ни σ.
    #[test]
    fn trimming_is_harmless_without_outliers() {
        let n = 10_000;
        // Разброс ±0,01 мс вокруг 0,2 — обычный шум замера.
        let clean: Vec<f64> = (0..n)
            .map(|i| 0.2 + if i % 2 == 0 { 0.01 } else { -0.01 })
            .collect();
        let stats = run_stats(&clean).expect("есть валидные сэмплы");
        assert!(
            (stats.average_throughput - 5000.0).abs() < 5.0,
            "среднее должно остаться точным: {}",
            stats.average_throughput
        );
        // ±0,01 мс на тике по 0,2 мс — разброс ±5 %, σ/μ около 5 %, стабильность
        // около 95. Выше она быть не может: это свойство самих данных, а не
        // усечения. Проверяем именно относительную величину — об этом весь смысл
        // усечения.
        assert!(
            stats.consistency_percent > 94.0,
            "чистые данные должны остаться стабильными, получено {}",
            stats.consistency_percent
        );
        assert!(
            (stats.excluded_fraction - 0.05).abs() < 0.01,
            "на чистых данных исключается те же 5 %, а не «найденные выбросы»: {}",
            stats.excluded_fraction
        );
        // Главное: усечение не должно ухудшать и без того чистые данные —
        // стабильность с выбросами обязана быть заметно ХУЖЕ, а не лучше.
        let noisy = run_stats(&realistic_series(10_000, 200, 20.0)).expect("есть валидные сэмплы");
        assert!(
            noisy.consistency_percent > stats.consistency_percent + 3.0,
            "данные с выбросами обязаны быть менее стабильными: {} против {}",
            noisy.consistency_percent,
            stats.consistency_percent
        );
    }

    /// Верхние перцентили времени НЕ должны вычисляться по усечённому набору.
    ///
    /// Усечение снимает самые медленные тики, то есть ровно тот хвост, который
    /// p95/p99 показывают. Посчитанные по усечённому набору, они всегда
    /// показывали бы «всё отлично» — и микрофризы в отчёте были бы не видны.
    #[test]
    fn upper_time_percentiles_see_the_slow_tail() {
        let n = 10_000;
        // 0,5 % тиков по 20 мс среди тиков по 0,2 мс.
        let with_outliers = realistic_series(n, 200, 20.0);
        let stats = run_stats(&with_outliers).expect("есть валидные сэмплы");
        // p95 в полном наборе — почти обычное время, потому что выбросов всего
        // 0,5 % и p95 до них не доходит.
        assert!(
            stats.p95_execution_time_ms < 1.0,
            "p95 не должен видеть 0,5 % выбросов, получено {}",
            stats.p95_execution_time_ms
        );
        // p99 — уже видит: выбросов 0,5 %, они попали в верхний процент.
        assert!(
            stats.p95_execution_time_ms <= stats.p99_execution_time_ms,
            "p95 должен быть не больше p99"
        );
        // А худшая секунда обязана быть кратно хуже средней: иначе провалы
        // полностью исчезают из отчёта.
        assert!(
            stats.worst_second_throughput < stats.average_throughput,
            "провал обязан отражаться на худшей секунде: {} против {}",
            stats.worst_second_throughput,
            stats.average_throughput
        );
    }

    /// На малых выборках усечение не применяется: иначе оно съедало бы
    /// основную часть данных и «стабильность» короткой фазы становилась бы
    /// характеристикой трёх сэмплов.
    #[test]
    fn small_samples_are_not_trimmed() {
        let short = vec![0.2, 0.2, 0.2, 5.0, 0.2, 0.2];
        let stats = run_stats(&short).expect("есть валидные сэмплы");
        assert_eq!(stats.samples, short.len());
        assert_eq!(stats.excluded_fraction, 0.0);
        assert_eq!(stats.samples_raw, short.len());
    }

    /// Граница усечения: при пороге выборки начинается отбрасывание ровно
    /// 2,5 % с каждого края.
    #[test]
    fn trim_keeps_the_middle_of_the_distribution() {
        let full: Vec<f64> = (0..1000).map(|i| i as f64).collect();
        // На пороге ровно 400 сэмплов — усечение применяется: floor(400*0.025)=10.
        let mut at_threshold = full.clone();
        at_threshold.truncate(MIN_TRIM_SAMPLES);
        let trimmed = trim_outliers(&at_threshold, TRIM_FRACTION);
        assert_eq!(trimmed.len(), 400 - 20);
        assert_eq!(trimmed[0], 10.0);
        assert_eq!(trimmed[trimmed.len() - 1], 389.0);
        // На один сэмпл ниже порога — не применяется вовсе.
        let mut below = full.clone();
        below.truncate(MIN_TRIM_SAMPLES - 1);
        assert_eq!(
            trim_outliers(&below, TRIM_FRACTION).len(),
            MIN_TRIM_SAMPLES - 1
        );
    }

    /// Усечение не может оставить меньше половины выборки: это уже не
    /// устойчивая оценка, а произвольно выбранный подотрезок.
    #[test]
    fn trimming_refuses_absurd_fractions() {
        let v: Vec<f64> = (0..1000).map(|i| i as f64).collect();
        // 40 % с каждого края оставили бы middle 20 % — среднее там ничего
        // не говорит о хвостах распределения.
        assert_eq!(trim_outliers(&v, 0.4).len(), 1000);
        assert_eq!(trim_outliers(&v, 0.26).len(), 1000);
        // Ровно 25 % с каждого края оставляет половину — предел допустимого.
        assert_eq!(trim_outliers(&v, 0.25).len(), 500);
        // Отрицательная доля и ноль — тоже без усечения.
        assert_eq!(trim_outliers(&v, -0.1).len(), 1000);
        assert_eq!(trim_outliers(&v, 0.0).len(), 1000);
    }

    /// P01 не зависит от длины фазы — ради этого он и переведён на окна.
    ///
    /// Раньше 0,001-й перцентиль по всем сэмплам при n = 2000 попадал на
    /// второй с конца сэмпл, а при n = 20000 — на двадцатый: «худший момент»
    /// улучшался тем длиннее фаза. Один и тот же отрезок работы, показанный
    /// дважды, должен давать ту же величину.
    #[test]
    fn p01_does_not_improve_with_phase_length() {
        // Одна секунда нормальной работы с одним коротким провалом.
        let base: Vec<f64> = (0..1000).map(|_| 0.2).collect();
        let mut one_second = base.clone();
        one_second[500] = 4.0; // провал на 100-мс окне
        // Тот же отрезок, показанный трижды: фаза втрое длиннее.
        let mut three_seconds = Vec::new();
        for _ in 0..3 {
            three_seconds.extend_from_slice(&one_second);
        }
        assert_eq!(one_second.len() * 3, three_seconds.len());

        let a = p01_throughput(&one_second);
        let b = p01_throughput(&three_seconds);
        assert!(
            (a - b).abs() / a.max(1.0) < 0.05,
            "P01 зависит от длины фазы: {a} против {b}"
        );
        // И он правда ловит провал, а не усредняет его.
        assert!(a < 4900.0, "P01 должен отражать худшее окно, получено {a}");
    }

    /// Окна throughput считаются по накопленному времени, а не по времени
    /// тика: иначе все тики короче окна попали бы в нулевое окно.
    #[test]
    fn windows_are_counted_by_elapsed_time() {
        // 10 тиков по 10 мс = ровно одно 100-мс окно.
        let times = vec![10.0; 10];
        let windows = windowed_throughput(&times, P01_WINDOW_MS, 5);
        assert_eq!(windows.len(), 1);
        assert!((windows[0] - 100.0).abs() < 1e-9, "100 тик/с в окне 100 мс");
    }

    /// Окна с малым числом сэмплов отбрасываются: неполное последнее окно
    /// дало бы «худшую секунду» из двух точек, и величина зависела бы от того,
    /// где закончилась фаза.
    ///
    /// Оговорка: хуже типичного окно всё равно остаётся — см. регресс M1 ниже.
    #[test]
    fn windows_without_enough_samples_are_dropped() {
        // 9 тиков по 10 мс: первое окно полное (10 сэмплов), второе — 9,
        // но при пороге 10 второе отбрасывается, первое тоже (ровно 10, проходит).
        let full = windowed_throughput(&[10.0; 9], P01_WINDOW_MS, 5);
        assert!(!full.is_empty());
        let strict = windowed_throughput(&[10.0; 9], P01_WINDOW_MS, 50);
        assert!(
            strict.is_empty(),
            "порог выше числа сэмплов отбрасывает всё"
        );
    }

    /// Регресс M1: микрофриз не должен исчезать из-за фильтра `min_samples`.
    ///
    /// Задержка, занявшая окно целиком, попадает в него В ОДИН-ЕДИНСТВЕННЫЙ
    /// сэмпл: соседние быстрые тики уже отнесены к другим окнам, и следующий
    /// тик перескакивает через границу. Фильтр «мало сэмплов — окно вон» его
    /// отбрасывал, и P01 показывал норму вместо провала: метрика, обещанная
    /// как ловушка кратковременной остановки, молчала именно о ней.
    ///
    /// Сценарий правдоподобный: тик 25 мс (40 тик/с) — это как раз тот случай,
    /// когда в 100-мс окне всего четыре сэмпла, то есть когда фильтр и умеет
    /// отбрасывать. Задержка 80 мс после ровно четырёх тиков приходится на
    /// начало окна и остаётся в нём одна.
    #[test]
    fn a_micro_freeze_survives_the_min_samples_filter() {
        const TICK_MS: f64 = 20.0;
        const FREEZE_MS: f64 = 80.0;
        let typical = 1000.0 / TICK_MS;

        let mut times = vec![TICK_MS; 400];
        times[4] = FREEZE_MS; // ровно на границе окна: elapsed = 100 мс

        let windows = windowed_throughput(&times, P01_WINDOW_MS, 5);
        let worst = windows.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(
            worst < typical * STALL_KEEP_RATIO,
            "окно с микрофризом отброшено: худшее окно {worst} тик/с при типичных {typical:.0}"
        );

        // Контроль: без задержки те же окна дают ровно типичный уровень. Значит
        // падение ниже вызвано именно микрофризом, а не тем, что окна с малым
        // числом сэмплов вдруг стали попадать в выборку всегда.
        let control = windowed_throughput(&vec![TICK_MS; 400], P01_WINDOW_MS, 5);
        let control_worst = control.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(
            (control_worst - typical).abs() < 1e-6,
            "контроль: ровная фаза должна давать типичный уровень, получено {control_worst}"
        );
        assert!(
            worst < control_worst / 2.0,
            "микрофриз не изменил худшее окно: {worst} против {control_worst}"
        );

        let p01 = p01_throughput(&times);
        assert!(
            p01 < typical * STALL_KEEP_RATIO,
            "P01 не увидел микрофриз: {p01} при типичных {typical:.0}"
        );
    }

    /// Регресс M1 для «худшей секунды»: там окно в десять раз крупнее, а
    /// задержка — того же порядка, что и весь обычный тик, и она снова
    /// оказывается одна в своём окне.
    #[test]
    fn a_long_stall_survives_the_filter_for_the_worst_second() {
        const TICK_MS: f64 = 20.0;
        const STALL_MS: f64 = 800.0;
        let typical = 1000.0 / TICK_MS;

        let mut times = vec![TICK_MS; 400];
        times[50] = STALL_MS; // elapsed = 1000 мс, начало окна в 1 с

        let windows = windowed_throughput(&times, WORST_SECOND_WINDOW_MS, 20);
        let worst = windows.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(
            worst < typical * STALL_KEEP_RATIO,
            "окно с остановкой отброшено: худшая секунда {worst} тик/с при типичных {typical:.0}"
        );

        let worst_second = worst_second_throughput(&times);
        assert!(
            worst_second < typical * STALL_KEEP_RATIO,
            "худшая секунда не увидела остановку: {worst_second} при типичных {typical:.0}"
        );
    }

    /// Обратная сторона: filter не должен сохранять окна, которые не хуже
    /// типичного. Иначе «микрофризом» станет любой неполный хвост фазы, и
    /// P01 перестанет зависеть от того, где закончилась фаза.
    #[test]
    fn an_ordinary_tail_window_is_not_mistaken_for_a_stall() {
        // Медленная, но ровная фаза: 20 мс на тик, окно 100 мс, порог 5.
        // В окно попадает ровно 5 сэмплов, окно обычное, выбросов нет.
        let even = vec![20.0f64; 50];
        let windows = windowed_throughput(&even, P01_WINDOW_MS, 5);
        assert!(
            windows.iter().all(|w| (w - 50.0).abs() < 1e-9),
            "окна ровной фазы искажены: {windows:?}"
        );
        // Неполный хвост (5 сэмплов по 20 мс = 100 мс, затем 2 тика) не должен
        // ни добавлять окно с выбросом, ни выбрасывать настоящие.
        let mut tail = even.clone();
        tail.extend_from_slice(&[20.0, 20.0]);
        let with_tail = windowed_throughput(&tail, P01_WINDOW_MS, 5);
        let plain_tp = with_tail.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(
            (plain_tp - 50.0).abs() < 1e-9,
            "хвост фазы изменил худшее окно: {plain_tp} при типичных 50"
        );
    }

    /// Регресс H1: P1 обязан считаться по ПОЛНОЙ выборке.
    ///
    /// Усечение снимает по 2,5 % с каждого края ВРЕМЁН, поэтому 1-й перцентиль
    /// throughput усечённого набора — это 3,5-й перцентиль полного. Метрика,
    /// обещанная как «худший 1 % тиков», показывала совсем другую величину и
    /// завышала её.
    ///
    /// Серия линейно-градуирована по времени: времена 1.000 … 10.999 мс, значит
    /// throughput строго убывает, и оба ответа можно посчитать руками.
    #[test]
    fn p1_is_taken_from_the_full_sample_not_from_the_trimmed_one() {
        let n = 10_000usize;
        let times: Vec<f64> = (0..n).map(|i| 1.0 + i as f64 * 0.001).collect();
        let stats = run_stats(&times).expect("есть валидные сэмплы");
        assert!(
            stats.samples < n,
            "тест бессмысленен без усечения: {} из {}",
            stats.samples,
            n
        );

        // По возрастанию throughput элемент с индексом i — это время
        // `10.999 − i·0.001`, то есть МЕДЛЕННЫЕ тики.
        let throughput_at = |i: f64| 1000.0 / (10.999 - i * 0.001);
        let interp = |pos: f64| {
            let (lo, frac) = (pos.floor(), pos - pos.floor());
            throughput_at(lo) + (throughput_at(lo + 1.0) - throughput_at(lo)) * frac
        };

        // Полный набор: pos = 0.01 · 9999 = 99.99.
        let from_full = interp(99.99);
        assert!(
            (stats.p1_throughput - from_full).abs() < 1e-6,
            "P1 = {} вместо 1-го перцентиля полного набора ({from_full})",
            stats.p1_throughput
        );

        // Усечённый набор: отброшено по 250 с краёв (всего 9500), его индекс j
        // — это индекс 250 + j полного. pos = 0.01 · 9499 = 94.99.
        let from_trimmed = interp(250.0 + 94.99);
        assert!(
            (stats.p1_throughput - from_trimmed).abs() > 1.0,
            "P1 посчитан по усечённому набору: {} (3,5-й перцентиль = {from_trimmed})",
            stats.p1_throughput
        );
        // И P1 ниже медианы: худшие тики он по-прежнему видит.
        assert!(stats.p1_throughput < stats.median_throughput);
    }

    /// Регресс H1 на настоящих данных замера: P1 обязан видеть медленный хвост,
    /// иначе он неотличим от p50 и перестаёт быть нижней границей диапазона.
    #[test]
    fn p1_still_sees_the_slow_tail() {
        // 2 % самых медленных тиков по 20 мс среди тиков по 0,2 мс.
        let with_outliers = realistic_series(10_000, 50, 20.0);
        let stats = run_stats(&with_outliers).expect("есть валидные сэмплов");
        assert_eq!(stats.samples_raw, 10_000);
        assert!(
            stats.p1_throughput < stats.median_throughput * 0.9,
            "P1 = {} не отличается от медианы {} — хвост не виден",
            stats.p1_throughput,
            stats.median_throughput
        );
    }

    /// На коротких сериях усечения нет вовсе, поэтому P1 обязан совпадать с
    /// расчётом по полному набору — регресс H1 не должен ломать малые данные.
    #[test]
    fn p1_on_short_series_is_the_full_sample_percentile() {
        let short: Vec<f64> = (0..50).map(|i| 1.0 + i as f64 * 0.01).collect();
        let stats = run_stats(&short).expect("есть валидные сэмплы");
        assert_eq!(stats.samples, short.len());
        let mut by_hand = short.clone();
        by_hand.sort_by(|a, b| a.total_cmp(b));
        let mut throughput: Vec<f64> = by_hand.iter().map(|ms| 1000.0 / ms).collect();
        throughput.sort_by(|a, b| a.total_cmp(b));
        assert_eq!(stats.p1_throughput, percentile_sorted(&throughput, 0.01));
    }

    // ------------------------------------------------------------------
    // Регресс C10 / M10: джиттер по хронологическим соседям
    // ------------------------------------------------------------------

    /// Регресс C10: джиттер обязан считаться по хронологической выборке.
    ///
    /// `trim_outliers` возвращает **отсортированный** массив. Если посчитать по
    /// нему разности соседей, они окажутся зазорами между соседними значениями,
    /// то есть плотностью выборки. Для тиков по 0,2 мс это ~1e-5 мс вместо
    /// настоящих скачков — занижение на порядки.
    #[test]
    fn jitter_is_not_computed_on_a_sorted_array() {
        let n = 10_000usize;
        // Тики по 0,2 мс, каждый 50-й — 20 мс: 2 % выбросов, то есть около 4 %
        // соседних пар. Этого достаточно, чтобы 99-й перцентиль разностей
        // попал именно в хвост, а не стоял ровно на его границе.
        let series = realistic_series(n, 50, 20.0);
        let stats = run_stats(&series).expect("есть валидные сэмплы");
        assert!(
            stats.samples < n,
            "усечение должно сработать, иначе тест не проверяет порядок: {}",
            stats.samples
        );

        // Настоящий хвост джиттера: разности вида |19.8 − 0.2| = 19.8 мс.
        assert!(
            stats.jitter_p99_ms > 1.0,
            "джиттер {} не видит скачков времени тика — посчитан по отсортированному набору",
            stats.jitter_p99_ms
        );
        // И он на порядки больше, чем зазор между соседями в отсортированном
        // массиве: 0,2 мс на 10 000 сэмплов даёт ~2e-5 мс.
        let sorted_gap = {
            let trimmed = trim_outliers(&filter_valid_times(&series), TRIM_FRACTION);
            let mut diffs: Vec<f64> = trimmed.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
            diffs.sort_by(|a, b| a.total_cmp(b));
            let at = (0.99 * (diffs.len() - 1) as f64) as usize;
            diffs[at]
        };
        assert!(
            stats.jitter_p99_ms > sorted_gap * 1000.0,
            "джиттер {} всего лишь в тысячу раз больше зазора {} — похоже на старую ошибку",
            stats.jitter_p99_ms,
            sorted_gap
        );
    }

    /// Джиттер по возрастающему ряду ровно ноль: соседи во времени не отличаются.
    ///
    /// Это же значение даёт и отсортированный набор — потому и ловится только
    /// на данных со скачками. Но на монотонном ряде метрика обязана быть нулём,
    /// то есть наивная сортировка не может «сработать» вместо хронологии.
    #[test]
    fn jitter_of_a_smooth_series_is_zero() {
        let smooth: Vec<f64> = (0..1000).map(|_| 0.2).collect();
        let stats = run_stats(&smooth).expect("есть валидные сэмплы");
        assert!(
            stats.jitter_p99_ms < 1e-12,
            "гладкий ряд дал джиттер {}",
            stats.jitter_p99_ms
        );
    }

    /// Хронологический скачок обязан попасть в джиттер: два тика 0,2 мс,
    /// затем один 20 мс, затем снова 0,2 мс — соседние различия 19,8 мс.
    ///
    /// Усечение здесь не срабатывает (< 400 сэмплов), поэтому проверка
    /// изолирует именно хронологию: если бы массив сортировали, скачок
    /// оказался бы в хвосте и соседних различий не дал.
    #[test]
    fn jitter_sees_a_single_chronological_spike() {
        // Тики по 0,2 мс, каждый 10-й — 20 мс. Хвост занимает 10 % соседних
        // пар, то есть заметно выше 1 %: 99-й перцентиль обязан в него попасть.
        //
        // Отдельный случай с одним скачком проверить нельзя: 2 выбросные пары
        // из 200 — это ровно 1 %, и 99-й перцентиль стоит на границе хвоста.
        let series: Vec<f64> = (0..200)
            .map(|i| if i % 10 == 0 { 20.0 } else { 0.2 })
            .collect();
        let stats = run_stats(&series).expect("есть валидные сэмплы");
        assert!(
            stats.jitter_p99_ms > 19.0,
            "хронологические скачки 19,8 мс не попали в джиттер (получено {})",
            stats.jitter_p99_ms
        );

        // По хронологии на каждом «выбросном» тике разность со следующим равна
        // 19,8 мс — это и есть то, что джиттер обязан показать.
        assert!((jitter_p99_ms(&series) - 19.8).abs() < 1e-9);
    }
}
