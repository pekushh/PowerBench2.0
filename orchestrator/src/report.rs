//! Сводный HTML-отчёт по истории сессий: автономный документ с итогами,
//! графиком тенденций и детализацией по каждой сессии. Без внешних
//! зависимостей — все стили и SVG встроены.

use crate::history::{date_time_stamp, session_started_at_ns};
use crate::result::SessionJson;

const COLORS: &[&str] = &[
    "#0e7490", "#2563eb", "#d97706", "#7c3aed", "#059669", "#db2777", "#ca8a04", "#0ea5e9",
    "#dc2626", "#475569",
];

const HTML_CSS: &str = r##"<!DOCTYPE html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Отчёт PowerBench</title><style>
:root{--bg:#f3f5f9;--panel:#ffffff;--line:#e3e8ef;--fg:#1b2430;--dim:#4d5a68;--mute:#8794a5;
--ok:#0f7d4b;--okbg:#e8f6ee;--acc:#0e7490;--accbg:#e7f4f8;--warn:#a35d00;--warnbg:#fdf0e0;
--err:#b3261e;--errbg:#fbecec;--shadow:0 1px 2px rgba(24,32,48,.05),0 10px 30px -18px rgba(24,32,48,.22)}
*{box-sizing:border-box}body{margin:0;padding:32px 20px 48px;background:var(--bg);color:var(--fg);
font:14px/1.55 -apple-system,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;-webkit-font-smoothing:antialiased}
body>*{max-width:960px;margin-left:auto;margin-right:auto}
header{background:linear-gradient(180deg,#fff,#fbfcfe);border:1px solid var(--line);border-radius:16px;
padding:26px 30px;box-shadow:var(--shadow);position:relative;overflow:hidden}
header::before{content:"";position:absolute;left:0;top:0;bottom:0;width:4px;
background:linear-gradient(180deg,var(--acc),var(--ok))}
.brand{display:inline-block;font-size:11px;letter-spacing:2.2px;text-transform:uppercase;color:var(--acc);
font-weight:700;background:var(--accbg);padding:4px 11px;border-radius:999px}
h1{font-size:25px;margin:12px 0 4px;letter-spacing:-.3px;line-height:1.2}
h2{font-size:16px;margin:30px 0 12px;letter-spacing:-.2px;display:flex;align-items:center;gap:9px}
h2::before{content:"";width:4px;height:16px;border-radius:2px;background:var(--acc)}
header+h2{margin-top:22px}
.meta{color:var(--mute);font-size:12.5px;line-height:1.6;margin-bottom:2px}
.muted{color:var(--mute)}
.note{color:var(--mute);font-size:12.5px;margin-top:6px;line-height:1.5}
.note-stat{font-size:12.5px;color:var(--mute)}
.mono{font-family:Consolas,Menlo,monospace;font-size:12px;color:#566170;background:#f2f5f8;padding:1.5px 6px;border-radius:6px}
.sumcards{display:flex;flex-wrap:wrap;gap:12px}
.sumcard{flex:1 1 200px;min-width:170px;background:var(--panel);border:1px solid var(--line);
border-radius:14px;padding:16px 18px;box-shadow:var(--shadow)}
.sc-name{font-size:11.5px;color:var(--mute);font-weight:700;letter-spacing:.4px;text-transform:uppercase;
overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.sc-num{font-size:22px;font-weight:700;margin-top:5px;font-variant-numeric:tabular-nums;letter-spacing:-.3px}
.sc-num small{font-size:12.5px;color:var(--mute);font-weight:500;margin-left:4px}
.sc-sub{font-size:12px;color:var(--mute);margin-top:3px}
table{width:100%;border-collapse:separate;border-spacing:0;margin:4px 0 22px;font-size:13px;
background:var(--panel);border:1px solid var(--line);border-radius:12px;box-shadow:var(--shadow)}
th{color:#5c6774;font-weight:600;text-align:left;padding:10px 12px;background:#f8fafc;
border-bottom:1px solid var(--line);font-size:12px;white-space:nowrap}
td{padding:9px 12px;border-bottom:1px solid #eef1f5;vertical-align:top}
tbody tr:last-child td{border-bottom:0}
tbody tr:nth-child(even) td{background:#fafbfd}
td.num,th.num{text-align:right;font-variant-numeric:tabular-nums}
tr.detail td{background:#fbfcfe;padding:16px 20px;border-top:1px solid var(--line)}
.detail-body{margin:0 0 18px}
.dhead{color:#5c6774;font-weight:700;font-size:12px;margin:0 0 10px;text-transform:uppercase;letter-spacing:.5px}
table.sub{border-radius:10px;box-shadow:none;margin:10px 0 14px}
table.sub th{background:transparent;border-bottom:1px solid var(--line)}
.rec{margin-top:12px;padding:10px 14px;background:var(--accbg);border:1px solid #cde6ee;border-radius:10px;
font-size:13px;color:#0b4455}
.rec b{color:#0b6f8a}
.verdict{display:flex;flex-wrap:wrap;align-items:center;gap:8px 12px;background:var(--panel);
border:1px solid var(--line);border-left:3px solid var(--acc);border-radius:12px;padding:14px 18px;
box-shadow:var(--shadow);font-size:14px}
.badge{display:inline-block;padding:2.5px 9px;border-radius:999px;font-size:11.5px;font-weight:600;
border:1px solid transparent;white-space:nowrap}
.badge.ok{color:#0f7d4b;background:var(--okbg);border-color:#cfeadd}
.badge.acc{color:#0b6f8a;background:var(--accbg);border-color:#cde6ee}
.badge.warn{color:#a35d00;background:var(--warnbg);border-color:#f3d9bd}
.badge.err{color:#b3261e;background:var(--errbg);border-color:#f3cfcf}
.pbar{display:flex;align-items:center;gap:12px;margin:7px 0;font-size:12.5px;flex-wrap:wrap}
.plab{width:212px;min-width:160px;flex:0 1 auto;color:var(--dim)}
.ptrack{flex:1 1 80px;max-width:340px;height:7px;border-radius:999px;background:#eef1f6;overflow:hidden}
.ptrack i{display:block;height:100%;border-radius:999px;background:linear-gradient(90deg,var(--acc),#17a2b8)}
.pval{width:64px;text-align:right;color:var(--fg);font-variant-numeric:tabular-nums;font-weight:600}
.chart{background:var(--panel);border:1px solid var(--line);border-radius:14px;padding:14px 6px 6px 6px;
box-shadow:var(--shadow);margin-bottom:22px}
.chart svg{display:block}
.panel{fill:var(--panel)}
.grid{stroke:#e6eaf0;stroke-width:1}
.axis{stroke:#9aa4b2;stroke-width:1}
.ylab,.xlab{fill:#7c8696;font-size:11px;font-variant-numeric:tabular-nums}
.legend{fill:#5c6774;font-size:11px}
.foot{margin-top:34px;color:var(--mute);font-size:11.5px;text-align:center;padding-top:18px;border-top:1px solid var(--line)}
@media (max-width:640px){body{padding:18px 12px 40px}header{padding:20px}h1{font-size:21px}
.plab{width:auto;flex:1 1 100%}table{display:block;overflow-x:auto}.sumcards{flex-direction:column}}
</style></head><body>
"##;

/// Построить законченный HTML-документ отчёта по сессии.
pub fn build_report(sessions: &[SessionJson]) -> String {
    let now_stamp = date_time_stamp(now_unix_ns());
    let mut ordered: Vec<&SessionJson> = sessions.iter().collect();
    ordered.sort_by_key(|a| session_started_at_ns(a).unwrap_or(0));

    let header = render_header(&ordered);
    let chart = render_chart(&ordered);
    let sessions_html = render_sessions(&ordered);

    format!(
        "{HTML_CSS}{header}{chart}{sessions_html}<div class=\"foot\">Сгенерировано {now_stamp} (UTC)</div></body></html>"
    )
}

/// Компактный HTML-отчёт по одной сессии: вердикт, рекомендация и сравнение
/// схем — без «лишней» информации (без сырых прогонов и bootstrap-вероятностей).
pub fn build_session_report(s: &SessionJson) -> String {
    let start = session_started_at_ns(s).unwrap_or(0);
    let stamp = date_time_stamp(start);
    let rec = &s.recommendation;
    let id = &s.identity;
    let winner = rec
        .recommended_scheme
        .as_ref()
        .and_then(|g| s.schemes.iter().find(|x| x.scheme_id == *g));
    let winner_name = winner
        .map(|w| esc(&w.name.clone().unwrap_or_else(|| w.scheme_id.clone())))
        .unwrap_or_else(|| "—".to_string());
    let margin = rec
        .expected_margin_percent
        .map(|m| format!("{m:.2}%"))
        .unwrap_or_else(|| "—".to_string());
    let tie = matches!(rec.level.as_str(), "Equivalent" | "KeepCurrent");
    let has_winner = !tie && winner.is_some();
    let verdict_text = if has_winner {
        format!("рекомендована <b>{rec_name}</b>", rec_name = winner_name)
    } else {
        match rec.level.as_str() {
            "Equivalent" => "схемы эквивалентны — значимых различий не выявлено".to_string(),
            "KeepCurrent" => "оставить текущую схему".to_string(),
            _ => "данных недостаточно для рекомендации".to_string(),
        }
    };

    let mut sch_rows = String::new();
    for sch in &s.schemes {
        let status = if sch.rejected {
            format!(
                "<span class=\"badge err\">{0}</span>",
                esc(sch.rejection_reason.as_deref().unwrap_or("забракована"))
            )
        } else if has_winner
            && winner
                .map(|w| w.scheme_id == sch.scheme_id)
                .unwrap_or(false)
        {
            "<span class=\"badge ok\">рекомендована</span>".to_string()
        } else {
            "<span class=\"badge acc\">допущена</span>".to_string()
        };
        let name = esc(&sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone()));
        sch_rows.push_str(&format!(
            "<tr><td>{name}</td><td class=\"num\">{runs}</td><td class=\"num\">{median}</td>\
             <td class=\"num\">{cv}</td><td class=\"num\">{cons}</td><td class=\"num\">{worst}</td>\
             <td>{status}</td></tr>",
            name = name,
            runs = sch.runs,
            median = f1(sch.median_throughput),
            cv = pct1(sch.run_variation_percent),
            cons = pct1(sch.median_consistency_percent),
            worst = f1(sch.median_worst_window_throughput),
            status = status,
        ));
    }
    if sch_rows.is_empty() {
        sch_rows = "<tr><td colspan=\"7\">Нет данных по схемам.</td></tr>".to_string();
    }

    let day_before = date_slice(&stamp, 6, 8);
    let mut stamp2 = stamp.clone();
    stamp2.truncate(15);
    let header_day = format!("{0} {1}", esc(&day_before), esc(&stamp2[9..]));

    format!(
        "{HTML_CSS}<header><div class=\"brand\">PowerBench</div>\
         <h1>Отчёт по сессии</h1>\
         <div class=\"meta\">{day} · план <span class=\"mono\">{plan}</span></div>\
         <div class=\"meta\">нагрузка {wl} · воркеров {wc} / ядер {lcpus} · хэш {hash} · seed {seed}</div></header>\
         <h2>Вердикт</h2>\
         <div class=\"verdict\"><span class=\"badge {cls}\">{level_label}</span>\
         <span>{verdict}</span><span class=\"note-stat\">перевес {margin}</span></div>\
         <div class=\"note\">{reason}</div>\
         <h2>Схемы</h2>\
         <table><thead><tr><th>Схема</th><th class=\"num\">Прогоны</th>\
         <th class=\"num\">Медиана, тик/с</th><th class=\"num\">CV</th>\
         <th class=\"num\">Стабильность</th><th class=\"num\">Худш. секунда</th><th>Статус</th></tr></thead>\
         <tbody>{rows}</tbody></table>\
         {score}\
         <div class=\"foot\">Сгенерировано {now_stamp} (UTC)</div></body></html>",
        day = header_day,
        plan = esc(&s.plan_guid),
        wl = esc(&id.workload_version),
        wc = id.worker_count,
        lcpus = id.logical_cpus,
        hash = esc(&id.config_hash),
        seed = esc(&id.seed_hex),
        cls = lvl_class(&rec.level),
        level_label = esc(&rec.level_label),
        verdict = verdict_text,
        margin = margin,
        reason = esc(&rec.reason),
        rows = sch_rows,
        score = score_section(s),
        now_stamp = esc(&date_time_stamp(now_unix_ns())),
    )
}

/// Заголовок отчёта и сводка по всем сессиям.
fn render_header(sessions: &[&SessionJson]) -> String {
    let count = sessions.len();
    let (first, last) = periods(sessions);
    let identity = sessions.first().map(|s| {
        let i = &s.identity;
        format!(
            " · {} · нагрузка {} · хэш {} · seed {} · воркеров {} / ядер ЦП {}",
            esc(&i.cpu_identifier),
            esc(&i.workload_version),
            esc(&i.config_hash),
            esc(&i.seed_hex),
            i.worker_count,
            i.logical_cpus,
        )
    });

    let mut by_scheme: Vec<(&str, usize, f64)> = Vec::new();
    for s in sessions {
        for sch in s.schemes.iter().filter(|x| !x.rejected) {
            if !(sch.median_throughput.is_finite() && sch.median_throughput > 0.0) {
                continue;
            }
            match by_scheme.iter_mut().find(|(id, _, _)| *id == sch.scheme_id) {
                Some((_, n, sum)) => {
                    *n += 1;
                    *sum += sch.median_throughput;
                }
                None => by_scheme.push((sch.scheme_id.as_str(), 1, sch.median_throughput)),
            }
        }
    }
    let sum_html = if by_scheme.is_empty() {
        "<div class=\"note\">Нет данных по схемам — завершите первую сессию.</div>".to_string()
    } else {
        let cards: String = by_scheme
            .iter()
            .map(|(id, n, sum)| {
                let avg = *sum / *n as f64;
                format!(
                    "<div class=\"sumcard\"><div class=\"sc-name\">{id}</div>\
                     <div class=\"sc-num\">{avg} <small>тик/с</small></div>\
                     <div class=\"sc-sub\">участвовала в {n} сессиях</div></div>",
                    id = esc(id),
                    avg = f1(avg),
                    n = n,
                )
            })
            .collect();
        format!("<div class=\"sumcards\">{cards}</div>")
    };

    let identity_line = identity.unwrap_or_default();
    let empty_note = if count == 0 {
        "<div class=\"note\">История пуста — отчёт не содержит сессий.</div>".to_string()
    } else {
        String::new()
    };
    format!(
        "<header><div class=\"brand\">PowerBench</div><h1>Отчёт по истории измерений</h1>\
         <div class=\"meta\">{count} сессий · период {first} — {last}{identity_line}</div>\
         {empty_note}</header>\
         <h2>Сводка</h2>{sum_html}"
    )
}

/// График тенденций медианного throughput по сессиям (SVG, встроенный).
fn render_chart(sessions: &[&SessionJson]) -> String {
    if sessions.len() < 2 {
        return String::new();
    }
    let mut series: Vec<(String, Vec<(usize, f64)>)> = Vec::new();
    let mut all: Vec<f64> = Vec::new();
    for (col, s) in sessions.iter().enumerate() {
        for sch in s.schemes.iter().filter(|x| !x.rejected) {
            if !(sch.median_throughput.is_finite() && sch.median_throughput > 0.0) {
                continue;
            }
            match series.iter_mut().find(|(id, _)| *id == sch.scheme_id) {
                Some((_, pts)) => pts.push((col, sch.median_throughput)),
                None => {
                    let name = sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone());
                    series.push((name, vec![(col, sch.median_throughput)]));
                }
            }
            all.push(sch.median_throughput);
        }
    }
    if series.is_empty() {
        return String::new();
    }
    all.sort_by(|a, b| a.total_cmp(b));
    let mut y_max = all[all.len() - 1];
    let mut y_min = all[0];
    if y_max - y_min < 1e-9 {
        y_min -= 1.0;
        y_max += 1.0;
    }
    let pad_y = (y_max - y_min) * 0.07;
    y_min -= pad_y;
    y_max += pad_y;

    const W: f64 = 900.0;
    const H: f64 = 280.0;
    const L: f64 = 84.0;
    const R: f64 = 18.0;
    const T: f64 = 16.0;
    const B: f64 = 36.0;
    let n = sessions.len();
    let inner_h = H - T - B;
    let step = nice_step((y_max - y_min) / 5.0);
    let start = (y_min / step).ceil() * step;
    let y = |v: f64| T + inner_h - ((v - y_min) / (y_max - y_min)) * inner_h;
    let x = |col: usize| L + (col as f64 + 0.5) * ((W - L - R) / n as f64);

    let mut gridlines = String::new();
    let mut v = start;
    while v <= y_max + 1e-9 {
        gridlines.push_str(&format!(
            "<line class=\"grid\" x1=\"{0:.0}\" y1=\"{1:.1}\" x2=\"{2:.1}\" y2=\"{3:.1}\"/>",
            L,
            y(v),
            W - R,
            y(v)
        ));
        gridlines.push_str(&format!(
            "<text class=\"ylab\" x=\"{0:.0}\" y=\"{1:.1}\" text-anchor=\"end\">{2:.0}</text>",
            L - 8.0,
            y(v) + 3.0,
            v
        ));
        v += step;
    }
    gridlines.push_str(&format!(
        "<line class=\"axis\" x1=\"{0:.1}\" y1=\"{1:.1}\" x2=\"{2:.1}\" y2=\"{3:.1}\"/>",
        L,
        T + inner_h,
        W - R,
        T + inner_h
    ));

    let label_every = ((n as f64) / 12.0).ceil().max(1.0) as usize;
    let mut xlabels = String::new();
    for (col, s) in sessions.iter().enumerate() {
        if col % label_every != 0 {
            continue;
        }
        let day = short_date(s);
        xlabels.push_str(&format!(
            "<text class=\"xlab\" x=\"{0:.1}\" y=\"{1:.0}\" text-anchor=\"middle\">{2}</text>",
            x(col),
            H - 12.0,
            esc(&day)
        ));
    }

    let mut legend = String::new();
    for (i, (name, _)) in series.iter().enumerate() {
        let cy = (i % 2) as f64 * 320.0;
        let top = 2.0 + ((i / 2) as f64) * 14.0;
        let color = COLORS[i % COLORS.len()];
        legend.push_str(&format!(
            "<circle cx=\"{0:.0}\" cy=\"{1:.0}\" r=\"3\" fill=\"{2}\"/><text class=\"legend\" x=\"{3:.0}\" y=\"{4:.0}\">{5}</text>",
            cy + 4.0, top, color, cy + 11.0, top + 3.5, esc(name)
        ));
    }

    let mut shapes = String::new();
    for (i, (_name, pts)) in series.iter().enumerate() {
        let color = COLORS[i % COLORS.len()];
        let mut d = String::new();
        for (k, (col, score)) in pts.iter().enumerate() {
            let cmd = if k == 0 { "M" } else { "L" };
            d.push_str(&format!("{cmd}{0:.1} {1:.1} ", x(*col), y(*score)));
        }
        if pts.len() > 1 {
            shapes.push_str(&format!(
                "<path class=\"line\" d=\"{} \" fill=\"none\" stroke=\"{1}\" stroke-width=\"1.8\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>",
                d.trim_end(),
                color
            ));
        }
        // Легенда с цветом серии выводится отдельно; точки — с подписью.
        for (col, score) in pts {
            let label = esc(&short_date(sessions[*col]));
            shapes.push_str(&format!(
                "<circle cx=\"{0:.1}\" cy=\"{1:.1}\" r=\"3.2\" fill=\"{2}\" stroke=\"#ffffff\" stroke-width=\"1\"><title>{3}</title></circle>",
                x(*col),
                y(*score),
                color,
                label
            ));
        }
    }

    format!(
        "<h2>Тенденции медианной производительности (тик/с)</h2><div class=\"chart\">\
         <svg viewBox=\"0 0 {W} {H}\" width=\"100%\" height=\"{H}\" xmlns=\"http://www.w3.org/2000/svg\">\
         <rect class=\"panel\" width=\"{W}\" height=\"{H}\" rx=\"8\"/>{legend}{gridlines}{xlabels}{shapes}</svg></div>"
    )
}

/// Таблица сессий + детали по каждой (схемы, рекомендация, вероятности).
fn render_sessions(sessions: &[&SessionJson]) -> String {
    if sessions.is_empty() {
        return String::new();
    }
    let mut rows = String::new();
    for s in sessions {
        let start = session_started_at_ns(s).unwrap_or(0);
        let stamp = date_time_stamp(start);
        let day = date_slice(&stamp, 6, 8);
        let time = &stamp[9..15];
        let best = best_scheme(s);
        let name = best
            .and_then(|b| b.name.clone().or(Some(b.scheme_id.clone())))
            .unwrap_or_default();
        let score = best
            .and_then(|b| {
                (b.median_throughput.is_finite() && b.median_throughput > 0.0)
                    .then_some(b.median_throughput)
            })
            .unwrap_or(f64::NAN);
        let stability = best
            .and_then(|b| {
                (b.median_consistency_percent.is_finite()).then_some(b.median_consistency_percent)
            })
            .unwrap_or(f64::NAN);
        let lvl = &s.recommendation;
        let detail = scheme_detail_block(s, &stamp);
        rows.push_str(&format!(
            "<tr><td class=\"num\">{day} {time}</td><td>{name_e}</td><td class=\"num\">{score}</td>\
             <td class=\"num\">{stability}</td><td><span class=\"badge {cls}\">{lvl_label}</span></td></tr>{detail}",
            day = day,
            time = time,
            name_e = esc(&name),
            score = f1(score),
            stability = pct1(stability),
            cls = lvl_class(&lvl.level),
            lvl_label = esc(&lvl.level_label),
            detail = detail,
        ));
    }
    format!(
        "<h2>Сессии ({n})</h2><table><thead><tr><th>Дата (UTC)</th><th>Лучшая схема</th>\
         <th class=\"num\">Медиана, тик/с</th><th class=\"num\">Стабильность</th><th>Уровень</th></tr></thead>\
         <tbody>{rows}</tbody></table>",
        n = sessions.len(),
        rows = rows,
    )
}

/// Развёрнутая часть по одной сессии (схемы + рекомендация).
fn scheme_detail_block(s: &SessionJson, stamp: &str) -> String {
    let rec = &s.recommendation;
    let mut sch_rows = String::new();
    let winner = rec
        .recommended_scheme
        .as_ref()
        .and_then(|id| s.schemes.iter().find(|x| &x.scheme_id == id));
    let tie = matches!(rec.level.as_str(), "Equivalent" | "KeepCurrent");
    let has_winner = !tie && winner.is_some();
    for sch in &s.schemes {
        let status = if sch.rejected {
            format!(
                "<span class=\"badge err\">{0}</span>",
                esc(sch.rejection_reason.as_deref().unwrap_or("забракована"))
            )
        } else {
            let is_win = has_winner
                && winner
                    .map(|w| w.scheme_id == sch.scheme_id)
                    .unwrap_or(false);
            if is_win {
                "<span class=\"badge ok\">рекомендована</span>".to_string()
            } else {
                "<span class=\"badge\">допущена</span>".to_string()
            }
        };
        let name = sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone());
        sch_rows.push_str(&format!(
            "<tr><td>{name}</td><td class=\"num\">{runs}</td><td class=\"num\">{median}</td>\
             <td class=\"num\">{cv}</td><td class=\"num\">{cons}</td><td class=\"num\">{worst}</td>\
             <td class=\"num\">{purity}</td><td class=\"num\">{mean}</td><td>{status}</td></tr>",
            name = esc(&name),
            runs = sch.runs,
            median = f1(sch.median_throughput),
            cv = pct1(sch.run_variation_percent),
            cons = pct1(sch.median_consistency_percent),
            worst = f1(sch.median_worst_window_throughput),
            purity = sch
                .median_background_purity
                .map(pct1)
                .unwrap_or_else(|| "—".to_string()),
            mean = f1(sch.mean_average_throughput),
            status = status,
        ));
    }
    let winner_name = winner
        .map(|w| esc(&w.name.clone().unwrap_or_else(|| w.scheme_id.clone())))
        .unwrap_or_else(|| "—".to_string());
    let margin = rec
        .expected_margin_percent
        .map(f2)
        .unwrap_or_else(|| "—".to_string());
    let pbest = rec.probabilities.map(|p| p[0]).unwrap_or(f64::NAN);
    let pgt0 = rec.probabilities.map(|p| p[1]).unwrap_or(f64::NAN);
    let pgt1 = rec.probabilities.map(|p| p[2]).unwrap_or(f64::NAN);
    let probs = format!(
        "{0}{1}{2}{3}",
        prob_bar("Перевес над вторым местом (ожид.)", &margin),
        prob_bar("P(лидер — лучший)", &pct_str(pbest)),
        prob_bar("P(перевес > 0)", &pct_str(pgt0)),
        prob_bar("P(перевес > 1%)", &pct_str(pgt1)),
    );
    let mode_line = match (&rec.bootstrap_mode, &rec.tie_criterion) {
        (Some(mode), Some(tie)) if tie != "None" => {
            format!("bootstrap: {} · ничья: {}", esc(mode), esc(tie))
        }
        (Some(mode), _) => format!("bootstrap: {}", esc(mode)),
        _ => String::new(),
    };
    let mode_html = if mode_line.is_empty() {
        String::new()
    } else {
        format!("<div class=\"note\">{mode_line}</div>")
    };

    let rec_line = if has_winner {
        format!(
            "Рекомендована: <b>{winner_name}</b> · перевес {margin}% · {lvl_label}",
            winner_name = winner_name,
            margin = margin,
            lvl_label = esc(&rec.level_label)
        )
    } else {
        let verb = match rec.level.as_str() {
            "Equivalent" => "схемы эквивалентны — значимых различий не выявлено",
            "KeepCurrent" => "оставить текущую схему",
            _ => "данных недостаточно для рекомендации",
        };
        format!(
            "Вердикт: {verb} · {lvl_label}",
            lvl_label = esc(&rec.level_label)
        )
    };

    format!(
        "<tr class=\"detail\"><td colspan=\"5\"><div class=\"detail-body\">\
         <div class=\"dhead\">Сессия <span class=\"mono\">{plan}</span> · {stamp} (UTC)</div>\
<table class=\"sub\"><thead><tr><th>Схема</th><th class=\"num\">Прогоны</th>\
         <th class=\"num\">Медиана, тик/с</th><th class=\"num\">CV</th>\
         <th class=\"num\">Стабильность</th><th class=\"num\">Худш. секунда</th>\
         <th class=\"num\">Фон</th><th class=\"num\">Среднее</th><th>Статус</th></tr></thead>\
         <tbody>{sch_rows}</tbody></table>\
<div class=\"rec\">{rec_line}</div>\
         <div class=\"note\">{reason}</div>{score}{probs}{mode_html}</div></td></tr>",
        plan = esc(&s.plan_guid),
        stamp = esc(stamp),
        sch_rows = sch_rows,
        rec_line = rec_line,
        reason = esc(&rec.reason),
        score = score_section(s),
        probs = probs,
        mode_html = mode_html,
    )
}

/// Секция отчёта «Балл по вашим весам» + информирование о досрочной остановке.
fn score_section(s: &SessionJson) -> String {
    let weights = crate::score::ScoreWeights {
        performance: s.score_weights[0],
        stability: s.score_weights[1],
        worst_second: s.score_weights[2],
    };
    let n = weights.normalized();
    let scores = crate::score::score_schemes(&s.schemes, &weights);
    let leader = crate::score::score_leader(&scores);
    let mut admitted = Vec::new();
    let mut bars = String::new();
    for sc in &scores {
        let name = sc.name.clone().unwrap_or_else(|| sc.scheme_id.clone());
        let is_leader = leader.map(|l| l.scheme_id == sc.scheme_id).unwrap_or(false);
        let badge = if sc.rejected {
            "<span class=\"badge err\">забракована</span>".to_string()
        } else if is_leader {
            "<span class=\"badge ok\">лидер по баллу</span>".to_string()
        } else {
            "<span class=\"badge\">допущена</span>".to_string()
        };
        if !sc.rejected && sc.score.is_finite() {
            admitted.push(sc.score);
        }
        let mean = s
            .schemes
            .iter()
            .find(|sch| sch.scheme_id.eq_ignore_ascii_case(&sc.scheme_id))
            .and_then(|sch| {
                (sch.mean_average_throughput.is_finite() && sch.mean_average_throughput > 0.0)
                    .then_some(sch.mean_average_throughput)
            });
        let mean_html = mean
            .map(|m| format!(" · <span class=\"muted\">{m:.0} тик/с</span>"))
            .unwrap_or_default();
        let p = if sc.score.is_finite() && sc.score > 0.0 {
            sc.score.min(100.0)
        } else {
            0.0
        };
        bars.push_str(&format!(
            "<div class=\"pbar\"><span class=\"plab\">{name}{mean_html}</span>\
             <div class=\"ptrack\"><i style=\"width:{p:.0}%\"></i></div>\
             <span class=\"pval\">{score:.1}</span>{badge}</div>",
            name = esc(&name),
            mean_html = mean_html,
            p = p,
            score = sc.score,
            badge = badge,
        ));
    }
    let close_note = if admitted.len() >= 2 {
        admitted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let diff = admitted[0] - admitted[1];
        if diff < 1.0 {
            format!(
                "<div class=\"note\">Баллы лидера и второго места почти неразличимы \
                 (разница {diff:.1} из 100) — ранжир по баллам нестабилен; оцените перевес и стабильность.",
                diff = diff
            )
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    let early_note = s
        .early_stop_reason
        .as_ref()
        .map(|r| {
            format!(
                "<div class=\"note\"> Досрочная остановка ({rd}/{rp} раундов): {r}</div>",
                rd = s.rounds_completed,
                rp = s.rounds_planned,
                r = esc(r)
            )
        })
        .unwrap_or_default();
    format!(
        "<h2>Балл по вашим весам</h2>\
         <div class=\"note\">Веса: производительность {wp:.0}% · стабильность {ws:.0}% · \
         худшая секунда {ww:.0}% (нормируются совместно; 100 = лучшая среди допущенных)</div>\
         {bars}{close_note}{early_note}",
        wp = n[0] * 100.0,
        ws = n[1] * 100.0,
        ww = n[2] * 100.0,
        bars = bars,
        close_note = close_note,
        early_note = early_note,
    )
}

/// Горизонтальная полоса вероятности для отчёта.
fn prob_bar(label: &str, value: &str) -> String {
    let width = num_from_pct(value);
    let width = if width.is_finite() && (0.0..=100.0).contains(&width) {
        width
    } else {
        0.0
    };
    format!(
        "<div class=\"pbar\"><span class=\"plab\">{label}</span><div class=\"ptrack\"><i style=\"width:{width:.0}%\"></i></div><span class=\"pval\">{value}</span></div>"
    )
}

/// Из «62%» — 62.0 (для полосы вероятностей); для произвольных значений 0.
fn num_from_pct(value: &str) -> f64 {
    value
        .strip_suffix('%')
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn pct_str(v: f64) -> String {
    if v.is_finite() {
        format!("{:.0}%", v * 100.0)
    } else {
        "—".to_string()
    }
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
        "—".to_string()
    }
}

fn pct1(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.1}%")
    } else {
        "—".to_string()
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn lvl_class(level: &str) -> &'static str {
    match level {
        "Confirmed" => "ok",
        "Probable" => "acc",
        "None" => "",
        _ => "warn",
    }
}

/// Лучшая (незабракованная) схема сессии по median throughput.
fn best_scheme(s: &SessionJson) -> Option<&crate::result::SchemeJson> {
    let accepted: Vec<&crate::result::SchemeJson> =
        s.schemes.iter().filter(|x| !x.rejected).collect();
    let pool: Vec<&crate::result::SchemeJson> = if accepted.is_empty() {
        s.schemes.iter().collect()
    } else {
        accepted
    };
    pool.into_iter().max_by(|a, b| {
        let av = if a.median_throughput.is_finite() {
            a.median_throughput
        } else {
            f64::MIN
        };
        let bv = if b.median_throughput.is_finite() {
            b.median_throughput
        } else {
            f64::MIN
        };
        av.total_cmp(&bv)
    })
}

fn periods(sessions: &[&SessionJson]) -> (String, String) {
    if sessions.is_empty() {
        return ("—".to_string(), "—".to_string());
    }
    let min = sessions
        .iter()
        .filter_map(|s| session_started_at_ns(s))
        .min()
        .unwrap_or(0);
    let max = sessions
        .iter()
        .filter_map(|s| session_started_at_ns(s))
        .max()
        .unwrap_or(0);
    (
        date_slice(&date_time_stamp(min), 6, 8),
        date_slice(&date_time_stamp(max), 6, 8),
    )
}

/// `дд.мм.гггг` из UTC-метки `YYYYMMDDTHHMMSSZ###`.
fn date_slice(stamp: &str, day0: usize, day1: usize) -> String {
    if stamp.len() < 8 {
        return stamp.to_string();
    }
    format!("{}.{}.{}", &stamp[day0..day1], &stamp[4..6], &stamp[0..4])
}

fn short_date(s: &SessionJson) -> String {
    session_started_at_ns(s)
        .map(date_time_stamp)
        .map(|st| format!("{}.{}.{}", &st[6..8], &st[4..6], &st[0..4]))
        .unwrap_or_default()
}

fn nice_step(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let mag = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if raw <= m * mag {
            return m * mag;
        }
    }
    10.0 * mag
}

fn now_unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{RESULTS_DIR_NAME, save_result_in};
    use crate::result::{IdentityJson, RecommendationJson, SchemeJson};

    fn sample_session(id: &str, median: f64) -> SessionJson {
        let mut sch = SchemeJson::from_aggregate(
            id.to_string(),
            false,
            None,
            &empty_aggregate(median),
            Vec::new(),
        );
        sch.name = Some(format!("План {id}"));
        SessionJson {
            plan_guid: format!("plan-{id}"),
            original_scheme_guid: None,
            original_restored: false,
            identity: IdentityJson {
                workload_version: "GamingCpuV1".into(),
                config_hash: "H".into(),
                seed_hex: "S".into(),
                worker_count: 4,
                logical_cpus: 8,
                timer_hz: 10_000_000,
                cpu_identifier: "cpu".into(),
                diagnostics_version: "0.1.0".into(),
            },
            schemes: vec![sch],
            recommendation: RecommendationJson {
                level: "Confirmed".into(),
                level_label: "Подтверждено".into(),
                recommended_scheme: Some(id.into()),
                runner_up_scheme: None,
                reason: "тест".into(),
                probabilities: Some([0.96, 0.9, 0.8]),
                expected_margin_percent: Some(2.5),
                bootstrap_mode: Some("PairedByRun".into()),
                tie_criterion: Some("None".into()),
            },
            warnings: Vec::new(),
            rounds_planned: 3,
            rounds_completed: 3,
            early_stop_reason: None,
            score_weights: [50.0, 30.0, 20.0],
        }
    }

    fn empty_aggregate(median: f64) -> powerbench_metrics::AggregateResult {
        powerbench_metrics::AggregateResult {
            runs: 1,
            mean_average_throughput: median,
            sample_std: 0.0,
            t_value: 0.0,
            margin: 0.0,
            ci_95: [0.0, 0.0],
            run_variation_percent: 1.0,
            cv_warning: false,
            median_throughput: median,
            median_p1_throughput: 0.0,
            median_p01_throughput: 0.0,
            median_p95_execution_time_ms: 0.0,
            median_p99_execution_time_ms: 0.0,
            median_consistency_percent: 96.0,
            median_burst_retention_percent: 0.0,
            median_jitter_p99_ms: 0.0,
            median_worst_window_throughput: 0.0,
            median_background_purity: None,
            run_duration_ms: 0,
            started_at_min_ns: 0,
        }
    }

    #[test]
    fn report_builds_complete_html() {
        let a = sample_session("AAA", 500.0);
        let b = sample_session("BBB", 520.0);
        let html = build_report(&[a, b]);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("Отчёт по истории"));
        assert!(html.contains("Сессии (2)"));
        assert!(html.contains("План AAA"));
        assert!(html.contains("Подтверждено"));
        assert!(html.contains("P(лидер — лучший)"));
        assert!(html.contains("96%"));
        assert!(html.ends_with("</body></html>"));
        assert!(html.contains("<svg"));
    }

    #[test]
    fn report_empty_history() {
        let html = build_report(&[]);
        assert!(html.contains("История пуста"));
    }

    #[test]
    fn report_filename_is_sanitizable_roundtrip() {
        // Отчёт не зависит от реальной истории; убеждаемся, что сохранение
        // результата в тестовый каталог осталось целостным.
        let dir = std::env::temp_dir().join(RESULTS_DIR_NAME);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = sample_session("CCC", 510.0);
        let saved = save_result_in(&s, &dir).unwrap();
        assert!(saved.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
