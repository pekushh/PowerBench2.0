//! Итоговый JSON-результат сессии: план, идентичность нагрузки, схемы
//! с агрегатами и прогонами, рекомендация (уровень + три вероятности).

use serde::{Deserialize, Serialize};

use powerbench_metrics::AggregateResult;
use powerbench_recommend::bootstrap::BootMode;
use powerbench_recommend::{EvidenceLevel, TieCriterion};

use crate::checkpoint::{Checkpoint, PhaseStats, StoredRun};

/// Строка рекомендации по схеме: идентификатор, активность, имя, агрегат, прогоны.
pub type RecommendationScheme = (
    String,
    bool,
    Option<String>,
    AggregateResult,
    Vec<StoredRun>,
);

/// Человекочитаемые подписи уровней доказательности.
pub fn evidence_level_label(level: EvidenceLevel) -> &'static str {
    match level {
        EvidenceLevel::Confirmed => "Подтверждено",
        EvidenceLevel::Probable => "Вероятно",
        EvidenceLevel::StabilityTieBreak => "Решено стабильностью",
        EvidenceLevel::Preliminary => "Предварительно",
        EvidenceLevel::Screening => "Скрининг",
        EvidenceLevel::KeepCurrent => "Оставить текущую",
        EvidenceLevel::Equivalent => "Эквиваленты",
        EvidenceLevel::None => "Нет данных",
    }
}

/// Уровень доказательности как машинный идентификатор.
pub fn evidence_level_id(level: EvidenceLevel) -> &'static str {
    match level {
        EvidenceLevel::Confirmed => "Confirmed",
        EvidenceLevel::Probable => "Probable",
        EvidenceLevel::StabilityTieBreak => "StabilityTieBreak",
        EvidenceLevel::Preliminary => "Preliminary",
        EvidenceLevel::Screening => "Screening",
        EvidenceLevel::KeepCurrent => "KeepCurrent",
        EvidenceLevel::Equivalent => "Equivalent",
        EvidenceLevel::None => "None",
    }
}

/// Режим bootstrap как строка.
pub fn bootstrap_mode_id(mode: BootMode) -> &'static str {
    match mode {
        BootMode::PairedByRun => "PairedByRun",
        BootMode::Independent => "Independent",
    }
}

/// Веса скоринга по умолчанию (производительность / стабильность / худшая
/// секунда, %). Источник значения — секция `scoring` настроек приложения.
///
/// Раньше здесь было 50/30/20. Средняя производительность и худшая секунда —
/// это один и тот же сигнал (throughput), просто взятый в разных точках
/// кривой, и на 50/30/20 вместе они получали 70 % веса. При этом среднее
/// определяется turbo-окном, где схемы питания ведут себя почти одинаково, а
/// расходятся они как раз на хвосте — там, где политика схемы (максимальное
/// состояние процессора, агрессивность разгона, охлаждение) уже вступила в
/// силу. Двадцать процентов на хвост недооценивали ровно то, ради чего замер
/// и делается.
///
/// 40/30/30: среднее остаётся заголовком, хвост получает равный голос,
/// стабильность сохраняет вес, но не перетягивает на себя итог — нестабильные
/// схемы и так отбрасываются автоматически.
pub fn default_score_weights() -> [f64; 3] {
    [40.0, 30.0, 30.0]
}

/// Признак разрешения ничьей как строка.
pub fn tie_criterion_id(criterion: TieCriterion) -> &'static str {
    match criterion {
        TieCriterion::P1 => "P1",
        TieCriterion::Stability => "Stability",
        TieCriterion::Cv => "Cv",
        TieCriterion::P01 => "P01",
        TieCriterion::Preference => "Preference",
        TieCriterion::None => "None",
    }
}

/// Идентичность нагрузки (CompatibilitySignature + детерминированные поля).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityJson {
    pub workload_version: String,
    pub config_hash: String,
    pub seed_hex: String,
    pub worker_count: usize,
    pub logical_cpus: usize,
    pub timer_hz: u64,
    pub cpu_identifier: String,
    pub diagnostics_version: String,
    /// Сборка и ревизия ОС (например, «22631.4169»).
    ///
    /// Обновление Windows меняет планировщик и политики питания, поэтому две
    /// сессии до и после патча нельзя считать сопоставимыми: раньше об этом
    /// можно было только догадываться.
    #[serde(default)]
    pub os_build: String,
    /// Объём физической памяти, ГБ.
    #[serde(default)]
    pub memory_gib: f64,
    /// Брендовое имя CPU для человека («AMD Ryzen 7 5800X3D»).
    #[serde(default)]
    pub cpu_brand: String,
}

impl IdentityJson {
    /// Идентичность по сигнатуре совместимости прогона.
    ///
    /// Нужна там, где важно совпадение полей для агрегирования, а не состав
    /// машины для человека: например, при поиске истории той же конфигурации.
    /// Поля ОС и памяти здесь пустые намеренно — их дополняет вызывающая сторона.
    pub fn from_signature(sig: &powerbench_metrics::CompatibilitySignature) -> Self {
        Self {
            workload_version: sig.workload_version.clone(),
            config_hash: sig.config_hash.clone(),
            seed_hex: format!("{:016X}", sig.seed),
            worker_count: sig.worker_count,
            logical_cpus: sig.logical_cpus,
            timer_hz: sig.timer_hz,
            cpu_identifier: sig.cpu_identifier.clone(),
            diagnostics_version: sig.diagnostics_version.clone(),
            os_build: String::new(),
            memory_gib: 0.0,
            cpu_brand: String::new(),
        }
    }
}

/// Одна схема в результате: агрегат + прогоны.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemeJson {
    pub scheme_id: String,
    /// Отображаемое имя плана (PlanName). Исключительно для вывода:
    /// идентификация ведётся только по `scheme_id` (guid).
    pub name: Option<String>,
    pub rejected: bool,
    pub rejection_reason: Option<String>,
    pub runs: usize,
    pub mean_average_throughput: f64,
    pub sample_std: f64,
    pub t_value: f64,
    pub margin: f64,
    pub ci_95: [f64; 2],
    pub run_variation_percent: f64,
    pub cv_warning: bool,
    pub median_throughput: f64,
    pub median_p1_throughput: f64,
    pub median_p01_throughput: f64,
    pub median_p95_execution_time_ms: f64,
    pub median_p99_execution_time_ms: f64,
    pub median_consistency_percent: f64,
    pub median_burst_retention_percent: f64,
    pub median_jitter_p99_ms: f64,
    /// Медиана худших секунд прогонов (тик/с).
    ///
    /// `#[serde(default)]` обязателен: поле добавили позже соседнего
    /// `median_background_purity` (который его получил), и без него записи
    /// истории, сделанные до этого коммита, переставали читаться целиком —
    /// они выпадали из базовой линии машины, экспорта и списка сессий.
    #[serde(default)]
    pub median_worst_window_throughput: f64,
    /// Медиана чистоты фона (%) — None, если данные недоступны.
    #[serde(default)]
    pub median_background_purity: Option<f64>,
    /// Метрики по фазам (медиана по прогонам).
    ///
    /// Три фазы измеряют разные вещи: лёгкая и «Отклик» реагируют на
    /// boost-политику схемы, тяжёлая упирается в лимиты мощности и может не
    /// отличаться вовсе. Усреднение их в одно число прячет случай, когда
    /// схема выигрывает в одной фазе и проигрывает в другой, — а это ровно
    /// тот вывод, который нужен пользователю, выбирающему схему.
    #[serde(default)]
    pub phases: Vec<PhaseSummaryJson>,
    pub run_duration_ms: u64,
    pub started_at_min_ns: u64,
    /// Прогоны по раундам. `#[serde(default)]` обязателен: поле появилось
    /// позже `phases`, и без него записи истории, сделанные до этого,
    /// переставали парситься целиком — вместе со всей сессией.
    #[serde(default)]
    pub per_run: Vec<StoredRun>,
}

impl SchemeJson {
    /// Нефинитное значение (`NaN`/`inf`) в JSON пишется как `null`, а поле
    /// объявлено как `f64` — такой файл потом не читается (`load_result`
    /// падает, запись навсегда теряется для истории и отчёта). Поэтому все
    /// метрики проходят через `finite_or_zero` на входе в DTO.
    pub fn from_aggregate(
        scheme_id: String,
        rejected: bool,
        rejection_reason: Option<String>,
        aggregate: &AggregateResult,
        per_run: Vec<StoredRun>,
    ) -> Self {
        Self::from_aggregate_with_base(
            scheme_id,
            rejected,
            rejection_reason,
            aggregate,
            per_run,
            0.0,
        )
    }

    /// То же, но с явной базой для отсчёта снижения частоты.
    ///
    /// База обязана быть общей на всю сессию. Считать её отдельно по схеме
    /// нельзя: у схемы, у которой просели все фазы, собственная база тоже
    /// просела, и падение показывалось как нулевое — ровно тот случай, ради
    /// которого флаг и нужен. Ноль означает «базы нет», тогда берётся лучшая
    /// частота по прогонам самой схемы.
    pub fn from_aggregate_with_base(
        scheme_id: String,
        rejected: bool,
        rejection_reason: Option<String>,
        aggregate: &AggregateResult,
        per_run: Vec<StoredRun>,
        frequency_base_mhz: f64,
    ) -> Self {
        let a = aggregate;
        Self {
            scheme_id,
            name: per_run.first().and_then(|r| r.scheme_name.clone()),
            rejected,
            rejection_reason,
            runs: a.runs,
            mean_average_throughput: finite_or_zero(a.mean_average_throughput),
            sample_std: finite_or_zero(a.sample_std),
            t_value: finite_or_zero(a.t_value),
            margin: finite_or_zero(a.margin),
            ci_95: [finite_or_zero(a.ci_95[0]), finite_or_zero(a.ci_95[1])],
            run_variation_percent: finite_or_zero(a.run_variation_percent),
            cv_warning: a.cv_warning,
            median_throughput: finite_or_zero(a.median_throughput),
            median_p1_throughput: finite_or_zero(a.median_p1_throughput),
            median_p01_throughput: finite_or_zero(a.median_p01_throughput),
            median_p95_execution_time_ms: finite_or_zero(a.median_p95_execution_time_ms),
            median_p99_execution_time_ms: finite_or_zero(a.median_p99_execution_time_ms),
            median_consistency_percent: finite_or_zero(a.median_consistency_percent),
            median_burst_retention_percent: finite_or_zero(a.median_burst_retention_percent),
            median_jitter_p99_ms: finite_or_zero(a.median_jitter_p99_ms),
            median_worst_window_throughput: finite_or_zero(a.median_worst_window_throughput),
            median_background_purity: a.median_background_purity.map(finite_or_zero),
            run_duration_ms: a.run_duration_ms,
            started_at_min_ns: a.started_at_min_ns,
            phases: phase_summaries(&per_run, frequency_base_mhz),
            per_run,
        }
    }
}

/// Медианы по фазам из прогонов схемы.
///
/// Фазы идентифицируются индексом, а имена берутся из канонического списка:
/// так порядок и подписи не зависят от того, в каком порядке фазы попали в
/// прогон.
///
/// Подпись фазы по индексу; неизвестный индекс (старый JSON) не выдумывает
/// подпись.
fn phase_label_for(index: u8) -> &'static str {
    match powerbench_core::config::Phase::from_index(index) {
        Some(p) => p.label(),
        None => "—",
    }
}

/// Порог, с которого падение частоты считается замеченным, % от лучшей
/// частоты, наблюдавшейся в этой же сессии.
///
/// Число выбрано так, чтобы ловить именно «просел и не вернулся», а не
/// обычный разброс счётчика частоты: на десктопах он прыгает на единицы
/// процентов сам по себе, и более строгий порог молчал бы на реальном
/// перегреве.
pub const FREQUENCY_DROP_ALERT_PERCENT: f64 = 5.0;

/// Медиана `current_mhz` по фазе, МГц; 0, если частоту не сообщали.
fn median_current_mhz(group: &[&PhaseStats]) -> f64 {
    let mut v: Vec<f64> = group
        .iter()
        .filter_map(|p| p.power.as_ref())
        .map(|p| p.current_mhz as f64)
        .filter(|x| *x > 0.0)
        .collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Самая высокая частота среди переданных прогонов — та, которую процессор
/// держал в нормальных условиях.
///
/// Именно она, а не `MaxMhz` из Windows, служит точкой отсчёта. `MaxMhz` —
/// текущий разрешённый потолок, в который одинаково входят штатный буст и
/// ручная настройка в BIOS, поэтому падение от него почти всегда означает
/// «у нас не разгон», а не «частоту снизили».
fn frequency_base<'a>(runs: impl Iterator<Item = &'a StoredRun>) -> f64 {
    runs.flat_map(|r| r.phases.iter())
        .filter_map(|p| p.power.as_ref())
        .map(|p| p.current_mhz as f64)
        .filter(|x| *x > 0.0)
        .fold(0.0f64, f64::max)
}

/// База по прогонам одной схемы — запасной вариант, когда база сессии
/// неизвестна.
fn session_frequency_base(per_run: &[StoredRun]) -> f64 {
    frequency_base(per_run.iter())
}

/// Медианы по фазам из прогонов схемы.
///
/// `session_base_mhz` — общая база сессии; ноль означает «считать по своим
/// прогонам».
fn phase_summaries(per_run: &[StoredRun], session_base_mhz: f64) -> Vec<PhaseSummaryJson> {
    let base = if session_base_mhz > 0.0 {
        session_base_mhz
    } else {
        session_frequency_base(per_run)
    };
    let mut out: Vec<PhaseSummaryJson> = Vec::new();
    for idx in 0..=u8::MAX {
        let group: Vec<&PhaseStats> = per_run
            .iter()
            .flat_map(|r| r.phases.iter())
            .filter(|p| p.phase_index == idx)
            .collect();
        if group.is_empty() {
            continue;
        }
        let med = |f: fn(&PhaseStats) -> f64| -> f64 {
            let mut v: Vec<f64> = group
                .iter()
                .map(|p| finite_or_zero(f(p)))
                .filter(|x| *x > 0.0)
                .collect();
            if v.is_empty() {
                return 0.0;
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            // Медиана по чётному числу значений — среднее двух центральных, как
            // в `metrics::median` и `history::median_of_values`. Раньше здесь
            // брался верхний центральный, и на двух прогонах пофазная таблица
            // показывала максимум там, где агрегат схемы показывал среднее.
            let n = v.len();
            if n % 2 == 1 {
                v[n / 2]
            } else {
                (v[n / 2 - 1] + v[n / 2]) / 2.0
            }
        };
        out.push(PhaseSummaryJson {
            name: phase_label_for(idx).to_string(),
            median_throughput: med(|p| p.stats.average_throughput),
            p1_throughput: med(|p| p.stats.p1_throughput),
            consistency_percent: med(|p| p.stats.consistency_percent),
            // Насколько фаза просела относительно лучшей частоты сессии.
            frequency_drop_percent: if base > 0.0 {
                let current = median_current_mhz(&group);
                if current > 0.0 {
                    ((base - current) / base * 100.0).max(0.0)
                } else {
                    0.0
                }
            } else {
                0.0
            },
            frequency_mhz: median_current_mhz(&group),
        });
    }
    out
}

/// Сводка по одной измеряемой фазе: медианы по прогонам.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PhaseSummaryJson {
    /// Название фазы («Лёгкая», «Тяжёлая», «Отклик», «Частичная»).
    pub name: String,
    /// Медиана throughput фазы, тик/с.
    pub median_throughput: f64,
    /// 1-й перцентиль throughput фазы (худшее поведение), тик/с.
    pub p1_throughput: f64,
    /// Медиана стабильности фазы, %.
    pub consistency_percent: f64,
    /// Насколько частота в этой фазе просела относительно лучшей частоты
    /// сессии, %. 0 — снижения не замечено.
    ///
    /// Раньше здесь был булев `throttled`, который означал «частота ниже
    /// потолка, который сообщает Windows». Формулировка вводила в заблуждение:
    /// на машине с разгоном в BIOS почти каждая фаза попадала в «троттлинг»,
    /// хотя никто ничего не ограничивал.
    #[serde(default)]
    pub frequency_drop_percent: f64,
    /// Медианная частота CPU в этой фазе, МГц; 0, если не сообщалась.
    #[serde(default)]
    pub frequency_mhz: f64,
}

impl PhaseSummaryJson {
    /// Замечено ли снижение частоты в этой фазе.
    pub fn frequency_dropped(&self) -> bool {
        self.frequency_drop_percent >= FREQUENCY_DROP_ALERT_PERCENT
    }
}

/// `NaN`/`inf` → `0.0`; конечные значения проходят без изменений.
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// Рекомендация в результате.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecommendationJson {
    pub level: String,
    pub level_label: String,
    pub recommended_scheme: Option<String>,
    pub runner_up_scheme: Option<String>,
    pub reason: String,
    pub probabilities: Option<[f64; 3]>,
    pub expected_margin_percent: Option<f64>,
    pub bootstrap_mode: Option<String>,
    pub tie_criterion: Option<String>,
}

/// Порог разброса опорной схемы, выше которого вердикт понижается.
///
/// Смысл: если сама эталонная схема «плавает» сильнее, чем различаются
/// участники, то любое ранжирование — шум. 1.5 % — заметно типичный разброс
/// повторов на одной машине, поэтому граница именно такая: ниже неё разницу
/// между схемами видно, выше — уже нет.
pub const REFERENCE_SPAN_LIMIT_PERCENT: f64 = 1.5;

/// Наибольшее число прогонов на схему, при котором сессия считается
/// скринингом. При одном прогоне доверительный интервал не строится вовсе
/// (k − 1 = 0), и любое утверждение «схема A лучше B» опирается на единственный
/// замер. Такой результат годится, чтобы отсеять явно слабые схемы, но не для
/// выбора победителя.
pub const SCREENING_MAX_RUNS: usize = 1;

/// Порог фоновой нагрузки (p95 за прогон), при котором результат считается
/// измеренным на загруженной машине. Задаётся в % одного ядра и умножается на
/// число логических CPU при сравнении.
pub const BACKGROUND_P95_NOTE_PERCENT: f64 = 20.0;

/// Фон, при котором уверенность в вердикте понижается.
///
/// 100 % — это целое занятое ядро, 200 % — два. Выше двух ядер фоновой
/// нагрузки разница между схемами питания уже неотличима от того, как
/// распределялись ресурсы между посторонними процессами, поэтому «Подтверждено»
/// и «Вероятно» здесь не выдаются.
pub const BLOCKING_BACKGROUND_P95_PERCENT: f64 = 200.0;
/// Во сколько раз ожидаемый размах прогонов должен превышать собственный CV,
/// чтобы дрейф считался настоящим.
///
/// Для нормального распределения ожидаемый размах выборки из n значений
/// примерно равен c_n * σ, где c_2 = 1.13, c_3 = 1.69, c_5 = 2.33.
/// Берётся консервативное c_5: порог не должен опускаться ниже размаха,
/// который статистически нормален для пяти раундов, — иначе детальный пресет
/// понижал бы уровень из-за шума самой машины.
pub const DRIFT_TREND_FROM_CV: f64 = 2.4;

/// Оценка стабильности машины по опорной схеме.
///
/// Сравнение «самой высокой из N схем» неявно опирается на то, что за все часы
/// замера машина вела себя одинаково. Допущение это нарушается постоянно (нагрев,
/// фон, обновления), а проверить его было нечем. Опорная схема меряется наравне
/// со всеми (дополнительного времени не требуется), а разброс её прогонов по
/// раундам показывает, насколько сама машина «плывёт» за сессию.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReferenceSummary {
    /// Идентификатор опорной схемы.
    pub scheme_id: String,
    /// Отображаемое имя опорной схемы.
    pub scheme_name: Option<String>,
    /// Throughput опорной схемы по раундам, тик/с.
    pub per_round: Vec<f64>,
    /// Размах по раундам относительно среднего, %.
    pub span_percent: f64,
    /// Наклон по раундам, % за раунд (линейный тренд).
    pub trend_percent_per_round: f64,
    /// Порог, при котором вердикт понижается: разброс эталона больше него
    /// означает, что машина плавает сильнее, чем различаются схемы.
    pub span_limit_percent: f64,
    /// Превышен ли порог.
    pub unstable: bool,
}

impl ReferenceSummary {
    /// Сводка по прогонам опорной схемы (уже отсортированным по раунду).
    ///
    /// `None`, если опорных прогонов меньше двух: по одному замеру дрейф не
    /// оценить, а выдавать за оценку ноль означало бы утверждать стабильность,
    /// которой никто не измерял.
    /// `span_limit_percent` — порог накопленного изменения.
    ///
    /// Решение принимается ПО ТРЕНДУ, а не по размаху. Размах растёт с числом
    /// раундов даже на идеально стабильной машине: ожидаемый размах выборки из
    /// n значений с разбросом σ равен примерно 1.13σ при n=2, 1.69σ при n=3 и
    /// 2.33σ при n=5. С постоянным порогом по размаху детальный пресет
    /// понижался до «Предварительно» чаще «Быстро», то есть шум машины
    /// трактовался как дрейф, а настоящий дрейф, «плавание» туда-сюда, вообще
    /// не отличался от него.
    ///
    /// Накопленное изменение по наклону прямой устойчивее: для шума его
    /// ожидаемая величина — около 0.65σ при n=5 и 1.15σ при n=3. Порог
    /// подстраивается под собственный разброс прогонов через
    /// [`DRIFT_TREND_FROM_CV`], размах остаётся в отчёте как характеристика
    /// разброса.
    pub fn build(
        scheme_id: &str,
        scheme_name: Option<String>,
        per_round: Vec<f64>,
        span_limit_percent: f64,
    ) -> Option<Self> {
        let vals: Vec<f64> = per_round
            .into_iter()
            .filter(|v| v.is_finite() && *v > 0.0)
            .collect();
        if vals.len() < 2 {
            return None;
        }
        let n = vals.len() as f64;
        let mean = vals.iter().sum::<f64>() / n;
        let max = vals.iter().cloned().fold(f64::MIN, f64::max);
        let min = vals.iter().cloned().fold(f64::MAX, f64::min);
        let span_percent = if mean > 0.0 {
            (max - min) / mean * 100.0
        } else {
            0.0
        };
        // Собственный разброс прогонов (CV) задаёт масштаб ожидаемого размаха.
        let cv_percent = if mean > 0.0 {
            let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
            (var.sqrt() / mean * 100.0).max(0.0)
        } else {
            0.0
        };
        let effective_limit = (span_limit_percent).max(DRIFT_TREND_FROM_CV * cv_percent);
        // Линейный тренд по индексам раундов: Σ(x−x̄)(y−ȳ) / Σ(x−x̄)².
        let x_mean = (n - 1.0) / 2.0;
        let mut num = 0.0;
        let mut den = 0.0;
        for (i, y) in vals.iter().enumerate() {
            let dx = i as f64 - x_mean;
            num += dx * (y - mean);
            den += dx * dx;
        }
        let slope = if den > 0.0 { num / den } else { 0.0 };
        let trend_percent_per_round = if mean > 0.0 {
            slope / mean * 100.0
        } else {
            0.0
        };
        let total_change_percent = trend_percent_per_round * (n - 1.0);
        Some(Self {
            scheme_id: scheme_id.to_string(),
            scheme_name,
            per_round: vals,
            span_percent,
            trend_percent_per_round,
            span_limit_percent: effective_limit,
            // Дрейф — накопленное изменение по тренду, а не размах: размах
            // растёт с числом раундов и на стабильной машине, тренд — нет.
            unstable: total_change_percent.abs() > effective_limit,
        })
    }

    /// Человекочитаемая сводка (для человека и для отчёта).
    ///
    /// Показываются обе величины: размах характеризует разброс, а решение о
    /// дрейфе принимается по накопленному изменению (тренду).
    pub fn note(&self) -> String {
        let total = self.trend_percent_per_round * (self.per_round.len() as f64 - 1.0);
        format!(
            "опорная схема: размах {:.1} % за прогонов (тренд {:+.1} %/раунд, \
             накопленное изменение {:+.1} %, порог {:.1} %)",
            self.span_percent, self.trend_percent_per_round, total, self.span_limit_percent
        )
    }
}

/// Итог сессии: JSON, готовый к записи на диск.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionJson {
    pub plan_guid: String,
    pub original_scheme_guid: Option<String>,
    pub original_restored: bool,
    pub identity: IdentityJson,
    pub schemes: Vec<SchemeJson>,
    pub recommendation: RecommendationJson,
    pub warnings: Vec<String>,
    /// Плановых раундов (повторов на схему) в сессии.
    #[serde(default)]
    pub rounds_planned: u32,
    /// Фактически выполненных раундов.
    #[serde(default)]
    pub rounds_completed: u32,
    /// Причина досрочного завершения (например, остановка вручную).
    #[serde(default)]
    pub early_stop_reason: Option<String>,
    /// Веса скоринга, действовавшие при сохранении (производительность,
    /// стабильность, худшая секунда), в процентах.
    #[serde(default)]
    pub score_weights: [f64; 3],
    /// Оценка дрейфа машины по опорной схеме.
    #[serde(default)]
    pub reference: Option<ReferenceSummary>,
    /// Режим скрининга: прогона на схему недостаточно для ранжирования,
    /// сессия годна только для отбраковки очевидно слабых схем.
    #[serde(default)]
    pub screening: bool,
}

/// Построить машинный JSON из контрольной точки и рекомендации.
#[allow(clippy::too_many_arguments)]
pub fn build_session_json(
    checkpoint: &Checkpoint,
    identity: IdentityJson,
    aggregated: Vec<(String, AggregateResult)>,
    recommendation_schemes: &[RecommendationScheme],
    recommendation: &powerbench_recommend::Recommendation,
    warnings: Vec<String>,
    cancelled: bool,
    score_weights: [f64; 3],
) -> SessionJson {
    let _ = &aggregated;
    // База отсчёта — одна на всю сессию: лучшая частота, которую процессор
    // держал в каких-либо фазах. Считать её отдельно по схеме нельзя: у
    // схемы, у которой просели все фазы, падение не было бы видно.
    let session_base = frequency_base(
        recommendation_schemes
            .iter()
            .flat_map(|(_, _, _, _, per_run)| per_run.iter()),
    );
    let schemes: Vec<SchemeJson> = recommendation_schemes
        .iter()
        .map(|(id, rejected, reason, agg, per_run)| {
            SchemeJson::from_aggregate_with_base(
                id.clone(),
                *rejected,
                reason.clone(),
                agg,
                per_run.clone(),
                session_base,
            )
        })
        .collect();
    let rounds_planned = checkpoint.plan.repetitions;
    let rounds_completed = {
        let mut rounds: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        for run in &checkpoint.runs {
            rounds.insert(run.round);
        }
        rounds.len() as u32
    };
    let rounds_completed = rounds_completed.min(rounds_planned);
    let early_stop_reason = if cancelled || rounds_completed < rounds_planned {
        Some(if cancelled {
            "сессия остановлена вручную".to_string()
        } else {
            "сессия завершена досрочно".to_string()
        })
    } else {
        None
    };
    // Дрейф по опорной схеме: её прогоны уже лежат в чекпоинте, отдельного
    // времени на эталон не тратится.
    let reference = checkpoint.plan.reference_scheme_id.as_ref().and_then(|id| {
        let mut per_round: Vec<(u32, f64)> = checkpoint
            .runs
            .iter()
            .filter(|r| r.scheme_id.eq_ignore_ascii_case(id))
            .map(|r| (r.round, r.combined.average_throughput))
            .collect();
        per_round.sort_by_key(|(r, _)| *r);
        let name = checkpoint
            .runs
            .iter()
            .find(|r| r.scheme_id.eq_ignore_ascii_case(id))
            .and_then(|r| r.scheme_name.clone());
        ReferenceSummary::build(
            id,
            name,
            per_round.into_iter().map(|(_, v)| v).collect(),
            REFERENCE_SPAN_LIMIT_PERCENT,
        )
    });
    let screening = aggregated
        .iter()
        .filter(|(_, a)| a.runs > 0)
        .map(|(_, a)| a.runs)
        .min()
        .map(|min_runs| min_runs <= SCREENING_MAX_RUNS)
        .unwrap_or(false);
    // Условия среды, способные обесценить вердикт, собираются здесь, а не в
    // вызывающем коде: и приложение, и CLI должны понижать одинаково.
    let mut warnings = warnings;
    let mut level = recommendation.level;
    if screening {
        // Собственное предупреждение здесь было бы третьим повтором одного и
        // того же текста: уровень «Скрининг» уже назван в шапке отчёта и в
        // плашке результата, а пояснение печатается рядом с условиями замера.
        level = EvidenceLevel::Screening;
    }
    if let Some(r) = &reference {
        // Дрейф машины больше, чем разница между схемами: любой уровень выше
        // «Предварительно» здесь был бы обещанием, которого замер не поддерживает.
        if r.unstable && matches!(level, EvidenceLevel::Confirmed | EvidenceLevel::Probable) {
            level = EvidenceLevel::Preliminary;
        }
        if r.unstable {
            warnings.push(format!(
                "машина нестабильна: {} — вердикт понижен",
                r.note()
            ));
        }
    }
    // Фон по каждому прогону: p95 вместо одной пробы 700 мс перед замером.
    let mut bg_p95 = 0.0f64;
    let mut bg_runs: usize = 0;
    for s in &schemes {
        for r in &s.per_run {
            if r.background_cpu_p95 > bg_p95 {
                bg_p95 = r.background_cpu_p95;
            }
            if r.background_sample_seconds > 0 {
                bg_runs += 1;
            }
        }
    }
    if bg_runs > 0 && bg_p95 > BACKGROUND_P95_NOTE_PERCENT {
        warnings.push(format!(
            "фон на загруженной машине: до {:.0} % CPU (пиковое, 95-й перцентиль прогона)",
            bg_p95
        ));
    }
    // Замечено снижение частоты: в отличие от сравнения с потолком Windows,
    // здесь отсчёт идёт от лучшей частоты самой сессии, поэтому на машине с
    // разгоном в BIOS пустых срабатываний не будет. Снижение частоты —
    // причина занизить результат, а не вина схемы, поэтому оно попадает в
    // предупреждения и понижает уровень до <Предварительно>.
    let dropped: Vec<String> = schemes
        .iter()
        .filter_map(|s| {
            let hit = s.phases.iter().filter(|p| p.frequency_dropped()).count();
            (hit > 0).then(|| {
                format!(
                    "{} ({} фаз)",
                    s.name.clone().unwrap_or(s.scheme_id.clone()),
                    hit
                )
            })
        })
        .collect();
    if !dropped.is_empty() {
        warnings.push(format!("замечено снижение частоты: {}", dropped.join(", ")));
    }
    if !dropped.is_empty() && matches!(level, EvidenceLevel::Confirmed | EvidenceLevel::Probable) {
        // Снижение частоты — причина занизить оценку: часть фазы измерялась
        // на пониженной частоте, и это не заслуга схемы питания.
        level = EvidenceLevel::Preliminary;
    }
    if bg_p95 >= BLOCKING_BACKGROUND_P95_PERCENT
        && matches!(level, EvidenceLevel::Confirmed | EvidenceLevel::Probable)
    {
        // Два занятых ядра фона — это уже не «фон», а соревнование за ресурс.
        // Порог в 2 × BACKGROUND_BLOCKING_PERCENT: тот же смысл, что и у
        // `BACKGROUND_BLOCKING_PERCENT`, но для пикового 95-го перцентиля.
        level = EvidenceLevel::Preliminary;
        warnings.push(format!(
            "уверенность понижена: фон {:.0} % CPU (95-й перцентиль) — машина \
             делила ресурс с посторонними процессами",
            bg_p95
        ));
    }
    SessionJson {
        plan_guid: checkpoint.plan.plan_guid.clone(),
        original_scheme_guid: checkpoint.original_scheme_guid.clone(),
        original_restored: checkpoint.original_restored,
        identity,
        schemes,
        recommendation: RecommendationJson {
            level: evidence_level_id(level).to_string(),
            level_label: evidence_level_label(level).to_string(),
            recommended_scheme: recommendation.recommended_scheme.clone(),
            runner_up_scheme: recommendation.runner_up_scheme.clone(),
            reason: recommendation.reason.clone(),
            probabilities: recommendation.three_probabilities.map(|p: [f64; 3]| p),
            expected_margin_percent: recommendation.expected_margin_percent,
            bootstrap_mode: recommendation
                .bootstrap_mode
                .map(bootstrap_mode_id)
                .map(String::from),
            tie_criterion: recommendation
                .tie_criterion
                .map(tie_criterion_id)
                .map(String::from),
        },
        warnings,
        rounds_planned,
        rounds_completed,
        early_stop_reason,
        score_weights,
        reference,
        screening,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Записи истории, сделанные до появления новых полей, должны читаться.
    ///
    /// Поле `median_worst_window_throughput` добавили без `#[serde(default)]` —
    /// и весь файл переставал парситься, из-за чего сессия исчезала из списка,
    /// экспорта и базовой линии машины. Проверяем именно это: JSON без
    /// последних полей обязан десериализоваться.
    #[test]
    fn history_record_without_late_fields_still_parses() {
        let json = r#"{
            "workload_version": "GamingCpuV1",
            "config_hash": "H",
            "seed_hex": "C52A202600000001",
            "worker_count": 4,
            "logical_cpus": 6,
            "timer_hz": 10000000,
            "cpu_identifier": "cpu",
            "diagnostics_version": "0.1.0"
        }"#;
        let id: IdentityJson = serde_json::from_str(json).expect("старый логин не читается");
        assert_eq!(id.worker_count, 4);
        assert!(id.os_build.is_empty());
        assert_eq!(id.memory_gib, 0.0);

        // Тот же контракт для схемы: отсутствующие поздние поля не должны
        // ронять десериализацию всего отчёта.
        let scheme = r#"{
            "scheme_id": "s1",
            "rejected": false,
            "runs": 3,
            "admitted": true,
            "mean_average_throughput": 100.0,
            "sample_std": 1.0,
            "t_value": 0.0,
            "margin": 0.0,
            "ci_95": [99.0, 101.0],
            "run_variation_percent": 1.0,
            "cv_warning": false,
            "median_throughput": 100.0,
            "median_p1_throughput": 90.0,
            "median_p01_throughput": 80.0,
            "median_p95_execution_time_ms": 1.0,
            "median_p99_execution_time_ms": 1.0,
            "median_consistency_percent": 95.0,
            "median_burst_retention_percent": 90.0,
            "median_jitter_p99_ms": 0.5,
            "run_duration_ms": 1000,
            "started_at_min_ns": 0
        }"#;
        let sch: SchemeJson =
            serde_json::from_str(scheme).expect("схема без новых полей не читается");
        assert_eq!(sch.median_worst_window_throughput, 0.0);
        assert!(sch.median_background_purity.is_none());
        assert!(sch.phases.is_empty());
    }

    /// Пустая статистика прогона для фикстур.
    fn run_stats_of(times: &[f64]) -> powerbench_metrics::run::RunStats {
        powerbench_metrics::run::run_stats(times).unwrap_or_else(|| {
            powerbench_metrics::run::run_stats(&[1.0]).expect("эталонная статистика")
        })
    }

    /// Медиана по чётному числу значений — среднее двух центральных.
    ///
    /// Иначе на двух прогонах пофазная таблица показывала максимум, тогда как
    /// агрегат по той же фазе — среднее, и таблица, ради которой всё и
    /// затевалось, противоречила итоговому числу.
    #[test]
    fn phase_median_of_two_runs_is_the_average() {
        let mk = |avg: f64| StoredRun {
            key: "0:g".to_string(),
            round: 0,
            scheme_id: "g".into(),
            scheme_name: None,
            started_at_ns: 0,
            duration_ms: 1,
            ticks: 1,
            supercycles: 0,
            first_tick_checksums: [1; crate::config::PHASES_PER_RUN as usize],
            run_checksums: [1; crate::config::PHASES_PER_RUN as usize],
            phases: vec![PhaseStats {
                phase_index: 0,
                stats: powerbench_metrics::run::RunStats {
                    average_throughput: avg,
                    ..run_stats_of(&[])
                },
                power: None,
            }],
            combined: run_stats_of(&[]),
            cross_phase_consistency: 0.0,
            burst_retention_percent: 0.0,
            background: Vec::new(),
            spike_windows: 0,
            power: None,
            background_cpu_p50: 0.0,
            background_cpu_p95: 0.0,
            background_sample_seconds: 0,
        };
        let summaries = phase_summaries(&[mk(900.0), mk(1100.0)], 0.0);
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].median_throughput, 1000.0,
            "медиана по двум прогонам должна быть средним двух значений"
        );
    }

    /// Снижение частоты считается от общей базы сессии.
    ///
    /// Проверяем ровно тот случай, который ломался при отсчёте внутри схемы:
    /// здоровая схема держит 5200 МГц, а у другой схемы частота просела до
    /// 4400 МГц. Считая базу по своей схеме, вторая схема получила бы 0 % и
    /// потеряла единственный признак, что её замер испорчен.
    #[test]
    fn frequency_drop_uses_session_base_not_scheme_base() {
        fn run_with_mhz(mhz: u32) -> StoredRun {
            StoredRun {
                key: "0:g".to_string(),
                round: 0,
                scheme_id: "g".into(),
                scheme_name: None,
                started_at_ns: 0,
                duration_ms: 1,
                ticks: 1,
                supercycles: 0,
                first_tick_checksums: [1; crate::config::PHASES_PER_RUN as usize],
                run_checksums: [1; crate::config::PHASES_PER_RUN as usize],
                phases: vec![PhaseStats {
                    phase_index: 0,
                    stats: run_stats_of(&[]),
                    power: Some(crate::checkpoint::PowerSnapshot {
                        max_mhz: mhz,
                        current_mhz: mhz,
                        throttled: false,
                        thermal_throttle: false,
                        policy_reason: 0,
                        on_ac: true,
                        unavailable: false,
                    }),
                }],
                combined: run_stats_of(&[]),
                cross_phase_consistency: 0.0,
                burst_retention_percent: 0.0,
                background: Vec::new(),
                spike_windows: 0,
                power: None,
                background_cpu_p50: 0.0,
                background_cpu_p95: 0.0,
                background_sample_seconds: 0,
            }
        }
        let slow = run_with_mhz(4400);
        let with_session_base = phase_summaries(std::slice::from_ref(&slow), 5200.0);
        let phase = with_session_base.first().expect("фаза не собрана");
        assert!(
            (phase.frequency_drop_percent - 15.38).abs() < 0.1,
            "падение посчитано как {} %, а не 15.38 %",
            phase.frequency_drop_percent
        );
        assert!(
            phase.frequency_dropped(),
            "падение в 15 % должно попадать под флаг"
        );
        assert_eq!(phase.frequency_mhz, 4400.0, "в таблице нужна сама частота");

        // Считаем базу по всем прогонам сессии — она берётся у здоровой фазы.
        let session_base = frequency_base([&slow, &run_with_mhz(5200)].into_iter());
        assert_eq!(session_base, 5200.0, "база сессии — лучшая частота");

        // Без базы сессии (0) отсчёт идёт по своей схеме: единственный замер
        // сам и есть база, и падение не должно выдумываться.
        let own_base = phase_summaries(&[slow], 0.0);
        assert_eq!(
            own_base.first().expect("фаза").frequency_drop_percent,
            0.0,
            "без базы сессии падение не должно выдумываться"
        );
    }

    /// Порог 5 %: ниже него падение не показывается.
    #[test]
    fn frequency_drop_threshold_is_five_percent() {
        let mut p = PhaseSummaryJson {
            name: "Лёгкая".into(),
            median_throughput: 500.0,
            p1_throughput: 100.0,
            consistency_percent: 90.0,
            frequency_drop_percent: 4.9,
            frequency_mhz: 5000.0,
        };
        assert!(!p.frequency_dropped(), "4.9 % — ниже порога");
        p.frequency_drop_percent = 5.0;
        assert!(
            p.frequency_dropped(),
            "5.0 % — ровно порог, флаг должен стоять"
        );
    }

    #[test]
    fn labels_map_to_russian() {
        assert_eq!(
            evidence_level_label(EvidenceLevel::Confirmed),
            "Подтверждено"
        );
        assert_eq!(evidence_level_label(EvidenceLevel::None), "Нет данных");
        assert_eq!(
            evidence_level_label(EvidenceLevel::KeepCurrent),
            "Оставить текущую"
        );
        assert_eq!(evidence_level_id(EvidenceLevel::None), "None");
        assert_eq!(bootstrap_mode_id(BootMode::Independent), "Independent");
        assert_eq!(tie_criterion_id(TieCriterion::Stability), "Stability");
    }

    #[test]
    fn probabilities_passed_through() {
        let _ = powerbench_recommend::bootstrap::BootstrapProbabilities {
            p_best: 0.9,
            p_margin_gt_0: 0.8,
            p_margin_gt_1pct: 0.7,
        };
    }

    /// Регресс: `NaN`/`inf` в агрегате (схема без прогонов) раньше писались
    /// в JSON как `null`, и запись истории становилась нечитаемой навсегда.
    #[test]
    fn non_finite_metrics_become_zero_and_survive_json() {
        let agg = AggregateResult {
            runs: 0,
            mean_average_throughput: f64::NAN,
            sample_std: f64::INFINITY,
            t_value: f64::NEG_INFINITY,
            margin: f64::NAN,
            ci_95: [f64::NAN, f64::INFINITY],
            run_variation_percent: f64::NAN,
            cv_warning: false,
            median_throughput: f64::NAN,
            median_p1_throughput: f64::NAN,
            median_p01_throughput: f64::NAN,
            median_p95_execution_time_ms: f64::NAN,
            median_p99_execution_time_ms: f64::NAN,
            median_consistency_percent: f64::NAN,
            median_burst_retention_percent: f64::NAN,
            median_jitter_p99_ms: f64::NAN,
            median_worst_window_throughput: f64::NAN,
            median_background_purity: Some(f64::NAN),
            run_duration_ms: 0,
            started_at_min_ns: 0,
        };
        let sch =
            SchemeJson::from_aggregate("g".into(), true, Some("брак".into()), &agg, Vec::new());
        // Метрики — числа: `null` означал бы, что файл не прочитается обратно.
        // `name` здесь `Option<String>`, его `null` допустим.
        let text = serde_json::to_string(&sch).expect("сериализация");
        for field in [
            "mean_average_throughput",
            "median_throughput",
            "run_variation_percent",
            "median_consistency_percent",
            "median_background_purity",
        ] {
            assert!(
                !text.contains(&format!("\"{field}\":null")),
                "поле {field} записано как null: {text}"
            );
        }
        let back: SchemeJson = serde_json::from_str(&text).expect("десериализация");
        assert_eq!(back.median_throughput, 0.0);
        assert_eq!(back.ci_95, [0.0, 0.0]);
        assert_eq!(back.median_background_purity, Some(0.0));
    }

    #[test]
    fn finite_metrics_pass_through_unchanged() {
        assert_eq!(finite_or_zero(12.5), 12.5);
        assert_eq!(finite_or_zero(-0.0), -0.0);
        assert_eq!(finite_or_zero(f64::NAN), 0.0);
        assert_eq!(finite_or_zero(f64::INFINITY), 0.0);
    }
}
