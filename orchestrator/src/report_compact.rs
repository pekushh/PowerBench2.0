//! Компактный HTML-отчёт по одной сессии.
//!
//! Формат — один экран, который читается целиком: вердикт, три ориентира,
//! одна таблица схем с поиском, фильтром и сортировкой, три карточки
//! диагностики. Всё остальное (подробности по фазам, причина брака) живёт в
//! раскрываемой строке схемы, поэтому первый экран отвечает на вопрос «какую
//! схему оставить», а не перечисляет замеры.
//!
//! Палитра — токены темы Graphite из `app/src/styles.css`. «Хорошее» при этом
//! показывается белым (`--accent`, #FDFDFD), а не зелёным: в интерфейсе акцент
//! Graphite белый, и зелёная метка в отчёте читалась как отдельный язык.
//! Жёлтый и красный оставлены — они несут смысл «просадка» и «брак», а не
//! «успех».

use crate::result::{PhaseSummaryJson, SchemeJson, SessionJson};

/// Сколько схем показывать до нажатия «Показать все».
const PAGE: usize = 25;
/// Сколько схем можно положить в лоток сравнения.
const CMP_MAX: usize = 4;
/// Порог «просадки» относительно лидера, %.
const DROP_LIMIT_PERCENT: f64 = 2.0;
/// Подписи фаз в каноническом порядке: 0 = Лёгкая, 1 = Частичная,
/// 2 = Тяжёлая, 3 = Отклик.
const PHASE_LABELS: [&str; 4] = ["Лёгкая", "Частичная", "Тяжёлая", "Отклик"];
const PHASE_LABELS_LONG: [&str; 4] = ["Лёгкая фаза", "Частичная фаза", "Тяжёлая фаза", "Отклик"];

const F_DUP: i32 = 1;
const F_KEY: i32 = 2;
const F_PROB: i32 = 4;
const F_REJ: i32 = 8;

// ---------------------------------------------------------------------------
// Вспомогательные функции
// ---------------------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Экранировать строку JSON для встраивания внутрь `<script>`.
///
/// `serde_json::to_string` экранирует только кавычки и обратный слэш, но НЕ
/// `<`, `>` и `&`. HTML-парсер, впрочем, заканчивает `<script>` на первом
/// же `</script` — независимо от того, находится ли он внутри строки JSON или
/// нет. Имя схемы питания приходит из `powercfg /list`, то есть это данные
/// извне: значение `</script><img src=x onerror=...>` вырывалось бы из строки и
/// превращалось в исполняемый HTML (регресс H46).
///
/// Заменяем на `\uXXXX`-последовательности: это валидный JSON, поэтому
/// `JSON.parse` продолжает работать без изменений на стороне интерфейса.
/// `\u2028`/`\u2029` — разделители строк JavaScript, которые ломают парсер
/// скрипта даже внутри строки.
fn json_escape(text: &str) -> String {
    serde_json::to_string(text)
        .unwrap_or_else(|_| "\"\"".to_string())
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// То же для готового JSON-массива: экранируются только его строковые
/// литералы, поэтому структура сохраняется.
fn json_escape_all<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[]".to_string())
}

fn f1(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.1}")
    } else {
        "—".to_string()
    }
}

fn f2(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.2}")
    } else {
        "0.00".to_string()
    }
}

fn pct1(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.1}%")
    } else {
        "—".to_string()
    }
}

fn pct0(v: f64) -> String {
    if v.is_finite() {
        format!("{}%", v.round() as i64)
    } else {
        "—".to_string()
    }
}

fn name_of(sch: &SchemeJson) -> String {
    sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone())
}

/// Схема реально измерена: не забракована, есть прогон и конечная медиана.
fn is_measured(sch: &SchemeJson) -> bool {
    !sch.rejected
        && sch.runs > 0
        && sch.median_throughput.is_finite()
        && sch.median_throughput > 0.0
}

/// Метрики фазы по индексу; нули, если фазы в схеме нет (старый JSON).
fn phase_triplet(sch: &SchemeJson, idx: usize) -> [f64; 3] {
    match sch.phases.get(idx) {
        Some(PhaseSummaryJson {
            median_throughput,
            p1_throughput,
            consistency_percent,
            ..
        }) => [*median_throughput, *p1_throughput, *consistency_percent],
        None => [0.0, 0.0, 0.0],
    }
}

fn delta_percent(value: f64, base: f64) -> Option<f64> {
    if !value.is_finite() || !base.is_finite() || base <= 0.0 {
        return None;
    }
    Some((value - base) / base * 100.0)
}

fn better<'a>(a: &'a SchemeJson, b: &'a SchemeJson, f: fn(&SchemeJson) -> f64) -> &'a SchemeJson {
    if f(b) > f(a) { b } else { a }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Схема, относительно которой считаются отклонения: опорная, иначе та, что
/// была активной при старте, иначе лидер по медиане.
// MSRV 1.85: схлопывание через let-цепочки требует Rust 1.88+.
#[allow(clippy::collapsible_if)]
fn reference_scheme<'a>(s: &'a SessionJson, active: Option<&'a str>) -> Option<&'a SchemeJson> {
    // Сравнение GUID — всегда регистронезависимое (регресс H48/H49): `powercfg`
    // отдаёт идентификаторы в верхнем регистре, а чекпоинт и настройки могут
    // хранить их в нижнем. Обычное `==` теряло опорную схему, и карточка
    // дрейфа молча показывала «опорная схема не задана».
    if let Some(r) = s.reference.as_ref() {
        if let Some(sch) = s
            .schemes
            .iter()
            .find(|x| x.scheme_id.eq_ignore_ascii_case(&r.scheme_id))
        {
            return Some(sch);
        }
    }
    if let Some(a) = active {
        if let Some(sch) = s
            .schemes
            .iter()
            .find(|x| x.scheme_id.eq_ignore_ascii_case(a))
        {
            return Some(sch);
        }
    }
    measured(s).into_iter().max_by(|a, b| {
        a.median_throughput
            .partial_cmp(&b.median_throughput)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn measured(s: &SessionJson) -> Vec<&SchemeJson> {
    s.schemes.iter().filter(|x| is_measured(x)).collect()
}

fn unique_names(s: &SessionJson) -> usize {
    let mut set = std::collections::HashSet::new();
    for sch in measured(s) {
        set.insert(name_of(sch).to_lowercase());
    }
    set.len()
}

// ---------------------------------------------------------------------------
// Строка таблицы
// ---------------------------------------------------------------------------

struct Row<'a> {
    sch: &'a SchemeJson,
    rank: usize,
    is_dup: bool,
    is_key: bool,
    is_prob: bool,
    is_rej: bool,
    drops_note: String,
    drops_count: usize,
}

/// Собрать строки таблицы, отсортированные по медиане, с флагами.
///
/// Ключевыми помечаются схемы, которые пользователь обязан увидеть даже при
/// фильтре «топ-15»: лидер по медиане, лидер по среднему, лучший по P1,
/// рекомендованная, активная и сами топ-15 по медиане. Без этого фильтр
/// «просадка / частота» выглядел бы как пустой раздел.
fn build_rows<'a>(s: &'a SessionJson, active: Option<&'a str>) -> Vec<Row<'a>> {
    let mut order: Vec<usize> = (0..s.schemes.len()).collect();
    order.sort_by(|a, b| {
        s.schemes[*b]
            .median_throughput
            .partial_cmp(&s.schemes[*a].median_throughput)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut key_ids: Vec<String> = Vec::new();
    let mark = |ids: &mut Vec<String>, id: &str| {
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    };
    let ms = measured(s);
    if let Some(top) = ms.iter().copied().reduce(|a, b| {
        if b.median_throughput > a.median_throughput {
            b
        } else {
            a
        }
    }) {
        mark(&mut key_ids, &top.scheme_id);
    }
    if let Some(top) = ms.iter().copied().reduce(|a, b| {
        if b.mean_average_throughput > a.mean_average_throughput {
            b
        } else {
            a
        }
    }) {
        mark(&mut key_ids, &top.scheme_id);
    }
    if let Some(top) = ms.iter().copied().reduce(|a, b| {
        if b.median_p1_throughput > a.median_p1_throughput {
            b
        } else {
            a
        }
    }) {
        mark(&mut key_ids, &top.scheme_id);
    }
    if let Some(r) = s.recommendation.recommended_scheme.as_deref() {
        mark(&mut key_ids, r);
    }
    if let Some(a) = active {
        mark(&mut key_ids, a);
    }
    for &i in order.iter().take(15) {
        mark(&mut key_ids, &s.schemes[i].scheme_id);
    }

    // Дубликаты: одинаковые имена, кроме лучшего результата в группе. Имя —
    // единственное, по чему различаются копии одной схемы у разных авторов.
    let mut best_in_group: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for &i in &order {
        let sch = &s.schemes[i];
        best_in_group
            .entry(name_of(sch).to_lowercase())
            .or_insert_with(|| sch.scheme_id.clone());
    }

    let leader = ms
        .iter()
        .map(|x| x.median_throughput)
        .filter(|v| v.is_finite())
        .fold(0.0f64, f64::max);

    let statics: Vec<Row<'a>> = order
        .iter()
        .enumerate()
        .map(|(rank, &i)| {
            let sch = &s.schemes[i];
            let key = name_of(sch).to_lowercase();
            let is_dup = best_in_group
                .get(&key)
                .map(|b| b != &sch.scheme_id)
                .unwrap_or(false);
            let drops: Vec<f64> = sch
                .phases
                .iter()
                .filter(|p| p.frequency_dropped())
                .map(|p| p.frequency_drop_percent)
                .collect();
            let drops_note = if drops.is_empty() {
                String::new()
            } else {
                let min = drops.iter().cloned().fold(f64::MAX, f64::min).round() as i64;
                let max = drops.iter().cloned().fold(0.0f64, f64::max).round() as i64;
                if min == max {
                    format!("{max} фаз")
                } else {
                    format!("{min}–{max} фаз")
                }
            };
            let is_prob = sch.rejected
                || !drops.is_empty()
                || delta_percent(sch.median_throughput, leader)
                    .map(|d| d < -DROP_LIMIT_PERCENT)
                    .unwrap_or(false);
            Row {
                sch,
                rank: rank + 1,
                is_dup,
                is_key: key_ids.contains(&sch.scheme_id),
                is_prob,
                is_rej: sch.rejected,
                drops_note,
                drops_count: drops.len(),
            }
        })
        .collect();
    statics
}

// ---------------------------------------------------------------------------
// Сборка
// ---------------------------------------------------------------------------

/// Компактный отчёт по одной сессии. `now_secs` — время генерации, локальное
/// время на машине, где открывают отчёт.
pub fn build(s: &SessionJson) -> String {
    let now = now_secs();
    let active = s.original_scheme_guid.as_deref();
    let rows = build_rows(s, active);
    let ref_sch = reference_scheme(s, active);
    let ref_med = ref_sch.map(|x| x.median_throughput).unwrap_or(0.0);
    let leader = measured(s)
        .iter()
        .map(|x| x.median_throughput)
        .filter(|v| v.is_finite())
        .fold(0.0f64, f64::max);
    let uniq = unique_names(s);

    let title = format!(
        "Отчёт PowerBench — {} ({})",
        s.identity.workload_version,
        session_local_time(s)
    );

    let mut h = String::new();
    h.push_str("<!DOCTYPE html><html lang=\"ru\"><head><meta charset=\"utf-8\">");
    h.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    h.push_str(&format!("<title>{}</title>", esc(&title)));
    h.push_str("<style>");
    h.push_str(css());
    h.push_str("</style></head><body><div class=\"sheet\">");
    h.push_str(&topbar_html(s));
    h.push_str("<div class=\"pad\">");
    h.push_str(&warnings_html(s));
    h.push_str(&hero_html(s, &rows, ref_sch, ref_med, active, uniq));
    h.push_str(&cmp_tray_html());
    h.push_str(&toolbar_html(s, uniq));
    h.push_str(&table_html(
        &rows,
        ref_sch,
        s.recommendation.recommended_scheme.as_deref(),
        ref_med,
        leader,
    ));
    h.push_str(&diag_html(s, &rows));
    h.push_str("</div>");
    h.push_str(&foot_html(s, now));
    h.push_str("</div><button type=\"button\" class=\"to-top\" aria-label=\"Наверх\">↑</button>");
    h.push_str("<script>");
    h.push_str(&script_html(s, ref_sch, uniq));
    h.push_str("</script></body></html>");
    h
}

/// Время сессии локальной строкой `ДД.ММ.ГГГГ ЧЧ:ММ:СС`.
fn session_local_time(s: &SessionJson) -> String {
    match crate::history::session_started_at_ns(s) {
        Some(ns) => powerbench_windows::power::local_time_string(ns / 1_000_000_000),
        None => "—".to_string(),
    }
}

fn topbar_html(s: &SessionJson) -> String {
    let id = &s.identity;
    let mut meta: Vec<String> = Vec::new();
    if !id.cpu_brand.is_empty() {
        meta.push(format!(
            "<b>{}</b> ({} {})",
            esc(&id.cpu_brand),
            id.logical_cpus,
            if id.logical_cpus == 1 {
                "ядро"
            } else {
                "ядер"
            }
        ));
    }
    if id.memory_gib > 0.0 {
        meta.push(format!("{:.0} ГБ", id.memory_gib));
    }
    if !id.os_build.is_empty() {
        meta.push(format!("ОС <b>{}</b>", esc(&id.os_build)));
    }
    if id.worker_count > 0 && id.logical_cpus > 0 {
        meta.push(format!(
            "Воркеры: <b>{}/{}</b>",
            id.worker_count, id.logical_cpus
        ));
    }
    meta.push(session_local_time(s));
    let meta_html = meta
        .iter()
        .map(|m| format!("<span>{}</span>", m))
        .collect::<Vec<_>>()
        .join("<span class=\"sep\">·</span>");

    format!(
        "<div class=\"topbar\"><div class=\"brand-wrap\">\
         <span class=\"logo\"><svg width=\"13\" height=\"13\" viewBox=\"0 0 24 24\">\
         <path d=\"M13 2 4 14h6l-1 8 9-12h-6l1-8z\" fill=\"#1B1B1B\"/></svg></span>\
         <span class=\"wordmark\">PowerBench</span><span class=\"sep\">·</span>\
         <span class=\"chip\"><svg width=\"14\" height=\"14\" viewBox=\"0 0 24 24\" fill=\"none\" \
         stroke=\"currentColor\" stroke-width=\"1.8\"><rect x=\"2\" y=\"7\" width=\"20\" height=\"11\" \
         rx=\"5.5\"/><circle cx=\"8.5\" cy=\"12.5\" r=\"1.2\" fill=\"currentColor\" stroke=\"none\"/>\
         <circle cx=\"15.5\" cy=\"12.5\" r=\"1.2\" fill=\"currentColor\" stroke=\"none\"/></svg>{}</span>\
         </div><div class=\"sess-meta\">{meta}</div></div>",
        esc(&id.workload_version),
        meta = meta_html,
    )
}

fn warnings_html(s: &SessionJson) -> String {
    let mut out = String::new();
    if let Some(reason) = s.early_stop_reason.as_deref() {
        out.push_str(&format!(
            "<div class=\"warn-box warn\"><b>Сессия прервана досрочно</b> ({}/{} раундов): {}</div>",
            s.rounds_completed,
            s.rounds_planned,
            esc(reason)
        ));
    }
    if s.screening {
        out.push_str(
            "<div class=\"warn-box warn\"><b>Скрининг.</b> Прогонов меньше, чем нужно для \
             доверительного интервала: ранжирование ориентировочное, вердикт не выдаётся.</div>",
        );
    }
    for w in &s.warnings {
        out.push_str(&format!("<div class=\"warn-box warn\">{}</div>", esc(w)));
    }
    out
}

fn hero_html(
    s: &SessionJson,
    rows: &[Row<'_>],
    ref_sch: Option<&SchemeJson>,
    ref_med: f64,
    active: Option<&str>,
    uniq: usize,
) -> String {
    let rec = &s.recommendation;
    let ms = measured(s);
    let top_med = ms
        .iter()
        .copied()
        .reduce(|a, b| better(a, b, |x| x.median_throughput));
    let top_p1 = ms
        .iter()
        .copied()
        .reduce(|a, b| better(a, b, |x| x.median_p1_throughput));

    let rank_of = |sch: &SchemeJson| -> usize {
        rows.iter()
            .find(|r| r.sch.scheme_id.eq_ignore_ascii_case(&sch.scheme_id))
            .map(|r| r.rank)
            .unwrap_or(0)
    };

    let card = |role: &str, badge: &str, sch: Option<&SchemeJson>| -> String {
        let Some(sch) = sch else {
            return String::new();
        };
        let d = match delta_percent(sch.median_throughput, ref_med) {
            None => ("—".to_string(), "d-n"),
            Some(v) if v > 0.0 => (format!("+{v:.1}%"), "d-p"),
            Some(v) if v < 0.0 => (format!("{v:.1}%"), "d-w"),
            Some(_) => ("база".to_string(), "d-n"),
        };
        format!(
            "<div class=\"pcard\" data-jump=\"{id}\" title=\"Нажмите, чтобы открыть схему в таблице\">\
             <div><div class=\"pcard-role\"><span>{role}</span><i>{badge}</i></div>\
             <div class=\"pcard-name\">{name}</div>\
             <div class=\"pcard-main\"><span class=\"pcard-val\">{med}</span>\
             <span class=\"pcard-unit\">тик/с · <span class=\"{cls}\">{d}</span></span></div></div>\
             <div class=\"pcard-sub\"><span>Стаб.: <b>{st}</b></span>\
             <span>Разброс: <b>{dev}</b></span><span>Ранг: <b>#{rank}</b></span></div></div>",
            id = esc(&sch.scheme_id),
            role = esc(role),
            badge = esc(badge),
            name = esc(&name_of(sch)),
            med = f1(sch.median_throughput),
            cls = d.1,
            d = esc(&d.0),
            st = pct1(sch.median_consistency_percent),
            dev = pct1(sch.run_variation_percent),
            rank = rank_of(sch),
        )
    };

    let role_current = if ref_sch
        .map(|c| active.is_some_and(|a| c.scheme_id.eq_ignore_ascii_case(a)))
        .unwrap_or(false)
    {
        "Текущая · Опорная"
    } else {
        "Опорная"
    };
    let mut podium = String::new();
    podium.push_str(&card(role_current, "#1 Среднее", ref_sch));
    podium.push_str(&card("Лидер по медиане", "#1 AVG", top_med));
    podium.push_str(&card("Самая стабильная", "#1 по P1", top_p1));

    let eyebrow = match (top_med, ref_sch) {
        (Some(t), Some(b)) => match delta_percent(t.median_throughput, b.median_throughput) {
            Some(d) if d.abs() < 1.0 => format!("Разница лидеров &lt; 1% ({d:+.1}%)"),
            Some(d) if d > 0.0 => format!("Лидер выше на {:.1}%", d),
            Some(d) => format!("Лидер ниже на {:.1}%", -d),
            None => "Сравнение недоступно".to_string(),
        },
        _ => "Сравнение недоступно".to_string(),
    };

    let title = match rec.recommended_scheme.as_deref() {
        Some(w) => {
            let wname = s
                .schemes
                .iter()
                .find(|x| x.scheme_id.eq_ignore_ascii_case(w))
                .map(name_of)
                .unwrap_or_else(|| w.to_string());
            format!("Рекомендуем: «{}»", wname)
        }
        None => rec.level_label.clone(),
    };

    let admitted = ms.len();
    let rejected = rows.iter().filter(|r| r.is_rej).count();
    let runs: usize = s.schemes.iter().map(|x| x.runs).sum();
    // Уверенность вердикта и перевес обязательны в сводке. Раньше подпись
    // уровня («Подтверждено» / «Предварительно») попадала в отчёт только когда
    // рекомендации не было, и при наличии схемы она пропадала совсем: оставался
    // заголовок «Рекомендуем: …» без указания, насколько он надёжен.
    let level = format!("Уверенность: <b>{}</b> · ", esc(&rec.level_label));
    let margin = rec
        .expected_margin_percent
        .map(|m| format!("Перевес: <b>{}</b> · ", pct1(m)))
        .unwrap_or_default();
    let stats = format!(
        "{level}{margin}Схем: <b>{}</b> (уник.: <b>{uniq}</b>) · Допущено: <b>{admitted}</b> · \
         Брак: <b{err}>{rejected}</b> · Прогонов: <b>{runs}</b> (по {rounds})",
        s.schemes.len(),
        err = if rejected > 0 {
            " style=\"color:var(--err)\""
        } else {
            ""
        },
        rounds = s.rounds_planned.max(1),
    );

    // Класс рамки вердикта. «Оставить текущую» и «равнозначны» — это не
    // провал, а нейтральный ответ: раньше они попадали в ветку «плохо» и
    // красная рамка противоречила тексту «менять схему не требуется».
    let cls = match rec.level.as_str() {
        "Confirmed" => "",
        "Probable" | "KeepCurrent" | "Equivalent" => "tie",
        _ => "bad",
    };
    format!(
        "<div class=\"hero-grid\"><div class=\"verdict-box {cls}\">\
         <div><div class=\"v-eyebrow\"><span>ВЕРДИКТ БЕНЧМАРКА</span><span>{eyebrow}</span></div>\
         <div class=\"v-title\">{title}</div><p class=\"v-desc\">{reason}</p></div>\
         <div class=\"v-stats\"><span>{stats}</span></div></div>\
         <div class=\"podium\">{podium}</div></div>",
        eyebrow = eyebrow,
        title = esc(&title),
        reason = esc(&rec.reason),
        stats = stats,
        podium = podium,
    )
}

fn cmp_tray_html() -> String {
    format!(
        "<div class=\"cmp-tray\" id=\"cmpTray\"><div class=\"cmp-head\">\
         <span class=\"cmp-title\">Сравнение схем (<span id=\"cmpCount\">0</span> из {CMP_MAX})</span>\
         <div class=\"cmp-actions\">\
         <button type=\"button\" class=\"btn-xs\" id=\"cmpPresetBtn\">Топ-3 ориентира</button>\
         <button type=\"button\" class=\"btn-xs\" id=\"cmpClearBtn\">Скрыть</button>\
         </div></div><div class=\"cmp-grid\" id=\"cmpGrid\"></div></div>"
    )
}

fn toolbar_html(s: &SessionJson, uniq: usize) -> String {
    let rejected = s.schemes.iter().filter(|x| x.rejected).count();
    let dups = s.schemes.len().saturating_sub(uniq);
    format!(
        "<div class=\"toolbar\"><div class=\"search-wrap\">\
         <input type=\"search\" class=\"sch-search\" id=\"schSearch\" placeholder=\"Поиск схемы по названию...\" \
         aria-label=\"Поиск по названию схемы\"><span class=\"kbd-hint\">/</span></div>\
         <div class=\"pill-group\" role=\"group\" aria-label=\"Фильтр схем\">\
         <button type=\"button\" class=\"f-btn active\" data-filter=\"all\" id=\"fAll\">Все ({total})</button>\
         <button type=\"button\" class=\"f-btn\" data-filter=\"key\">Топ-15 и ключевые</button>\
         <button type=\"button\" class=\"f-btn\" data-filter=\"prob\">Просадка / частота</button>\
         <button type=\"button\" class=\"f-btn\" data-filter=\"rejected\">Брак ({rejected})</button>\
         </div>\
         <label class=\"dup-toggle\" title=\"Скрыть {dups} дублирующихся копий схем с одинаковыми именами, \
         оставив лучший результат каждой\"><input type=\"checkbox\" id=\"hideDups\">\
         <span>Без дубликатов ({uniq})</span></label>\
         <button type=\"button\" class=\"btn-xs\" id=\"quickCmpBtn\" \
         title=\"Сравнить ключевые схемы сессии\">Сравнить топ-3</button>\
         </div><div class=\"toolbar-sub\"><div class=\"ph-switch\"><span>Колонки фаз:</span>\
         <div class=\"pill-group\">\
         <button type=\"button\" class=\"f-btn active acc\" data-phmode=\"0\">Медиана, тик/с</button>\
         <button type=\"button\" class=\"f-btn\" data-phmode=\"1\">P1 (худшая с), тик/с</button>\
         <button type=\"button\" class=\"f-btn\" data-phmode=\"2\">Стабильность, %</button>\
         </div></div><div class=\"sub-right\"><span id=\"schCount\" class=\"muted\"></span>\
         <button type=\"button\" class=\"btn-xs\" id=\"topShowAllBtn\"></button>\
         </div></div>",
        total = s.schemes.len(),
        dups = dups,
    )
}

fn table_html(
    rows: &[Row<'_>],
    ref_sch: Option<&SchemeJson>,
    rec_id: Option<&str>,
    ref_med: f64,
    leader: f64,
) -> String {
    let mut out = String::from(
        "<div class=\"table-wrap\"><table class=\"main-tbl\" id=\"mainTable\"><thead><tr>",
    );
    out.push_str("<th class=\"c-rank sortable sorted\" data-sort=\"med\" data-dir=\"&darr;\" title=\"Ранг по медиане\">#</th>");
    out.push_str("<th class=\"sortable\" data-sort=\"name\">Схема питания <span style=\"font-weight:400;text-transform:none;letter-spacing:0;color:var(--mute)\">(клик — все фазы)</span></th>");
    out.push_str("<th class=\"num sortable sorted\" data-sort=\"med\" data-dir=\"&darr;\">Медиана, тик/с</th>");
    out.push_str("<th class=\"num sortable\" data-sort=\"dev\" title=\"Разброс между прогонами\">Разброс</th>");
    out.push_str("<th class=\"num sortable\" data-sort=\"st\">Стаб.</th>");
    for (i, l) in PHASE_LABELS.iter().enumerate() {
        out.push_str(&format!(
            "<th class=\"num c-ph sortable\" data-sort=\"ph{i}\">{l}</th>"
        ));
    }
    out.push_str("<th class=\"c-cmp\" title=\"Добавить к сравнению\">+</th>");
    out.push_str("</tr></thead><tbody id=\"schBody\">");

    let base_id = ref_sch.map(|r| r.scheme_id.as_str()).unwrap_or("");
    for row in rows {
        let sch = row.sch;
        let mut flags = 0;
        if row.is_dup {
            flags |= F_DUP;
        }
        if row.is_key {
            flags |= F_KEY;
        }
        if row.is_prob {
            flags |= F_PROB;
        }
        if row.is_rej {
            flags |= F_REJ;
        }
        let p_attr: Vec<String> = (0..4)
            .flat_map(|i| {
                let t = phase_triplet(sch, i);
                [t[0], t[1], t[2]].map(f1).into_iter()
            })
            .collect();

        let (d_txt, d_cls) = match delta_percent(sch.median_throughput, ref_med) {
            None => ("—".to_string(), "d-n"),
            Some(d) if d > 0.0 => (format!("+{d:.1}%"), "d-p"),
            Some(d) if d < 0.0 => (format!("{d:.1}%"), "d-w"),
            Some(_) => ("база".to_string(), "d-n"),
        };
        let bar_pct = if leader > 0.0 && sch.median_throughput.is_finite() {
            (sch.median_throughput / leader * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        // Полоса сравнивает с лидером, а не с опорной: разрыв виден сразу, и
        // у схем в 2 раза хуже полоса действительно выглядит вдвое короче.
        let bar_cls = match delta_percent(sch.median_throughput, leader) {
            Some(d) if d < -DROP_LIMIT_PERCENT * 4.0 => "b-err",
            Some(d) if d < -DROP_LIMIT_PERCENT => "b-warn",
            _ => "",
        };
        let row_cls = match (row.is_rej, row.is_key) {
            (true, _) => "sr out",
            (false, true) => "sr win",
            _ => "sr",
        };

        let mut tags = String::new();
        if row.is_key {
            let mut parts: Vec<&str> = Vec::new();
            if sch.scheme_id.eq_ignore_ascii_case(base_id) {
                parts.push("ОПОРНАЯ");
            }
            if rec_id.is_some_and(|r| sch.scheme_id.eq_ignore_ascii_case(r)) {
                parts.push("РЕКОМЕНДУЕТСЯ");
            }
            if !parts.is_empty() {
                tags.push_str(&format!(
                    "<span class=\"tag ok\">{}</span>",
                    esc(&parts.join(" · "))
                ));
            }
        }
        if row.is_prob && !row.is_rej {
            let mut parts: Vec<&str> = Vec::new();
            if delta_percent(sch.median_throughput, leader)
                .map(|d| d < -DROP_LIMIT_PERCENT)
                .unwrap_or(false)
            {
                parts.push("ПРОСАДКА");
            }
            if row.drops_count > 0 {
                parts.push("↓ ЧАСТОТА");
            }
            if !parts.is_empty() {
                tags.push_str(&format!(
                    "<span class=\"tag warn\">{}</span>",
                    esc(&parts.join(" · "))
                ));
            }
        }
        if row.is_rej {
            tags.push_str("<span class=\"tag err\">БРАК</span>");
        }

        out.push_str(&format!(
            "<tr class=\"{row_cls}\" data-id=\"{id}\" data-med=\"{med}\" data-f=\"{flags}\" \
             data-p=\"{p}\"><td>{rank}</td><td><div class=\"nw\"><span class=\"stitle\">{name}</span>\
             {tags}</div></td><td><div class=\"mc\"><span class=\"mv\">{medtxt}</span>\
             <span class=\"md {dcls}\">{dtxt}</span></div><div class=\"mb\"><i class=\"{bcls}\" \
             style=\"width:{bar:.1}%\"></i></div></td><td>{dev}</td><td>{st}</td>{ph}\
             <td><button type=\"button\" class=\"cbtn\">+</button></td></tr>",
            row_cls = row_cls,
            id = esc(&sch.scheme_id),
            med = f2(sch.median_throughput),
            flags = flags,
            p = p_attr.join(","),
            rank = row.rank,
            name = esc(&name_of(sch)),
            tags = tags,
            medtxt = f1(sch.median_throughput),
            dcls = d_cls,
            dtxt = esc(&d_txt),
            bcls = bar_cls,
            bar = bar_pct,
            dev = pct1(sch.run_variation_percent),
            st = pct1(sch.median_consistency_percent),
            ph = (0..4)
                .map(|i| format!("<td>{}</td>", f1(phase_triplet(sch, i)[0])))
                .collect::<Vec<_>>()
                .join(""),
        ));
    }
    out.push_str(
        "</tbody></table><div class=\"more-bar\" id=\"moreWrap\">\
         <button type=\"button\" class=\"sch-more\" id=\"moreBtn\"></button></div></div>",
    );
    out
}

fn diag_html(s: &SessionJson, rows: &[Row<'_>]) -> String {
    format!(
        "<div class=\"diag-sec\">{}{}{}</div>",
        drift_card(s),
        background_card(s),
        quarantine_card(rows)
    )
}

fn drift_card(s: &SessionJson) -> String {
    let Some(r) = s.reference.as_ref() else {
        return "<div class=\"dcard\"><div class=\"dcard-head\"><span>Дрейф стенда</span></div>\
            <div class=\"muted\">Опорная схема не задана, дрейф не оценивался.</div></div>"
            .to_string();
    };
    let name = r.scheme_name.clone().unwrap_or_else(|| r.scheme_id.clone());
    let (tag, tag_cls) = if r.unstable {
        ("РАЗБРОС ВЫШЕ НОРМЫ", "warn")
    } else {
        ("В НОРМЕ", "ok")
    };
    let pills: String = r
        .per_round
        .iter()
        .enumerate()
        .map(|(i, v)| {
            format!(
                "<div class=\"dpill\"><small>Раунд {n}</small><b>{val}</b></div>",
                n = i + 1,
                val = f1(*v)
            )
        })
        .collect();
    // Регресс H43: тренд — это изменение НА ИНТЕРВАЛ, поэтому за сессию его надо
    // умножать на число интервалов, а не на число замеров. `n` замеров дают
    // `n − 1` интервалов, и ровно на `n − 1` умножается то же значение в
    // `ReferenceJson::total_change_percent` и в решении о стабильности
    // дрейфа. Прежнее умножение на `n` завышало накопленное изменение на целый
    // процент тренда — и карточка показывала не то, по чему выносился вердикт.
    let intervals = r.per_round.len().saturating_sub(1);
    let text = format!(
        "Размах <b>{}</b> за сессию (тренд {:+.1}%/раунд, накопленное {:+.1}%). {}",
        pct1(r.span_percent),
        r.trend_percent_per_round,
        r.trend_percent_per_round * intervals as f64,
        if r.unstable {
            "Разброс выше порога — машина плавает сильнее, чем различаются схемы, ранжирование ориентировочно."
        } else {
            "Разброс в пределах нормы — ранжирование устойчиво."
        }
    );
    format!(
        "<div class=\"dcard\"><div class=\"dcard-head\"><span>Дрейф стенда (Опорная схема)</span>\
         <span class=\"tag {tc}\">{tag}</span></div>\
         <div style=\"color:var(--dim);margin-bottom:4px\"><b>{name}</b> — {n} контрольных замера:</div>\
         <div class=\"drift-pills\">{pills}</div>\
         <div class=\"muted\" style=\"font-size:11.5px;line-height:1.45\">{text}</div></div>",
        tc = tag_cls,
        name = esc(&name),
        n = r.per_round.len(),
        pills = pills,
        text = text,
    )
}

fn background_card(s: &SessionJson) -> String {
    struct Acc {
        name: String,
        median: f64,
        peak: f64,
    }
    let mut acc: Vec<Acc> = Vec::new();
    let (mut p50, mut p95) = (0.0f64, 0.0f64);
    for sch in &s.schemes {
        for run in &sch.per_run {
            p50 = p50.max(run.background_cpu_p50);
            p95 = p95.max(run.background_cpu_p95);
            for p in &run.background {
                let key = p.name.trim().to_string();
                match acc.iter_mut().find(|a| a.name.eq_ignore_ascii_case(&key)) {
                    Some(a) => {
                        a.median = a.median.max(p.median_cpu_percent);
                        a.peak = a.peak.max(p.peak_cpu_percent);
                    }
                    None => acc.push(Acc {
                        name: key,
                        median: p.median_cpu_percent,
                        peak: p.peak_cpu_percent,
                    }),
                }
            }
        }
    }
    if acc.is_empty() {
        return "<div class=\"dcard\"><div class=\"dcard-head\"><span>Фон системы при провалах</span>\
            <span class=\"tag ok\">ЧИСТО</span></div><div class=\"muted\">Ни один процесс не превысил \
            порог фильтра фоновой нагрузки.</div></div>"
            .to_string();
    }
    acc.sort_by(|a, b| {
        b.peak
            .partial_cmp(&a.peak)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top: Vec<&Acc> = acc.iter().take(3).collect();
    let quiet: Vec<&Acc> = acc.iter().skip(3).filter(|a| a.peak < 5.0).collect();
    let quiet_peak = quiet.iter().map(|a| a.peak).fold(0.0f64, f64::max);

    let li = |a: &Acc| {
        let cls = if a.peak >= 100.0 { "err" } else { "warn" };
        format!(
            "<li><span>{}</span><span>обычно {} · <b style=\"color:var(--{})\">пик {}</b></span></li>",
            esc(&a.name),
            pct1(a.median),
            cls,
            pct0(a.peak)
        )
    };
    let mut out = format!(
        "<div class=\"dcard\"><div class=\"dcard-head\"><span>Фон системы при провалах</span>\
         <span class=\"tag warn\">ПИК ДО {peak}% ЯДРА</span></div>\
         <div class=\"muted\" style=\"margin-bottom:6px;font-size:11.5px\">Медианный фон CPU: \
         <b>{p50}%</b> (95-й перцентиль: {p95}% одного ядра). Заметные всплески процессов:</div>\
         <ul class=\"mini-list\">{top}</ul>",
        peak = p95.round() as i64,
        p50 = p50.round() as i64,
        p95 = p95.round() as i64,
        top = top.iter().map(|a| li(a)).collect::<String>(),
    );
    if !quiet.is_empty() {
        let names: Vec<String> = quiet.iter().map(|a| esc(&a.name)).collect();
        out.push_str(&format!(
            "<details class=\"clean-det\"><summary>Показать ещё {n} процессов с пиком &lt; 5% CPU\
             </summary><ul class=\"mini-list\" style=\"margin-top:5px\"><li>\
             <span class=\"muted\">{names}</span><span class=\"muted\">пик {peak}%</span></li></ul></details>",
            n = quiet.len(),
            names = names.join(", "),
            peak = quiet_peak,
        ));
    }
    out.push_str("</div>");
    out
}

fn quarantine_card(rows: &[Row<'_>]) -> String {
    let rejected: Vec<&Row> = rows.iter().filter(|r| r.is_rej).collect();
    let dropped: Vec<&Row> = rows.iter().filter(|r| r.drops_count > 0).collect();
    let tag = format!("{} БРАК · {} ЧАСТОТА", rejected.len(), dropped.len());
    let tag_cls = if rejected.is_empty() { "ok" } else { "err" };
    let mut out = format!(
        "<div class=\"dcard\"><div class=\"dcard-head\"><span>Карантин и снижение частоты</span>\
         <span class=\"tag {tc}\">{tag}</span></div>",
        tc = tag_cls,
        tag = esc(&tag),
    );
    if rejected.is_empty() && dropped.is_empty() {
        out.push_str(
            "<div class=\"muted\">Ни одна схема не забракована, снижения частоты не замечено.</div>",
        );
        out.push_str("</div>");
        return out;
    }
    if !rejected.is_empty() {
        out.push_str("<ul class=\"mini-list\">");
        for r in &rejected {
            out.push_str(&format!(
                "<li><span class=\"jump-link\" data-jump=\"{id}\"><b style=\"color:var(--err)\">{name}</b>\
                 </span><span class=\"muted\">{reason}</span></li>",
                id = esc(&r.sch.scheme_id),
                name = esc(&name_of(r.sch)),
                reason = esc(
                    r.sch
                        .rejection_reason
                        .as_deref()
                        .unwrap_or("причина не указана")
                ),
            ));
        }
        out.push_str("</ul>");
    }
    if !dropped.is_empty() {
        out.push_str(
            "<div class=\"muted\" style=\"margin-top:8px;padding-top:7px;\
             border-top:1px solid var(--line);font-size:11.5px;line-height:1.45\">\
             <b style=\"color:var(--warn)\">Снижение частоты в фазах:</b><br>",
        );
        let items: Vec<String> = dropped
            .iter()
            .take(8)
            .map(|r| {
                format!(
                    "<span class=\"jump-link\" data-jump=\"{}\">{}</span> ({})",
                    esc(&r.sch.scheme_id),
                    esc(&name_of(r.sch)),
                    esc(&r.drops_note)
                )
            })
            .collect();
        out.push_str(&items.join(", "));
        if dropped.len() > 8 {
            out.push_str(&format!(" и ещё {}", dropped.len() - 8));
        }
        out.push_str("</div>");
    }
    out.push_str("</div>");
    out
}

fn foot_html(s: &SessionJson, now: u64) -> String {
    let id = &s.identity;
    format!(
        "<div class=\"foot\"><span>Сгенерировано {now} · PowerBench</span><span>\
         план <span class=\"mono\">{plan}</span> · seed <span class=\"mono\">{seed}</span> · \
         хэш <span class=\"mono\">{hash}</span></span></div>",
        now = esc(&powerbench_windows::power::local_time_string(now)),
        plan = esc(&s.plan_guid),
        seed = esc(&id.seed_hex),
        hash = esc(&short_hash(&id.config_hash)),
    )
}

fn short_hash(hash: &str) -> String {
    if hash.len() <= 20 {
        return hash.to_string();
    }
    format!("{}…{}", &hash[..8], &hash[hash.len() - 8..])
}

// ---------------------------------------------------------------------------
// Скрипт
// ---------------------------------------------------------------------------

fn script_html(s: &SessionJson, ref_sch: Option<&SchemeJson>, uniq: usize) -> String {
    let ref_phase: Vec<String> = (0..4)
        .map(|i| {
            ref_sch
                .map(|r| f1(phase_triplet(r, i)[0]))
                .unwrap_or_else(|| "0".to_string())
        })
        .collect();

    // Причина брака и примечание о частоте уезжают в подробную строку. Ключи и
    // значения сериализуются, поэтому названия схем с кавычками не ломают JSON.
    let mut meta: Vec<String> = Vec::new();
    for sch in &s.schemes {
        let reason = sch.rejection_reason.clone().unwrap_or_default();
        let n = sch.phases.iter().filter(|p| p.frequency_dropped()).count();
        if reason.is_empty() && n == 0 {
            continue;
        }
        let note = if n == 0 {
            String::new()
        } else {
            format!("снижение частоты в {n} фаз")
        };
        meta.push(format!(
            "{}:{{\"r\":{},\"t\":{}}}",
            json_escape(&sch.scheme_id),
            json_escape(&reason),
            json_escape(&note)
        ));
    }

    format!(
        r##"
(function () {{
  var extraMeta = {{{meta}}};
  var refPhase = [{refphase}];
  var ALL_COUNT = {all};
  var UNIQ_COUNT = {uniq};
  var PAGE = {page};
  var CMP_MAX = {cmpmax};
  var tbody = document.getElementById('schBody');
  var rows = Array.prototype.slice.call(tbody.querySelectorAll('tr.sr'));
  var searchInput = document.getElementById('schSearch');
  var filterBtns = Array.prototype.slice.call(document.querySelectorAll('[data-filter]'));
  var hideDupsChk = document.getElementById('hideDups');
  var phModeBtns = Array.prototype.slice.call(document.querySelectorAll('[data-phmode]'));
  var sortHeaders = Array.prototype.slice.call(document.querySelectorAll('th.sortable'));
  var counterEl = document.getElementById('schCount');
  var moreWrap = document.getElementById('moreWrap');
  var moreBtn = document.getElementById('moreBtn');
  var topShowAllBtn = document.getElementById('topShowAllBtn');

  var showAll = false;
  var curFilter = 'all';
  var curPhMode = 0;
  var curSort = 'med';
  var curDir = -1;
  var openIds = {{}};
  var cmpIds = [];

  var rowCache = {{}};
  rows.forEach(function (tr) {{
    var id = tr.getAttribute('data-id');
    var title = tr.querySelector('.stitle').textContent;
    var flags = parseInt(tr.getAttribute('data-f'), 10);
    var cells = tr.children;
    rowCache[id] = {{
      tr: tr,
      id: id,
      rk: parseInt(cells[0].textContent, 10),
      title: title,
      nameLower: title.toLowerCase(),
      med: parseFloat(tr.getAttribute('data-med')),
      dev: parseFloat(cells[3].textContent),
      st: parseFloat(cells[4].textContent),
      isDup: (flags & {dup}) !== 0,
      isKey: (flags & {key}) !== 0,
      isProb: (flags & {prob}) !== 0,
      isRej: (flags & {rej}) !== 0,
      p: tr.getAttribute('data-p').split(',').map(parseFloat),
      phCells: [cells[5], cells[6], cells[7], cells[8]]
    }};
  }});

  function updatePhaseCells() {{
    var suffix = curPhMode === 2 ? '%' : '';
    rows.forEach(function (tr) {{
      var c = rowCache[tr.getAttribute('data-id')];
      for (var i = 0; i < 4; i++) {{
        c.phCells[i].textContent = c.p[i * 3 + curPhMode].toFixed(1) + suffix;
      }}
    }});
  }}

  function el(tag, cls, text) {{
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined && text !== null) e.textContent = text;
    return e;
  }}

  // Названия схем подставляются через textContent, а не склейкой строк: это
  // пользовательские названия, и html-запись дала бы XSS из отчёта.
  function buildDetailRow(c) {{
    var tr = el('tr', 'detail-row');
    var td = el('td');
    td.colSpan = 10;
    var box = el('div', 'detail-box');
    var grid = el('div', 'detail-grid');
    var labels = {labels};
    for (var i = 0; i < 4; i++) {{
      var med = c.p[i * 3];
      var p1 = c.p[i * 3 + 1];
      var st = c.p[i * 3 + 2];
      var base = refPhase[i];
      var diff = base > 0 ? ((med - base) / base) * 100 : 0;
      var card = el('div', 'd-phase');
      var title = el('div', 'dp-title');
      title.appendChild(el('span', '', labels[i]));
      var dspan = el('span', '', (diff >= 0 ? '+' : '') + diff.toFixed(1) + '% к опорной');
      dspan.style.color = Math.abs(diff) < 0.3 ? 'var(--mute)' : (diff > 0 ? 'var(--pos)' : 'var(--warn)');
      dspan.style.textTransform = 'none';
      dspan.style.letterSpacing = '0';
      title.appendChild(dspan);
      card.appendChild(title);
      var metrics = el('div', 'dp-metrics');
      [['Медиана', med.toFixed(1) + ' тик/с'],
       ['P1 (худшая с)', p1.toFixed(1) + ' тик/с'],
       ['Стабильность', st.toFixed(1) + '%']].forEach(function (pair) {{
        var row = el('div');
        row.appendChild(el('span', '', pair[0]));
        row.appendChild(el('b', '', pair[1]));
        metrics.appendChild(row);
      }});
      card.appendChild(metrics);
      grid.appendChild(card);
    }}
    box.appendChild(grid);
    var meta = extraMeta[c.id];
    if (meta && (meta.r || meta.t)) {{
      var alerts = el('div', 'detail-alerts');
      if (meta.r) alerts.appendChild(el('div', 'd-alert err', 'Карантин: ' + meta.r));
      if (meta.t) alerts.appendChild(el('div', 'd-alert warn',
        'Примечание: ' + meta.t + ' (частота ниже лучшей за сессию)'));
      box.appendChild(alerts);
    }}
    td.appendChild(box);
    tr.appendChild(td);
    return tr;
  }}

  function apply() {{
    Array.prototype.slice.call(tbody.querySelectorAll('tr.detail-row')).forEach(function (node) {{
      node.parentNode.removeChild(node);
    }});
    var q = (searchInput.value || '').toLowerCase().trim();
    var noDups = hideDupsChk.checked;
    document.getElementById('fAll').textContent = 'Все (' + (noDups ? UNIQ_COUNT : ALL_COUNT) + ')';

    var hit = rows.filter(function (tr) {{
      var c = rowCache[tr.getAttribute('data-id')];
      if (noDups && c.isDup) return false;
      if (q && c.nameLower.indexOf(q) < 0) return false;
      if (curFilter === 'key') return c.isKey;
      if (curFilter === 'prob') return c.isProb;
      if (curFilter === 'rejected') return c.isRej;
      return true;
    }});

    hit.sort(function (a, b) {{
      var ca = rowCache[a.getAttribute('data-id')];
      var cb = rowCache[b.getAttribute('data-id')];
      if (curSort === 'name') return curDir * ca.nameLower.localeCompare(cb.nameLower, 'ru');
      var av, bv;
      if (curSort === 'med' || curSort === 'dev' || curSort === 'st') {{
        av = ca[curSort]; bv = cb[curSort];
      }} else {{
        var phIdx = parseInt(curSort.charAt(2), 10);
        av = ca.p[phIdx * 3 + curPhMode];
        bv = cb.p[phIdx * 3 + curPhMode];
      }}
      if (av === bv) return ca.rk - cb.rk;
      return curDir * (av - bv);
    }});

    rows.forEach(function (tr) {{ tr.style.display = 'none'; }});
    var limit = showAll ? hit.length : Math.min(PAGE, hit.length);
    hit.forEach(function (tr, idx) {{
      tbody.appendChild(tr);
      var visible = idx < limit;
      tr.style.display = visible ? '' : 'none';
      var id = tr.getAttribute('data-id');
      var isOpen = !!openIds[id];
      tr.classList.toggle('open', isOpen);
      if (visible && isOpen) tbody.appendChild(buildDetailRow(rowCache[id]));
    }});

    counterEl.textContent = 'Показано ' + Math.min(limit, hit.length) + ' из ' + hit.length;
    var hasMore = hit.length > PAGE;
    moreWrap.style.display = hasMore ? '' : 'none';
    topShowAllBtn.style.display = hasMore ? '' : 'none';
    var btnTxt = showAll ? 'Свернуть до ' + PAGE : 'Показать все (' + hit.length + ')';
    moreBtn.textContent = btnTxt;
    topShowAllBtn.textContent = btnTxt;
    topShowAllBtn.classList.toggle('active', showAll);
  }}

  sortHeaders.forEach(function (th) {{
    th.addEventListener('click', function () {{
      var key = th.getAttribute('data-sort');
      if (curSort === key) {{
        curDir = -curDir;
      }} else {{
        curSort = key;
        curDir = (key === 'name' || key === 'dev') ? 1 : -1;
      }}
      sortHeaders.forEach(function (h) {{
        var match = h.getAttribute('data-sort') === curSort;
        h.classList.toggle('sorted', match);
        if (match) h.setAttribute('data-dir', curDir === -1 ? '↓' : '↑');
      }});
      apply();
    }});
  }});

  filterBtns.forEach(function (btn) {{
    btn.addEventListener('click', function () {{
      curFilter = btn.getAttribute('data-filter');
      filterBtns.forEach(function (b) {{ b.classList.toggle('active', b === btn); }});
      apply();
    }});
  }});

  phModeBtns.forEach(function (btn) {{
    btn.addEventListener('click', function () {{
      curPhMode = parseInt(btn.getAttribute('data-phmode'), 10);
      phModeBtns.forEach(function (b) {{
        b.classList.toggle('active', b === btn);
        b.classList.toggle('acc', b === btn);
      }});
      updatePhaseCells();
      if (curSort.indexOf('ph') === 0) apply();
    }});
  }});

  hideDupsChk.addEventListener('change', apply);
  searchInput.addEventListener('input', apply);
  searchInput.addEventListener('keydown', function (e) {{
    if (e.key === 'Escape') {{ searchInput.value = ''; apply(); }}
  }});
  document.addEventListener('keydown', function (e) {{
    if (e.key === '/' && document.activeElement !== searchInput) {{
      e.preventDefault();
      searchInput.focus();
    }}
  }});

  function toggleShowAll() {{ showAll = !showAll; apply(); }}
  moreBtn.addEventListener('click', toggleShowAll);
  topShowAllBtn.addEventListener('click', toggleShowAll);

  rows.forEach(function (tr) {{
    var id = tr.getAttribute('data-id');
    var cmpBtn = tr.querySelector('.cbtn');
    cmpBtn.addEventListener('click', function (e) {{
      e.stopPropagation();
      toggleCompare(id);
    }});
    tr.addEventListener('click', function () {{
      openIds[id] = !openIds[id];
      apply();
    }});
  }});

  Array.prototype.forEach.call(document.querySelectorAll('[data-jump]'), function (node) {{
    node.addEventListener('click', function () {{
      var id = node.getAttribute('data-jump');
      var c = rowCache[id];
      if (!c) return;
      curFilter = 'all';
      filterBtns.forEach(function (b) {{
        b.classList.toggle('active', b.getAttribute('data-filter') === 'all');
      }});
      if (c.isDup) hideDupsChk.checked = false;
      searchInput.value = '';
      showAll = true;
      openIds[id] = true;
      apply();
      c.tr.scrollIntoView({{ behavior: 'smooth', block: 'center' }});
    }});
  }});

  var cmpTray = document.getElementById('cmpTray');
  var cmpGrid = document.getElementById('cmpGrid');
  var cmpCount = document.getElementById('cmpCount');
  var quickCmpBtn = document.getElementById('quickCmpBtn');

  function toggleCompare(id) {{
    var idx = cmpIds.indexOf(id);
    if (idx >= 0) {{
      cmpIds.splice(idx, 1);
    }} else {{
      if (cmpIds.length >= CMP_MAX) cmpIds.shift();
      cmpIds.push(id);
    }}
    renderCompare();
  }}

  function cmpRow(label, value, best) {{
    var d = el('div');
    d.appendChild(el('span', '', label));
    d.appendChild(el('b', best ? 'best' : '', value));
    return d;
  }}

  function renderCompare() {{
    rows.forEach(function (tr) {{
      var btn = tr.querySelector('.cbtn');
      var on = cmpIds.indexOf(tr.getAttribute('data-id')) >= 0;
      btn.classList.toggle('on', on);
      btn.textContent = on ? '✓' : '+';
    }});
    cmpCount.textContent = String(cmpIds.length);
    quickCmpBtn.classList.toggle('active', cmpIds.length > 0);
    if (!cmpIds.length) {{
      cmpTray.classList.remove('on');
      cmpGrid.textContent = '';
      return;
    }}
    cmpTray.classList.add('on');
    var sel = cmpIds.map(function (id) {{ return rowCache[id]; }}).filter(Boolean);
    var best = {{}};
    ['med', 'st'].forEach(function (k) {{
      best[k] = Math.max.apply(null, sel.map(function (c) {{ return c[k]; }}));
    }});
    [0, 3, 6, 9].forEach(function (i) {{
      best['p' + i] = Math.max.apply(null, sel.map(function (c) {{ return c.p[i]; }}));
    }});
    cmpGrid.textContent = '';
    sel.forEach(function (c) {{
      var item = el('div', 'cmp-item');
      var rm = el('button', 'cmp-rm', '×');
      rm.type = 'button';
      rm.title = 'Убрать';
      rm.setAttribute('data-rm', c.id);
      item.appendChild(rm);
      var nm = el('div', 'cmp-iname', '#' + c.rk + ' ' + c.title);
      nm.title = c.title;
      item.appendChild(nm);
      var box = el('div', 'cmp-rows');
      box.appendChild(cmpRow('Медиана', c.med.toFixed(1) + ' тик/с', c.med === best.med));
      box.appendChild(cmpRow('Стабильность', c.st.toFixed(1) + '%', c.st === best.st));
      [['Лёгкая', 0], ['Частичная', 3], ['Тяжёлая', 6], ['Отклик', 9]].forEach(function (pair) {{
        var i = pair[1];
        box.appendChild(cmpRow(
          pair[0] + ' (Мед / P1)',
          c.p[i].toFixed(1) + ' / ' + c.p[i + 1].toFixed(0),
          c.p[i] === best['p' + i]
        ));
      }});
      item.appendChild(box);
      cmpGrid.appendChild(item);
    }});
    Array.prototype.forEach.call(cmpGrid.querySelectorAll('[data-rm]'), function (btn) {{
      btn.addEventListener('click', function () {{
        toggleCompare(btn.getAttribute('data-rm'));
      }});
    }});
  }}

  // Топ-3 ориентира: опорная, лидер по медиане, лучший по P1 — они же
  // помечены ключевыми при сборке строк.
  function setPresetTop3() {{
    cmpIds = rows
      .filter(function (tr) {{
        var c = rowCache[tr.getAttribute('data-id')];
        return c.isKey && !c.isRej;
      }})
      .slice(0, CMP_MAX)
      .map(function (tr) {{ return tr.getAttribute('data-id'); }});
    renderCompare();
  }}

  quickCmpBtn.addEventListener('click', function () {{
    if (cmpIds.length) {{
      cmpIds = [];
      renderCompare();
    }} else {{
      setPresetTop3();
    }}
  }});
  document.getElementById('cmpPresetBtn').addEventListener('click', setPresetTop3);
  document.getElementById('cmpClearBtn').addEventListener('click', function () {{
    cmpIds = [];
    renderCompare();
  }});

  var toTop = document.querySelector('.to-top');
  window.addEventListener('scroll', function () {{
    toTop.classList.toggle('on', window.scrollY > 500);
  }}, {{ passive: true }});
  toTop.addEventListener('click', function () {{ window.scrollTo({{ top: 0, behavior: 'smooth' }}); }});

  apply();
}})();
"##,
        meta = meta.join(","),
        refphase = ref_phase.join(","),
        all = s.schemes.len(),
        uniq = uniq,
        page = PAGE,
        cmpmax = CMP_MAX,
        dup = F_DUP,
        key = F_KEY,
        prob = F_PROB,
        rej = F_REJ,
        labels = json_escape_all(&PHASE_LABELS_LONG),
    )
}

fn css() -> &'static str {
    CSS
}

const CSS: &str = include_str!("report_compact.css");

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{IdentityJson, RecommendationJson};

    fn agg(median: f64, p1: f64) -> powerbench_metrics::AggregateResult {
        powerbench_metrics::AggregateResult {
            runs: 4,
            mean_average_throughput: median * 0.54,
            sample_std: 1.0,
            t_value: 2.0,
            margin: 2.0,
            ci_95: [median * 0.99, median * 1.01],
            run_variation_percent: 0.2,
            cv_warning: false,
            median_throughput: median,
            median_p1_throughput: p1,
            median_p01_throughput: p1 * 0.7,
            median_p95_execution_time_ms: 1.0,
            median_p99_execution_time_ms: 1.0,
            median_consistency_percent: 95.4,
            median_burst_retention_percent: 0.0,
            median_jitter_p99_ms: 0.0,
            median_worst_window_throughput: 180.0,
            median_background_purity: Some(98.0),
            run_duration_ms: 60_000,
            started_at_min_ns: 1_700_000_000_000_000_000,
        }
    }

    /// Четыре фазы; `drop` — процент падения в тяжёлой.
    fn phases(median: f64, drop: f64) -> Vec<PhaseSummaryJson> {
        const NAMES: [&str; 4] = ["Лёгкая", "Частичная", "Тяжёлая", "Отклик"];
        (0..4)
            .map(|i| PhaseSummaryJson {
                name: NAMES[i as usize].to_string(),
                median_throughput: median * [1.05, 0.29, 0.26, 0.51][i as usize],
                p1_throughput: median * 0.2,
                consistency_percent: 95.0,
                frequency_drop_percent: if i == 2 { drop } else { 0.0 },
                frequency_mhz: if i == 2 && drop > 0.0 { 4400.0 } else { 5201.0 },
                seconds: 10,
                samples_used: 3000,
                samples_raw: 3157,
                excluded_fraction: 0.05,
            })
            .collect()
    }

    fn scheme(id: &str, name: &str, median: f64, p1: f64) -> SchemeJson {
        let mut s =
            SchemeJson::from_aggregate(id.into(), false, None, &agg(median, p1), Vec::new());
        s.name = Some(name.to_string());
        s.phases = phases(median, 0.0);
        s
    }

    fn session(schemes: Vec<SchemeJson>) -> SessionJson {
        SessionJson {
            plan_guid: "plan-x".into(),
            original_scheme_guid: Some("aaa".into()),
            original_restored: true,
            identity: IdentityJson {
                workload_version: "GamingCpuV1".into(),
                config_hash: "H1".into(),
                seed_hex: "SEED0080".into(),
                worker_count: 4,
                logical_cpus: 6,
                timer_hz: 10_000_000,
                cpu_identifier: "cpu".into(),
                diagnostics_version: "0.1.0".into(),
                os_build: "26200.1".into(),
                memory_gib: 32.0,
                cpu_brand: "AMD Ryzen 5 7500F".into(),
                affinity_mode: "p-only".into(),
                affinity_signature: "p-only:test".into(),
            },
            schemes,
            recommendation: RecommendationJson {
                level: "Confirmed".into(),
                level_label: "Подтверждено".into(),
                recommended_scheme: Some("aaa".into()),
                runner_up_scheme: None,
                reason: "схема «AAA» лидирует".into(),
                probabilities: None,
                expected_margin_percent: Some(2.5),
                bootstrap_mode: None,
                tie_criterion: None,
            },
            warnings: Vec::new(),
            rounds_planned: 4,
            rounds_completed: 4,
            early_stop_reason: None,
            score_weights: [50.0, 30.0, 20.0],
            reference: None,
            screening: false,
        }
    }

    fn demo() -> SessionJson {
        let mut bad = scheme("ccc", "IIIEXOIII LOW LATENCY", 452.0, 65.0);
        bad.rejected = true;
        bad.rejection_reason = Some("проваливает такты".into());
        let mut freq = scheme("ddd", "Высокая производительность", 1040.0, 240.0);
        freq.phases = phases(1040.0, 6.0);
        session(vec![
            scheme("aaa", "Velo's Power Plan", 1050.0, 246.0),
            scheme("bbb", "Gio Intel", 1047.0, 250.0),
            freq,
            bad,
        ])
    }

    /// Отчёт открывается офлайн: без внешних таблиц стилей и скриптов.
    #[test]
    fn report_is_self_contained() {
        let html = build(&demo());
        assert!(!html.contains("<link "), "внешняя таблица стилей");
        assert!(!html.contains("src=\"http"), "внешний скрипт");
        assert!(html.contains("<style>"), "нет встроенных стилей");
        assert!(html.contains("<script>"), "нет встроенного скрипта");
        assert!(html.ends_with("</body></html>"));
    }

    /// Зелёного в отчёте нет: «хорошее» — белый акцент Graphite.
    ///
    /// Проверяем именно токен, а не «похожие цвета»: в приложении `--ok` зелёный
    /// и используется для успеха, поэтому его попадание в отчёт означало бы
    /// чужую палитру.
    #[test]
    fn positive_colour_is_white_not_green() {
        let html = build(&demo());
        assert!(!html.contains("#6fd0a0"), "в отчёте остался зелёный --ok");
        assert!(
            !html.contains("--ok:"),
            "в отчёте появился зелёный токен --ok"
        );
        assert!(
            html.contains("--accent:#FDFDFD"),
            "нет белого акцента приложения"
        );
        assert!(
            html.contains("--pos:var(--accent)"),
            "«хорошее» должно быть белым акцентом, а не отдельным цветом"
        );
    }

    /// Вердикт, три ориентира и одна таблица — первый экран целиком.
    #[test]
    fn first_screen_has_verdict_podium_and_one_table() {
        let html = build(&demo());
        assert!(html.contains("ВЕРДИКТ БЕНЧМАРКА"));
        assert!(html.contains("Уверенность: <b>Подтверждено</b>"));
        assert!(html.contains("Перевес: <b>2.5%</b>"));
        assert_eq!(
            html.matches("class=\"pcard\"").count(),
            3,
            "ориентиров должно быть ровно три"
        );
        assert_eq!(
            html.matches("<table class=\"main-tbl\"").count(),
            1,
            "таблица схем должна быть одна"
        );
        assert_eq!(
            html.matches("<tr class=\"sr").count(),
            4,
            "в таблице по строке на схему"
        );
    }

    /// Строки отсортированы по медиане, ранг проставлен.
    #[test]
    fn table_is_ranked_by_median() {
        let html = build(&demo());
        let body = html
            .split("<tbody id=\"schBody\">")
            .nth(1)
            .expect("нет тела таблицы");
        let rows: Vec<&str> = body.split("<tr class=\"sr").skip(1).collect();
        assert_eq!(rows.len(), 4);
        assert!(
            rows[0].contains("Velo&#x27;s Power Plan"),
            "первым не лидер"
        );
        assert!(
            rows[3].contains("IIIEXOIII LOW LATENCY"),
            "последним не худший"
        );
        assert!(rows[0].contains("<td>1</td>"), "нет ранга 1");
        assert!(rows[3].contains("<td>4</td>"), "нет ранга 4");
    }

    /// Забракованная схема помечена и приглушена, причина видна в карточке.
    #[test]
    fn rejected_scheme_is_marked_and_explained() {
        let html = build(&demo());
        assert!(html.contains("БРАК"), "нет метки «БРАК»");
        assert!(html.contains("<span class=\"tag err\">БРАК</span>"));
        assert!(html.contains("sr out"), "бракованная строка не приглушена");
        assert!(
            html.contains("проваливает такты"),
            "причина брака не попала в карточку"
        );
    }

    /// Снижение частоты видно и в строке, и в карточке диагностики.
    #[test]
    fn frequency_drop_is_visible_in_row_and_diagnostics() {
        let html = build(&demo());
        assert!(
            html.contains("↓ ЧАСТОТА"),
            "в строке схемы нет метки о снижении частоты"
        );
        assert!(
            html.contains("Снижение частоты в фазах"),
            "в диагностике нет раздела о снижении частоты"
        );
        assert!(
            !html.to_lowercase().contains("троттлинг"),
            "в отчёте снова появилось слово «троттлинг»"
        );
    }

    /// Названия схем экранируются: это пользовательские строки из Windows.
    #[test]
    fn scheme_names_are_escaped() {
        let s = session(vec![scheme(
            "aaa",
            "<script>alert(1)</script>",
            1000.0,
            200.0,
        )]);
        let html = build(&s);
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "название схемы попало в разметку как есть"
        );
        assert!(html.contains("&lt;script&gt;"));
    }

    /// Регресс H46: JSON внутри `<script>` не должен вырываться из строки.
    ///
    /// HTML-парсер заканчивает `<script>` на первом же `</script` — независимо
    /// от того, находится ли он внутри строки JSON или нет. `serde_json`
    /// экранирует только кавычки и обратный слэш, поэтому `</script>` в имени
    /// схемы (а имя приходит из `powercfg /list`) вырывался наружу и превращался
    /// в исполняемый HTML.
    #[test]
    fn a_script_closing_tag_cannot_escape_the_embedded_json() {
        let injection = "</script><img src=x onerror=alert(1)>";
        let mut s = session(vec![scheme("aaa", injection, 1000.0, 200.0)]);
        // Идентификатор и причина брака попадают во встраиваемый JSON как ключ и
        // значение — именно они там и проверяются.
        s.schemes[0].rejected = true;
        s.schemes[0].rejection_reason = Some(injection.to_string());
        let html = build(&s);
        // Всё, что лежит внутри единственного блока скрипта, обязано быть
        // экранировано: «сырой» `</script>` внутри него означал бы, что JSON
        // вырвался и разметка развалилась.
        let script_start = html.find("<script>").expect("в отчёте нет блока скрипта");
        let script_end = html[script_start..]
            .find("</script>")
            .expect("блок скрипта не закрыт");
        let body = &html[script_start + "<script>".len()..script_start + script_end];
        assert!(
            !body.contains("</script"),
            "внутри скрипта снова появился закрывающий тег: JSON вырвался наружу"
        );
        assert!(
            body.contains("\\u003c/script"),
            "значение обязано быть экранировано через \\u003c: {}",
            body.chars().take(400).collect::<String>()
        );
        assert!(
            !html.contains("<img src=x onerror=alert(1)>"),
            "инъекция попала в разметку как HTML"
        );
    }

    /// Разделители строк JavaScript ломают парсер скрипта даже внутри строки.
    #[test]
    fn json_escape_also_neutralizes_javascript_line_separators() {
        let escaped = json_escape("a\u{2028}b\u{2029}c");
        assert!(!escaped.contains('\u{2028}'), "{escaped}");
        assert!(!escaped.contains('\u{2029}'), "{escaped}");
        assert!(escaped.contains("\\u2028"), "{escaped}");
        assert!(escaped.contains("\\u2029"), "{escaped}");
        // И результат остаётся валидным JSON с тем же значением.
        let back: String = serde_json::from_str(&escaped).expect("экранированный JSON не читается");
        assert_eq!(back, "a\u{2028}b\u{2029}c");
    }

    /// Экранирование не должно ломать обычные строки: значение обязано
    /// переживать ту же остановку JSON, что и раньше.
    #[test]
    fn json_escape_keeps_ordinary_values_readable() {
        for value in ["", "План Обычный", "План \"с кавычками\"", "a\\b", "≈5 %"]
        {
            let escaped = json_escape(value);
            let back: String =
                serde_json::from_str(&escaped).expect("экранированный JSON не читается");
            assert_eq!(back, value, "значение исказилось при экранировании");
        }
    }

    /// Регресс H43: накопленное изменение дрейфа считается по интервалам.
    ///
    /// `n` замеров дают `n − 1` интервалов, и ровно на `n − 1` умножается тренд
    /// в `ReferenceJson::total_change_percent` — то есть в решении о стабильности
    /// дрейфа. Прежнее умножение на `n` показывало в карточке число, отличающееся
    /// от принятого решения на целый процент тренда.
    #[test]
    fn drift_card_accumulates_over_intervals_not_samples() {
        let mut s = session(vec![scheme("aaa", "План Опорный", 1000.0, 900.0)]);
        let reference = |trend: f64| crate::result::ReferenceSummary {
            scheme_id: "aaa".into(),
            scheme_name: Some("План Опорный".into()),
            per_round: vec![1000.0, 1001.0, 1002.0],
            trend_percent_per_round: trend,
            span_percent: 0.2,
            span_limit_percent: 3.0,
            unstable: false,
        };
        // Пока опорной схемы в сессии нет — карточка объясняет это.
        let html = drift_card(&s);
        assert!(html.contains("не задана"), "{html}");
        // 3 замера → 2 интервала → 2.0 %, а не 3.0 %.
        s.reference = Some(reference(1.0));
        let card = drift_card(&s);
        assert!(
            card.contains("+2.0%"),
            "накопленное изменение считается не по интервалам: {card}"
        );
        assert!(
            !card.contains("+3.0%"),
            "вернулось старое умножение на n: {card}"
        );
        // Карточка обязана совпадать с решением по стабильности дрейфа.
        let r = reference(2.0);
        let expected = r.trend_percent_per_round * (r.per_round.len() - 1) as f64;
        s.reference = Some(r);
        assert!(
            drift_card(&s).contains(&format!("+{expected:.1}%")),
            "карточка разошлась с решением по дрейфу"
        );
    }

    /// Регресс H48: опорная схема ищется регистронезависимо.
    ///
    /// `powercfg` отдаёт GUID в верхнем регистре, а `reference.scheme_id` мог
    /// прийти из чекпоинта в нижнем. При точном `==` карточка дрейфа молча
    /// сообщала «опорная схема не задана».
    #[test]
    fn reference_scheme_is_found_regardless_of_case() {
        let upper = session(vec![scheme(
            "381B4222-F694-41F0-9685-FF5BB260DF2E",
            "План Опорный",
            1000.0,
            900.0,
        )]);
        let found = reference_scheme(&upper, Some("381B4222-F694-41F0-9685-FF5BB260DF2E"))
            .expect("опорная схема не найдена в своём же регистре");
        assert!(
            found
                .scheme_id
                .eq_ignore_ascii_case("381b4222-f694-41f0-9685-ff5bb260df2e")
        );

        // Идентификатор опорной схемы в другом регистре тоже находится.
        let mut s = upper.clone();
        s.reference = Some(crate::result::ReferenceSummary {
            scheme_id: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            scheme_name: Some("План Опорный".into()),
            per_round: vec![1000.0, 1001.0],
            trend_percent_per_round: 0.1,
            span_percent: 0.1,
            span_limit_percent: 3.0,
            unstable: false,
        });
        assert!(
            reference_scheme(&s, None).is_some(),
            "опорная схема потеряна из-за регистра GUID"
        );
        // И через активную схему в другом регистре.
        let lower = session(vec![scheme(
            "381b4222-f694-41f0-9685-ff5bb260df2e",
            "План Опорный",
            1000.0,
            900.0,
        )]);
        assert!(
            reference_scheme(&lower, Some("381B4222-F694-41F0-9685-FF5BB260DF2E")).is_some(),
            "активная схема не найдена из-за регистра GUID"
        );
    }

    /// Фазы в строке: три метрики на каждую из четырёх фаз.
    #[test]
    fn each_row_carries_all_four_phases() {
        let html = build(&demo());
        let first = html
            .split("data-p=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .expect("нет data-p");
        assert_eq!(
            first.split(',').count(),
            12,
            "ожидались 4 фазы × 3 метрики, получено {}",
            first.split(',').count()
        );
    }

    /// Шапка содержит четыре колонки фаз плюс служебные.
    #[test]
    fn header_lists_the_four_phases() {
        let html = build(&demo());
        let head = html.split("</thead>").next().expect("нет шапки таблицы");
        for l in PHASE_LABELS {
            assert!(head.contains(l), "в шапке нет фазы «{l}»");
        }
        assert_eq!(
            head.matches("data-sort=\"ph").count(),
            4,
            "в шапке должно быть четыре сортируемых колонки фаз"
        );
    }

    /// Кнопка «Показать все» переключает состояние, а не читает `checked`.
    ///
    /// У `<button>` нет свойства `checked`: обращение к нему даёт `undefined`,
    /// кнопка не срабатывала, а схемы после двадцати пятой оставались скрыты.
    #[test]
    fn show_all_button_toggles_state() {
        let html = build(&demo());
        assert!(
            !html.contains("moreBtn.checked"),
            "кнопка снова читает checked у кнопки"
        );
        assert!(html.contains("showAll = !showAll"), "нет переключателя");
        assert!(html.contains("id=\"moreBtn\""), "нет самой кнопки");
    }

    /// Подробная строка строится через DOM, а не склейкой строк.
    ///
    /// Названия схем приезжают из Windows и в отчёте повторяются в лотке
    /// сравнения; html-запись дала бы возможность встроить разметку.
    #[test]
    fn detail_row_is_built_without_html_injection() {
        let html = build(&demo());
        assert!(
            html.contains("function el(tag, cls, text)"),
            "нет помощника построения через textContent"
        );
        assert!(
            !html.contains("innerHTML = '<td"),
            "подробная строка собирается склейкой строк"
        );
    }

    /// Шапка: машина, нагрузка и время.
    #[test]
    fn topbar_names_the_machine_and_workload() {
        let html = build(&demo());
        assert!(html.contains("AMD Ryzen 5 7500F"), "нет названия CPU");
        assert!(html.contains("6 ядер"), "нет числа ядер");
        assert!(html.contains("32 ГБ"), "нет объёма памяти");
        assert!(html.contains("GamingCpuV1"), " нет версии нагрузки");
        assert!(html.contains("Воркеры: <b>4/6</b>"), "нет числа воркеров");
    }

    /// Каждый `var(--токен)` в стилях объявлен.
    ///
    /// Необъявленный пользовательский токен не выдаёт ошибку: правило целиком
    /// уходит в `initial`. Так `background: … var(--bg)` у `body` молча
    /// становился прозрачным, и на странице просвечивал белый фон браузера —
    /// весь тёмный отчёт выглядел белым листом, а тесты были зелёные, потому
    /// что HTML собирался правильно. Единственный способ это поймать —
    /// проверить сами объявления.
    #[test]
    fn every_css_token_is_declared() {
        // Объявление — это `--имя` сразу за двоеточием, использование — то же
        // имя сразу за скобкой. Разбор идёт по `char_indices`: в стилях есть
        // кириллица в комментариях, и байтовый проход спотыкается о границы.
        let mut declared: Vec<&str> = Vec::new();
        let mut used: Vec<&str> = Vec::new();
        for (i, ch) in CSS.char_indices() {
            if ch != '-' || !CSS[i..].starts_with("--") {
                continue;
            }
            let rest = &CSS[i + 2..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .unwrap_or(rest.len());
            if end == 0 {
                continue;
            }
            let name = &CSS[i..i + 2 + end];
            match rest[end..].chars().next() {
                // Объявление: `--имя:`.
                Some(':') => declared.push(name),
                // Использование: `var(--имя)`.
                Some(')') => used.push(name),
                _ => {}
            }
        }
        let missing: Vec<&str> = used
            .iter()
            .filter(|n| !declared.contains(n))
            .copied()
            .collect();
        assert!(
            missing.is_empty(),
            "в стилях используются необъявленные токены: {} — правила с ними \
             молча откатятся в initial (прозрачный фон вместо тёмного)",
            missing.join(", ")
        );
        assert!(
            used.len() > 10 && declared.len() > 10,
            "разбор стилей ничего не нашёл ({} объявлений, {} использований): \
             тест проходит вхолостую",
            declared.len(),
            used.len()
        );
    }

    /// Фон страницы задаётся явно и он тёмный.
    #[test]
    fn page_background_is_dark_and_declared() {
        assert!(
            CSS.contains("background:radial-gradient"),
            "у body нет фоновой заливки: страница откроется белой"
        );
        assert!(
            CSS.contains("--bg:var(--bg-0)"),
            "фон страницы не связан с тёмным токеном Graphite"
        );
    }

    /// Пустая сессия не должна ломать разметку.
    ///
    /// Такого результата в жизни не бывает, но отчёт об отсутствующем замере
    /// всё равно открывается: иначе страница вкладки падала бы целиком.
    #[test]
    fn empty_session_still_produces_valid_document() {
        let html = build(&session(Vec::new()));
        assert!(html.ends_with("</body></html>"));
        assert!(html.contains("class=\"topbar\""), "шапка отчёта пропала");
        assert!(
            html.contains("id=\"schBody\""),
            "каркас таблицы пропал — фильтры и сортировка не повесят скрипт"
        );
        assert!(
            html.contains("Сравнение недоступно"),
            "при пустой сессии должно быть сказано, что сравнивать нечего"
        );
    }
}
