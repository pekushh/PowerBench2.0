//! Сводный HTML-отчёт по истории сессий: автономный документ с итогами,
//! графиком тенденций и детализацией по каждой сессии. Без внешних
//! зависимостей — все стили и SVG встроены.

use crate::history::{date_time_stamp, session_started_at_ns};
use crate::result::{IdentityJson, SessionJson};

/// Описание машины одной строкой для подвала отчёта.
///
/// Сборка ОС и объём памяти добавлены не для красоты: обновление Windows меняет
/// планировщик и политики питания, поэтому по отчёту должно быть видно, что все
/// сессии измерялись на одной и той же системе.
fn machine_line(id: &IdentityJson) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !id.cpu_brand.is_empty() {
        parts.push(id.cpu_brand.clone());
    }
    if !id.os_build.is_empty() {
        parts.push(format!("ОС {}", id.os_build));
    }
    if id.memory_gib > 0.0 {
        parts.push(format!("{:.0} ГБ", id.memory_gib));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("машина: {}", parts.join(" · "))
    }
}

/// Цвета серий на графике тенденций.
///
/// Палитра отчёта — графит с зелёным акцентом (как тема Graphite в
/// приложении), поэтому здесь нет синих и фиолетовых тонов: линии должны
/// читаться на тёмном фоне и различаться между собой, а не спорить с
/// оформлением. Первый цвет — зелёный, он же акцент отчёта.
const COLORS: &[&str] = &[
    "#6fd0a0", "#e4b46f", "#e57979", "#a8c686", "#d9a066", "#9a958e", "#c9a86a", "#7fb8a0",
    "#d98f8f", "#6b6660",
];

/// Потолок числа линий сетки: страховка от бесконечного цикла при
/// вырожденных значениях в данных.
const MAX_GRID_LINES: usize = 24;

/// Сколько схем одновременно рисуется на графике тенденций. Больше — уже
/// каша из пересекающихся линий; остальные схемы видно в таблицах.
const CHART_MAX_SERIES: usize = 8;

/// Сколько схем показывать плитками в сводке отчёта по истории.
const SUMMARY_MAX_CARDS: usize = 12;

const HTML_CSS: &str = r##"<!DOCTYPE html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Отчёт PowerBench</title><style>
:root{
/* Палитра отчёта = токены темы Graphite из приложения (`app/src/styles.css`).
   Раньше здесь стояли собственные, более сине-серые значения, — на фоне
   приложения отчёт выглядел «синим». Значения здесь должны совпадать с
   `:root` приложения; расхождение ловит тест `report_palette_matches_app_graphite_theme`. */
--bg-0:#0F0F0F;--bg-1:#1B1B1B;--bg-2:#2B2B2B;--bg-3:#303030;
--line-1:#f3f3f31c;--line-2:#f3f3f338;
--fg:#F3F3F3;--fg-dim:#ECECEC;--fg-mute:#7E7E7E;--accent:#FDFDFD;
--ok:#6fd0a0;--warn:#e4b46f;--err:#e57979;
--bg:var(--bg-0);--card:var(--bg-1);--card2:var(--bg-2);--line:var(--line-1);--line2:var(--line-2);
--dim:var(--fg-dim);--mute:var(--fg-mute);
--okbg:rgba(111,208,160,.12);--okline:rgba(111,208,160,.45);
--warnbg:rgba(228,180,111,.10);--warnline:rgba(228,180,111,.42);
--errbg:rgba(229,121,121,.10);--errline:rgba(229,121,121,.42);
--shadow:0 30px 70px -30px rgba(0,0,0,.8)}
*{box-sizing:border-box}body{margin:0;padding:36px 16px 60px;color:var(--fg);
background:radial-gradient(1000px 420px at 50% -6%,#1B1B1B,var(--bg) 72%);
font:14px/1.6 -apple-system,"Segoe UI",Roboto,"Helvetica Neue",Arial,sans-serif;-webkit-font-smoothing:antialiased}
body>*{max-width:1000px;margin-left:auto;margin-right:auto}
.sheet{background:var(--card);border:1px solid var(--line);border-radius:20px;overflow:hidden;box-shadow:var(--shadow)}
.topbar{display:flex;align-items:center;gap:10px;padding:16px 28px;border-bottom:1px solid var(--line)}
.logo{width:26px;height:26px;border-radius:50%;background:linear-gradient(135deg,#8FE3B4,#3F9E6E);
display:inline-grid;place-items:center;flex:0 0 auto}
.wordmark{font-size:15px;font-weight:650;letter-spacing:-.2px}
.pad{padding:28px 30px 8px}
h1{font-size:30px;margin:0;letter-spacing:-.5px;line-height:1.15}
.title-row{display:flex;align-items:center;gap:14px;flex-wrap:wrap}
.chips{margin-left:auto;display:flex;align-items:center;gap:14px}
.chip{display:inline-flex;align-items:center;gap:8px;padding:7px 15px;border:1px solid var(--line2);
border-radius:999px;background:var(--card2);font-size:13px;font-weight:600}
.chip svg{color:var(--mute)}
.workers{font-size:13px;color:var(--mute)}
.workers b{color:var(--fg);font-weight:650;font-variant-numeric:tabular-nums}
.dateline{color:var(--mute);font-size:13.5px;margin:8px 0 20px}
.stats{display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin:0 0 14px}
.stat{background:var(--card2);border:1px solid var(--line);border-radius:14px;padding:16px 18px;min-width:0}
.stat .k{font-size:10.5px;letter-spacing:1.6px;color:var(--mute);font-weight:600}
.stat .v{font-size:27px;font-weight:750;margin-top:6px;font-variant-numeric:tabular-nums;letter-spacing:-.4px;
overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.stat .v small{font-size:13px;color:var(--mute);font-weight:500}
.stat .v.green{color:var(--ok)}
.stat .s{font-size:12.5px;color:var(--mute);margin-top:4px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.rec-card{background:var(--card2);border:1px solid var(--line);border-radius:14px;padding:20px 22px;margin:0 0 6px}
.rec-label{display:flex;align-items:center;gap:8px;font-size:10.5px;letter-spacing:1.6px;color:var(--mute);font-weight:600}
.rec-label svg{color:var(--ok)}
.rec-title{font-size:19px;font-weight:700;margin:10px 0 0;letter-spacing:-.2px}
.rec-title b{color:var(--ok)}
.rec-notes{margin:8px 0 0;color:var(--mute);font-size:13px;font-style:italic;line-height:1.6}
.rec-notes div{margin-top:2px}
.rec-why{margin:8px 0 0;color:var(--mute);font-size:12px}
h2{font-size:12px;margin:28px 0 12px;letter-spacing:1.6px;color:var(--dim);font-weight:700;text-transform:uppercase}
table{width:100%;border-collapse:collapse;margin:0 0 8px;font-size:13.5px;color:var(--fg);
background:var(--card2);border:1px solid var(--line);border-radius:14px;overflow:hidden}
thead th{color:var(--mute);font-weight:600;text-align:left;padding:12px 16px;background:rgba(255,255,255,.02);
border-bottom:1px solid var(--line);font-size:10.5px;letter-spacing:1.2px;white-space:nowrap}
td{padding:13px 16px;border-bottom:1px solid var(--line);vertical-align:middle}
tbody tr:last-child td{border-bottom:0}
td.num,th.num{text-align:right;font-variant-numeric:tabular-nums}
tbody tr.win td{background:rgba(111,208,160,.05)}
tbody tr.out td{opacity:.5}
.pill{display:inline-block;padding:4px 12px;border-radius:8px;font-size:10.5px;font-weight:700;letter-spacing:1px;
border:1px solid transparent;white-space:nowrap}
.pill.ok{color:var(--ok);background:var(--okbg);border-color:var(--okline)}
.pill.dim{color:var(--dim);background:rgba(255,255,255,.05);border-color:var(--line2)}
.pill.err{color:var(--err);background:var(--errbg);border-color:var(--errline)}
.pbar{display:grid;grid-template-columns:minmax(150px,230px) 1fr auto auto;align-items:center;gap:14px;margin:12px 0}
.plab{color:var(--dim);font-size:13px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.plab small{color:var(--mute)}
.ptrack{height:7px;border-radius:999px;background:rgba(255,255,255,.08);overflow:hidden;min-width:60px}
.ptrack i{display:block;height:100%;border-radius:999px;background:#4A4A4A}
.pbar.lead .ptrack i{background:linear-gradient(90deg,#4EA87F,var(--ok))}
.pval{color:var(--fg);font-variant-numeric:tabular-nums;font-weight:700;font-size:13.5px;min-width:44px;text-align:right}

/* Категории схем: заменили «балл по вашим весам». Три колонки на широком
   экране, одна на узком — карточки не должны сжиматься в нечитаемую полосу. */
.cats{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:12px;margin:12px 0 14px}
.catcard{background:var(--bg-2);border:1px solid var(--line-1);border-radius:var(--radius-lg);padding:14px 16px}
.cat-title{font-size:var(--fs-sm);color:var(--fg-mute);text-transform:uppercase;letter-spacing:.04em}
.cat-name{font-size:var(--fs-lead,18px);font-weight:600;margin:6px 0 10px;color:var(--fg);word-break:break-word}
.cat-metrics{display:grid;gap:4px}
.cat-metrics div{display:flex;align-items:baseline;gap:8px;font-size:var(--fs-sm);color:var(--fg-dim)}
.cat-metrics span{flex:0 0 auto;min-width:132px;color:var(--fg-mute)}
.cat-metrics b{font-size:var(--fs-body);color:var(--fg);font-variant-numeric:tabular-nums}
.cat-hint{margin-top:10px;font-size:var(--fs-sm);color:var(--fg-mute);line-height:1.5}
/* Таблица метрик по фазам шире экрана: прокручивается по горизонтали
   внутри своей обёртки, а не растягивает страницу и не срезает колонки. */
.table-scroll{overflow-x:auto;-webkit-overflow-scrolling:touch;max-width:100%}
table.phases{border-collapse:collapse;width:100%}
table.phases th,table.phases td{white-space:nowrap}
/* Подзаголовок групп фаз — не «жирный» заголовок уровня фазы. */
table.phases thead tr:nth-child(2) th{font-weight:400;font-size:var(--fs-sm);color:var(--fg-mute);text-align:right}
.meta{color:var(--mute);font-size:12.5px;line-height:1.6;margin-bottom:2px}
.muted{color:var(--mute)}
.note{color:var(--mute);font-size:12.5px;margin-top:8px;line-height:1.55}
.note-stat{font-size:12.5px;color:var(--mute)}
.mono{font-family:Consolas,Menlo,monospace;font-size:12px;color:var(--dim);background:rgba(255,255,255,.05);
padding:2px 7px;border-radius:6px;border:1px solid var(--line)}
.sumcards{display:flex;flex-wrap:wrap;gap:12px}
.sumcard{flex:1 1 200px;min-width:170px;background:var(--card2);border:1px solid var(--line);
border-radius:14px;padding:16px 18px}
.sc-name{font-size:11.5px;color:var(--mute);font-weight:700;letter-spacing:.4px;text-transform:uppercase;
overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.sc-num{font-size:22px;font-weight:700;margin-top:5px;font-variant-numeric:tabular-nums;letter-spacing:-.3px}
.sc-num small{font-size:12.5px;color:var(--mute);font-weight:500;margin-left:4px}
.sc-sub{font-size:12px;color:var(--mute);margin-top:3px}
header.hero{background:var(--card2);border:1px solid var(--line);border-radius:16px;padding:24px 28px}
.brand{display:inline-flex;align-items:center;gap:8px;font-size:11px;letter-spacing:2.2px;text-transform:uppercase;
color:var(--ok);font-weight:700;background:var(--okbg);border:1px solid var(--okline);
padding:5px 13px;border-radius:999px}
header.hero h1{margin-top:12px}
.chart{background:var(--card2);border:1px solid var(--line);border-radius:14px;padding:14px 6px 6px 6px;margin-bottom:22px}
.chart svg{display:block}
.panel{fill:var(--card2)}
.grid{stroke:var(--line);stroke-width:1}
.axis{stroke:var(--mute);stroke-width:1}
.ylab,.xlab{fill:var(--mute);font-size:11px;font-variant-numeric:tabular-nums}
.legend{fill:var(--dim);font-size:11px}
.badge{display:inline-flex;align-items:center;gap:6px;padding:3px 11px;border-radius:999px;font-size:11.5px;
font-weight:600;border:1px solid var(--line2);background:rgba(255,255,255,.05);color:var(--dim);white-space:nowrap}
.badge.ok{color:var(--ok);background:var(--okbg);border-color:var(--okline)}
.badge.acc{color:var(--dim);background:rgba(255,255,255,.05);border-color:var(--line2)}
.badge.warn{color:var(--warn);background:var(--warnbg);border-color:var(--warnline)}
.badge.err{color:var(--err);background:var(--errbg);border-color:var(--errline)}
tr.detail td{background:rgba(255,255,255,.015);padding:16px 20px;border-top:1px solid var(--line)}
.detail-body{margin:0 0 18px}
.dhead{color:var(--dim);font-weight:700;font-size:12px;margin:0 0 10px;text-transform:uppercase;letter-spacing:.5px}
table.sub{border-radius:10px;margin:10px 0 14px;background:transparent;border:0}
table.sub th{background:transparent;border-bottom:1px solid var(--line)}
.rec{margin-top:12px;padding:12px 16px;background:rgba(255,255,255,.03);border:1px solid var(--line);
border-radius:12px;font-size:13px;color:var(--dim)}
.rec b{color:var(--ok)}
.verdict{display:flex;flex-wrap:wrap;align-items:center;gap:10px 14px;background:var(--card2);
border:1px solid var(--line);border-left:4px solid var(--ok);border-radius:14px;padding:18px 22px;font-size:15px}
.verdict .who{font-size:20px;font-weight:700;letter-spacing:-.3px}
.foot{color:var(--mute);font-size:12px;text-align:center;padding:20px 20px 24px;border-top:1px solid var(--line)}
.foot .prov{display:block;margin-top:4px;font-size:11px;opacity:.75}
.warn-list{list-style:none;padding:0;margin-top:6px}
.warn-list li{margin:2px 0;color:#e0b060}
@media (max-width:640px){body{padding:20px 10px 44px}.pad{padding:20px 16px 6px}h1{font-size:23px}
.stats{grid-template-columns:1fr 1fr}.pbar{grid-template-columns:1fr auto}table{display:block;overflow-x:auto}
.sumcards{flex-direction:column}.chips{margin-left:0}}
@media print{body{background:#fff;color:#111}.sheet{box-shadow:none}}
/* ===== Большие отчёты: 100+ схем =====
   Инструменты над таблицей и липкая шапка решают главную проблему — длинную
   ленту вниз. Сами строки не виртуализируются: это обычный текст, 100 `<tr>`
   браузеру не тяжело, а данные остаются доступны Ctrl+F. */
.sch-tools{display:flex;flex-wrap:wrap;align-items:center;gap:10px;margin:0 0 12px}
.sch-tools input[type=search],.sch-tools select{background:var(--card2);color:var(--fg);
 border:1px solid var(--line2);border-radius:10px;padding:8px 12px;font:inherit;font-size:13px;min-width:0}
.sch-tools input[type=search]{flex:1 1 220px}
.sch-tools input[type=search]:focus,.sch-tools select:focus{outline:2px solid var(--okline);outline-offset:1px}
.sch-count{color:var(--mute);font-size:12.5px;font-variant-numeric:tabular-nums;margin-left:auto}
.sch-morewrap{display:flex;align-items:center;gap:8px;margin:0 0 12px;color:var(--mute);font-size:12.5px}
.sch-more{background:var(--card2);color:var(--fg);border:1px solid var(--line2);border-radius:10px;
 padding:8px 14px;font:inherit;font-size:13px;cursor:pointer}
.sch-more:hover{border-color:var(--okline)}
table thead th{position:sticky;top:0;z-index:2;background:#161616}
tr.sess-row{cursor:pointer}
tr.sess-row:hover td{background:rgba(255,255,255,.035)}
tr.sess-row td:first-child::before{content:"▸ ";color:var(--mute)}
tr.sess-row.open td:first-child::before{content:"▾ ";color:var(--ok)}
.to-top{position:fixed;right:22px;bottom:22px;width:40px;height:40px;border-radius:50%;
 border:1px solid var(--line2);background:var(--card2);color:var(--fg);cursor:pointer;font-size:15px;
 box-shadow:var(--shadow);display:none}
.to-top.on{display:block}
@media (max-width:640px){.sch-count{margin-left:0}}
</style></head><body>
"##;

/// Ванильный JS для больших отчётов: поиск, фильтр и сортировка по схемам,
/// сворачивание деталей сессий.
///
/// Без внешних зависимостей — отчёт должен открываться двойным кликом офлайн.
/// Это прогрессивное улучшение: без JS отчёт остаётся полным и читаемым,
/// просто без навигации.
const HTML_JS: &str = r##"
<script>
(function () {
  function initSchemes(box) {
    var table = box.querySelector('table');
    var body = table && table.tBodies[0];
    if (!body || !body.rows.length) return;
    var rows = Array.prototype.slice.call(body.rows);
    var search = box.querySelector('.sch-search');
    var status = box.querySelector('.sch-status');
    var sort = box.querySelector('.sch-sort');
    var counter = box.querySelector('.sch-count');
    var more = box.querySelector('.sch-more');
    var moreWrap = box.querySelector('.sch-morewrap');
    // Список длинный: схем больше PAGE показываем постранично, остальные
    // раскрывает кнопка.
    var PAGE = 60;
    // Состояние «показаны все» держим в переменной, а не в свойстве
    // `checked` кнопки: у `<button>` такого свойства нет, оно всегда
    // `undefined`, из-за чего кнопка «Показать все» никогда не срабатывала,
    // а строки после шестидесятой оставались скрыты навсегда.
    var showAll = false;

    rows.forEach(function (tr, i) { tr.dataset.order = String(i); });

    function apply() {
      var q = (search.value || '').toLowerCase().trim();
      var st = status.value;
      var by = sort.value;
      var hit = rows.filter(function (tr) {
        var okQ = !q || (tr.dataset.name || '').indexOf(q) >= 0;
        var okS = st === 'all' || tr.dataset.status === st;
        return okQ && okS;
      });
      hit.sort(function (a, b) {
        if (by === 'name') {
          return (a.dataset.name || '').localeCompare(b.dataset.name || '', 'ru');
        }
        if (by === 'median') {
          var av = parseFloat(a.dataset.median);
          var bv = parseFloat(b.dataset.median);
          // Схема без замеров (пусто или «—») уходит вниз, а не вверх.
          return (isNaN(bv) ? -Infinity : bv) - (isNaN(av) ? -Infinity : av);
        }
        if (by === 'runs') {
          return (parseInt(b.dataset.runs, 10) || 0) - (parseInt(a.dataset.runs, 10) || 0);
        }
        return (+a.dataset.order) - (+b.dataset.order);
      });
      hit.forEach(function (tr) { body.appendChild(tr); });
      var limit = showAll ? hit.length : Math.min(PAGE, hit.length);
      rows.forEach(function (tr) { tr.style.display = 'none'; });
      hit.slice(0, limit).forEach(function (tr) { tr.style.display = ''; });
      counter.textContent = 'Показано ' + Math.min(limit, hit.length) + ' из ' + rows.length
        + (hit.length === rows.length ? '' : ' · найдено ' + hit.length);
      moreWrap.style.display = hit.length > PAGE ? '' : 'none';
      more.textContent = showAll
        ? 'Свернуть до ' + PAGE
        : 'Показать все (' + (hit.length - Math.min(PAGE, hit.length)) + ')';
      more.setAttribute('aria-expanded', showAll ? '1' : '0');
    }

    [search, status, sort].forEach(function (el) {
      el.addEventListener('input', apply);
      if (el === status || el === sort) el.addEventListener('change', apply);
    });
    more.addEventListener('click', function () { showAll = !showAll; apply(); });
    search.addEventListener('keydown', function (e) {
      if (e.key === 'Escape') { search.value = ''; apply(); }
    });
    // «/» фокусирует поиск — привычно для длинных списков.
    box.addEventListener('keydown', function (e) {
      if (e.key === '/' && document.activeElement !== search) { e.preventDefault(); search.focus(); }
    });
    box.tabIndex = -1;
    apply();
  }

  function initSessions() {
    var rows = Array.prototype.slice.call(document.querySelectorAll('tr.sess-row'));
    if (!rows.length) return;
    function pair(row) {
      var out = [], n = row.nextElementSibling;
      while (n && n.classList.contains('detail')) { out.push(n); n = n.nextElementSibling; }
      return out;
    }
    function setOpen(row, open) {
      pair(row).forEach(function (d) { d.style.display = open ? '' : 'none'; });
      row.classList.toggle('open', open);
    }
    rows.forEach(function (row, i) {
      // Первая сессия открыта: страница должна сразу показать, что внутри.
      setOpen(row, i === 0);
      row.addEventListener('click', function () {
        setOpen(row, !row.classList.contains('open'));
      });
    });
  }

  function initToTop() {
    var btn = document.querySelector('.to-top');
    if (!btn) return;
    window.addEventListener('scroll', function () {
      btn.classList.toggle('on', window.scrollY > 600);
    }, { passive: true });
    btn.addEventListener('click', function () { window.scrollTo(0, 0); });
  }

  function init() {
    Array.prototype.forEach.call(document.querySelectorAll('[data-schemes]'), initSchemes);
    initSessions();
    initToTop();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
</script>
"##;

/// Панель управления над таблицей схем: поиск, фильтр по статусу, сортировка.
///
/// JS (`HTML_JS`) навешивается один раз и работает с любым блоком
/// `[data-schemes]`, поэтому панель одинаково подходит и отчёту по сессии,
/// и сводному отчёту по истории.
fn scheme_toolbar() -> String {
    "<div class=\"sch-tools\">\
     <input type=\"search\" class=\"sch-search\" placeholder=\"Поиск по названию схемы\" \
      aria-label=\"Поиск по названию схемы\">\
     <select class=\"sch-status\" aria-label=\"Фильтр по статусу\">\
     <option value=\"all\">Все статусы</option>\
     <option value=\"admitted\">Допущены</option>\
     <option value=\"rejected\">Забракованы</option>\
     <option value=\"lead\">Лидер / рекомендация</option></select>\
     <select class=\"sch-sort\" aria-label=\"Сортировка\">\
     <option value=\"order\">Как в отчёте</option>\
     <option value=\"name\">По названию</option>\
     <option value=\"median\">По медиане</option>\
     <option value=\"runs\">По числу прогонов</option></select>\
     <span class=\"sch-count\"></span></div>\
     <div class=\"sch-morewrap\"><button type=\"button\" class=\"sch-more\"></button></div>"
        .to_string()
}

/// Обёртка таблицы схем вместе с панелью управления и разметкой для JS.
fn scheme_block(table_html: &str) -> String {
    format!("<div data-schemes>{}{table_html}</div>", scheme_toolbar())
}

/// Атрибуты строки для поиска/фильтра/сортировки в браузере.
///
/// `data-name` — уже экранированное имя: JS читает его как текст, а не как
/// HTML, поэтому экранирование здесь достаточно ровно один раз.
fn scheme_row_attrs(name: &str, runs: usize, median: f64, status: &str) -> String {
    format!(
        " data-name=\"{}\" data-runs=\"{runs}\" data-median=\"{}\" data-status=\"{status}\"",
        esc(&name.to_lowercase()),
        if median.is_finite() {
            format!("{median:.4}")
        } else {
            String::new()
        },
    )
}

/// Построить законченный HTML-документ отчёта по сессии.
pub fn build_report(sessions: &[SessionJson]) -> String {
    let now_stamp = date_time_stamp(now_unix_ns());
    let mut ordered: Vec<&SessionJson> = sessions.iter().collect();
    ordered.sort_by_key(|a| session_started_at_ns(a).unwrap_or(0));

    let header = render_header(&ordered);
    let chart = render_chart(&ordered);
    let sessions_html = render_sessions(&ordered);

    format!(
        "{HTML_CSS}{header}{chart}{sessions_html}<div class=\"foot\">Сгенерировано {now_stamp} (UTC)</div>\
         <button type=\"button\" class=\"to-top\" aria-label=\"Наверх\">↑</button>{js}</body></html>",
        js = HTML_JS
    )
}

/// Компактный HTML-отчёт по одной сессии — то, что приложение показывает и
/// сохраняет по умолчанию.
///
/// Формат собран в [`crate::report_compact`]: вердикт, три ориентира, одна
/// таблица схем с поиском/фильтром/сортировкой и три карточки диагностики на
/// токенах темы Graphite, где «хорошее» белое, а не зелёное.
pub fn build_session_report(s: &SessionJson) -> String {
    crate::report_compact::build(s)
}

/// Развёрнутый отчёт по одной сессии: те же данные, но отдельными длинными
/// разделами (категории, таблица фаз, условия замера, фон, фоновый журнал).
///
/// Основной формат приложения — [`build_session_report`]. Этот оставлен для
/// разбора результата: когда нужно посмотреть всё сразу, а не прокручивать
/// один экран.
pub fn build_session_report_verbose(s: &SessionJson) -> String {
    let start = session_started_at_ns(s).unwrap_or(0);
    let stamp = date_time_stamp(start);
    let rec = &s.recommendation;
    let id = &s.identity;
    let winner = rec
        .recommended_scheme
        .as_ref()
        // Регресс H48/H49: GUID сравнивается регистронезависимо. `powercfg`
        // отдаёт идентификаторы в верхнем регистре, а рекомендация могла прийти
        // из чекпоинта, сохранённого в нижнем, — и победитель в отчёте просто
        // исчезал («рекомендация: —» при непустой рекомендации).
        .and_then(|g| {
            s.schemes
                .iter()
                .find(|x| x.scheme_id.eq_ignore_ascii_case(g))
        });
    let winner_name = winner
        .map(|w| esc(&w.name.clone().unwrap_or_else(|| w.scheme_id.clone())))
        .unwrap_or_else(|| "—".to_string());
    let margin = rec
        .expected_margin_percent
        .map(|m| format!("{m:.2}%"))
        .unwrap_or_else(|| "—".to_string());
    let tie = matches!(rec.level.as_str(), "Equivalent" | "KeepCurrent");
    let has_winner = !tie && winner.is_some();

    // Робастный лидер (по медиане) — второе мнение поверх recommend/.
    let robust = crate::leader::robust_leader(&s.schemes);
    let robust_id = robust.as_ref().map(|r| r.scheme_id.clone());
    // Строка подсвечивается, если это рекомендованная схема либо (при ничьей
    // или отсутствии рекомендации) лидер по медиане.
    let win_id: Option<&str> =
        winner
            .map(|w| w.scheme_id.as_str())
            .or(if tie { robust_id.as_deref() } else { None });

    let mut sch_rows = String::new();
    for sch in &s.schemes {
        let is_win = win_id.map(|w| w == sch.scheme_id).unwrap_or(false);
        let row_status = if sch.rejected {
            "rejected"
        } else if is_win {
            "lead"
        } else if is_measured(sch) {
            "admitted"
        } else {
            "unmeasured"
        };
        let status = if sch.rejected {
            format!(
                "<span class=\"pill err\" title=\"{0}\">ЗАБРАКОВАНА</span>",
                esc(sch.rejection_reason.as_deref().unwrap_or("забракована"))
            )
        } else if is_win {
            "<span class=\"pill ok\">ЛИДЕР</span>".to_string()
        } else if is_measured(sch) {
            "<span class=\"pill dim\">ДОПУЩЕНА</span>".to_string()
        } else {
            // Раньше такая схема выглядела как обычная допущенная, хотя
            // прогонов не было ни одного и сравнивать её было не с чем.
            "<span class=\"pill dim\" title=\"ни одного законченного прогона\">НЕ ИЗМЕРЕНА</span>"
                .to_string()
        };
        let row_cls = if is_win {
            "win"
        } else if sch.rejected {
            "out"
        } else {
            ""
        };
        // CV по одному прогону — всегда 0 по определению: честнее прочерк.
        let deviation = if sch.runs < 2 {
            "—".to_string()
        } else {
            pct1(sch.run_variation_percent)
        };
        let plain = sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone());
        let name = esc(&plain);
        sch_rows.push_str(&format!(
            "<tr{cls}{attrs}><td>{name}</td><td class=\"num\">{runs}</td><td class=\"num\">{median}</td>\
             <td class=\"num\">{dev}</td><td class=\"num\">{cons}</td>\
             <td>{status}</td></tr>",
            cls = if row_cls.is_empty() {
                String::new()
            } else {
                format!(" class=\"{row_cls}\"")
            },
            attrs = scheme_row_attrs(&plain, sch.runs, sch.median_throughput, row_status),
            name = name,
            runs = sch.runs,
            median = f1(sch.median_throughput),
            dev = deviation,
            cons = pct1(sch.median_consistency_percent),
            status = status,
        ));
    }
    if sch_rows.is_empty() {
        sch_rows = "<tr><td colspan=\"6\">Нет данных по схемам.</td></tr>".to_string();
    }

    // Карточка рекомендации: заголовок + курсивные пометки качества.
    let rec_title = if has_winner {
        format!("Рекомендуем: <b>{rec_name}</b>", rec_name = winner_name)
    } else {
        match rec.level.as_str() {
            "Equivalent" => "Схемы эквивалентны".to_string(),
            "KeepCurrent" => "Оставляем текущую схему".to_string(),
            _ => "Лидер не определён".to_string(),
        }
    };
    let mut rec_notes: Vec<String> = match &robust {
        None => vec!["Все схемы забракованы — сравнивать нечего.".to_string()],
        Some(r)
            if r.flags.is_empty() && r.confidence == crate::leader::LeaderConfidence::Normal =>
        {
            vec!["Лидер устойчив по медиане, выбросов и шума нет.".to_string()]
        }
        Some(r) => {
            let mut v = r.flags.clone();
            // Расхождение двух методов выбора — отдельная пометка.
            // MSRV 1.85: схлопывание через let-цепочки требует Rust 1.88+.
            #[allow(clippy::collapsible_if)]
            if let (Some(rid), Some(w)) = (&robust_id, winner) {
                if rid != &w.scheme_id {
                    v.push(format!(
                        "методы расходятся: по среднему лидирует «{}», по медиане — «{}» (среднее искажено выбросами)",
                        w.name.clone().unwrap_or_else(|| w.scheme_id.clone()),
                        r.name.clone().unwrap_or_else(|| r.scheme_id.clone()),
                    ));
                }
            }
            v
        }
    };
    if !rec.reason.is_empty() {
        rec_notes.push(rec.reason.clone());
    }
    let rec_notes_html: String = rec_notes
        .iter()
        .map(|n| format!("<div>{}</div>", esc(n)))
        .collect();

    // Схема без единого законченного прогона не «допущена»: её не с чем было
    // сравнивать, и в счётчик попадала как равная остальным.
    let admitted = s.schemes.iter().filter(|x| is_measured(x)).count();
    let rejected_n = s.schemes.iter().filter(|x| x.rejected).count();
    let unmeasured_n = s.schemes.len() - admitted - rejected_n;
    let total_runs: usize = s.schemes.iter().map(|x| x.runs).sum();
    let (lead_value, lead_sub) = match &robust {
        Some(r) => (
            format!("{:.0} <small>тик/с</small>", r.median),
            r.name.clone().unwrap_or_else(|| r.scheme_id.clone()),
        ),
        None => ("—".to_string(), "нет данных".to_string()),
    };
    let schemes_sub = match (rejected_n, unmeasured_n) {
        (0, 0) => "Допущено".to_string(),
        (0, u) => format!("Допущено · без замеров: {u}"),
        (r, 0) => format!("Допущено · брак: {r}"),
        (r, u) => format!("Допущено · брак: {r} · без замеров: {u}"),
    };

    format!(
        "{HTML_CSS}<div class=\"sheet\">\
         <div class=\"topbar\"><span class=\"logo\">\
         <svg width=\"13\" height=\"13\" viewBox=\"0 0 24 24\"><path d=\"M13 2 4 14h6l-1 8 9-12h-6l1-8z\" fill=\"#1B1B1B\"/></svg>\
         </span><span class=\"wordmark\">PowerBench</span></div>\
         <div class=\"pad\">\
         <div class=\"title-row\"><h1>Отчёт по сессии</h1>\
         <div class=\"chips\"><span class=\"chip\">\
         <svg width=\"15\" height=\"15\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\">\
         <rect x=\"2\" y=\"7\" width=\"20\" height=\"11\" rx=\"5.5\"/>\
         <circle cx=\"8.5\" cy=\"12.5\" r=\"1.2\" fill=\"currentColor\" stroke=\"none\"/>\
         <circle cx=\"15.5\" cy=\"12.5\" r=\"1.2\" fill=\"currentColor\" stroke=\"none\"/></svg>\
         {wl}</span><span class=\"workers\">Воркеры: <b>{wc} / {lcpus}</b></span></div></div>\
         <div class=\"dateline\">{day}</div>\
         <div class=\"stats\">\
         <div class=\"stat\"><div class=\"k\">ЛИДЕР</div><div class=\"v green\">{lead_value}</div>\
         <div class=\"s\">{leader_name}</div></div>\
         <div class=\"stat\"><div class=\"k\">ПЕРЕВЕС</div><div class=\"v\">{margin}</div>\
         <div class=\"s\">{level_label}</div></div>\
         <div class=\"stat\"><div class=\"k\">СХЕМ</div><div class=\"v\">{admitted}</div>\
         <div class=\"s\">{schemes_sub}</div></div>\
         <div class=\"stat\"><div class=\"k\">ПРОГОНОВ</div><div class=\"v\">{runs}</div>\
         <div class=\"s\">Всего</div></div>\
         </div>\
         <div class=\"rec-card\">\
         <div class=\"rec-label\">\
         <svg width=\"14\" height=\"14\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\">\
         <path d=\"M12 3l2.7 5.6 6.1.8-4.5 4.2 1.1 6-5.4-3-5.4 3 1.1-6L3.2 9.4l6.1-.8L12 3z\"/></svg>\
         РЕКОМЕНДАЦИЯ</div>\
         <div class=\"rec-title\">{rec_title}</div>\
         <div class=\"rec-notes\">{rec_notes}</div>\
         </div>\
          <h2>СХЕМЫ</h2>\
          {schemes}\
          {score}\
          {background}\
          {conditions}\
          {phases}\
          </div>\
          <div class=\"foot\">Сгенерировано {now_stamp} · PowerBench\
          <span class=\"prov\">план <span class=\"mono\">{plan}</span> · хэш \
          <span class=\"mono\">{hash}</span> · seed <span class=\"mono\">{seed}</span>\
          · {machine}</span></div>\
         </div><button type=\"button\" class=\"to-top\" aria-label=\"Наверх\">↑</button>\
         {js}</body></html>",
        wl = esc(&id.workload_version),
        wc = id.worker_count,
        lcpus = id.logical_cpus,
        machine = esc(&machine_line(id)),
        day = esc(&pretty_dt(&stamp)),
        lead_value = lead_value,
        leader_name = esc(&lead_sub),
        margin = margin,
        level_label = esc(&rec.level_label),
        admitted = admitted,
        schemes_sub = esc(&schemes_sub),
        runs = total_runs,
        rec_title = rec_title,
        rec_notes = rec_notes_html,
        schemes = scheme_block(&format!(
            "<table><thead><tr><th>Схема</th><th class=\"num\">Прогонов</th>\
             <th class=\"num\">Медиана, тик/с</th><th class=\"num\">Отклонение</th>\
             <th class=\"num\">Стабильность</th><th>Статус</th></tr></thead>\
             <tbody>{rows}</tbody></table>",
            rows = sch_rows
        )),
        score = categories_section(s),
        background = background_section(s, "<h2>Фоновая нагрузка</h2>", ""),
        conditions = conditions_section(s),
        phases = phase_table_section(s),
        now_stamp = esc(&pretty_dt(&date_time_stamp(now_unix_ns()))),
        plan = esc(&s.plan_guid),
        hash = esc(&id.config_hash),
        seed = esc(&id.seed_hex),
        js = HTML_JS,
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
        for sch in s.schemes.iter().filter(|x| is_measured(x)) {
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
        // Сводка показывает лидеров; при 100+ схемах полная плитка на каждую
        // схему — это просто длинный список без пользы. Сколько скрыто, пишем
        // явно, чтобы отчёт не выглядел полнее, чем он есть.
        by_scheme.sort_by(|a, b| {
            (b.2 / b.1 as f64)
                .partial_cmp(&(a.2 / a.1 as f64))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let shown = by_scheme.len().min(SUMMARY_MAX_CARDS);
        let cards: String = by_scheme
            .iter()
            .take(SUMMARY_MAX_CARDS)
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
        let more = if by_scheme.len() > shown {
            format!(
                "<div class=\"note\">Показаны {shown} лучших из {total}. \
                 Остальные схемы — в таблицах сессий ниже.</div>",
                total = by_scheme.len()
            )
        } else {
            String::new()
        };
        format!("<div class=\"sumcards\">{cards}</div>{more}")
    };

    let identity_line = identity.unwrap_or_default();
    let empty_note = if count == 0 {
        "<div class=\"note\">История пуста — отчёт не содержит сессий.</div>".to_string()
    } else {
        String::new()
    };
    format!(
        "<header class=\"hero\"><div class=\"brand\"><i></i>PowerBench</div><h1>Отчёт по истории измерений</h1>\
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
    // При 100+ схемах график из сотни линий не читается: сначала выбираем
    // лидеров по средней производительности и рисуем только их. Остальные
    // считаем и упоминаем под графиком явно, а не прячем молча.
    let mut totals: Vec<(String, f64, usize)> = Vec::new();
    for s in sessions.iter() {
        for sch in s.schemes.iter().filter(|x| is_measured(x)) {
            let name = sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone());
            match totals.iter_mut().find(|(n, _, _)| *n == name) {
                Some(e) => {
                    e.1 += sch.median_throughput;
                    e.2 += 1;
                }
                None => totals.push((name, sch.median_throughput, 1)),
            }
        }
    }
    totals.sort_by(|a, b| {
        (b.1 / b.2 as f64)
            .partial_cmp(&(a.1 / a.2 as f64))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let omitted_series = totals.len().saturating_sub(CHART_MAX_SERIES);
    let top: Vec<String> = totals
        .iter()
        .take(CHART_MAX_SERIES)
        .map(|(n, _, _)| n.clone())
        .collect();

    let mut series: Vec<(String, Vec<(usize, f64)>)> = Vec::new();
    let mut all: Vec<f64> = Vec::new();
    for (col, s) in sessions.iter().enumerate() {
        for sch in s.schemes.iter().filter(|x| is_measured(x)) {
            let name = sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone());
            if !top.contains(&name) {
                continue;
            }
            match series.iter_mut().find(|(id, _)| *id == name) {
                Some((_, pts)) => pts.push((col, sch.median_throughput)),
                None => {
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
    // Защита от бесконечного цикла: при огромных/вырожденных значениях
    // (например, битая запись истории) `start` мог стать inf/NaN, а шаг —
    // слишком мелким, и `v += step` переставал менять v. Ограничиваем и
    // шаг, и число линий.
    let step = if step.is_finite() && step > 0.0 {
        step.max((y_max - y_min) / MAX_GRID_LINES as f64)
    } else {
        1.0
    };
    let start = (y_min / step).ceil() * step;
    let y = |v: f64| T + inner_h - ((v - y_min) / (y_max - y_min)) * inner_h;
    let x = |col: usize| L + (col as f64 + 0.5) * ((W - L - R) / n as f64);

    let mut gridlines = String::new();
    let mut v = start;
    let mut drawn = 0usize;
    while v <= y_max + 1e-9 && drawn < MAX_GRID_LINES {
        drawn += 1;
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
         <rect class=\"panel\" width=\"{W}\" height=\"{H}\" rx=\"8\"/>{legend}{gridlines}{xlabels}{shapes}</svg></div>{note}",
        note = if omitted_series > 0 {
            format!(
                "<div class=\"note\">На графике {shown} лидеров по средней производительности. \
                 Остальные {omitted_series} схем(ы) не показаны, чтобы линии не пересекались; \
                 полные данные — в таблицах ниже.</div>",
                shown = series.len(),
            )
        } else {
            String::new()
        }
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
        // Регресс H44: время бралось срезом `HHMMSS` и попадало в таблицу
        // сырой строкой — «221320» вместо «22:13». Секунды в сводке сессий не
        // нужны, а разделитель обязателен: метка времени в истории должна
        // читаться как время, а не как слипшиеся цифры.
        let time = time_hm(&stamp);
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
            "<tr class=\"sess-row\"><td class=\"num\">{day} {time}</td><td>{name_e}</td>\
             <td class=\"num\">{score}</td>\
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
        "<h2>Сессии ({n})</h2>\
         <p class=\"note\">Нажмите на строку сессии, чтобы раскрыть или скрыть её детали. \
         Без JavaScript все детали показаны сразу.</p>\
         <table><thead><tr><th>Дата (UTC)</th><th>Лучшая схема</th>\
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
        .and_then(|id| {
            s.schemes
                .iter()
                .find(|x| x.scheme_id.eq_ignore_ascii_case(id))
        });
    let tie = matches!(rec.level.as_str(), "Equivalent" | "KeepCurrent");
    let has_winner = !tie && winner.is_some();
    for sch in &s.schemes {
        let is_win = has_winner
            && winner
                .map(|w| w.scheme_id.eq_ignore_ascii_case(&sch.scheme_id))
                .unwrap_or(false);
        let row_status = if sch.rejected {
            "rejected"
        } else if is_win {
            "lead"
        } else if is_measured(sch) {
            "admitted"
        } else {
            "unmeasured"
        };
        let status = if sch.rejected {
            format!(
                "<span class=\"badge err\">{0}</span>",
                esc(sch.rejection_reason.as_deref().unwrap_or("забракована"))
            )
        } else if is_win {
            "<span class=\"badge ok\">рекомендована</span>".to_string()
        } else if is_measured(sch) {
            "<span class=\"badge\">допущена</span>".to_string()
        } else {
            "<span class=\"badge\" title=\"ни одного законченного прогона\">не измерена</span>"
                .to_string()
        };
        let name = sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone());
        sch_rows.push_str(&format!(
            "<tr{attrs}><td>{name}</td><td class=\"num\">{runs}</td><td class=\"num\">{median}</td>\
             <td class=\"num\">{cv}</td><td class=\"num\">{cons}</td><td class=\"num\">{worst}</td>\
             <td class=\"num\">{purity}</td><td class=\"num\">{mean}</td><td>{status}</td></tr>",
            attrs = scheme_row_attrs(&name, sch.runs, sch.median_throughput, row_status),
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
{schemes}\
<div class=\"rec\">{rec_line}</div>\
          <div class=\"note\">{reason}</div>{score}{background}{probs}{mode_html}</div></td></tr>",
        plan = esc(&s.plan_guid),
        stamp = esc(stamp),
        schemes = scheme_block(&format!(
            "<table class=\"sub\"><thead><tr><th>Схема</th><th class=\"num\">Прогоны</th>\
             <th class=\"num\">Медиана, тик/с</th><th class=\"num\">CV</th>\
             <th class=\"num\">стабильность</th><th class=\"num\">худш. окно, тик/с</th>\
             <th class=\"num\">Фон</th><th class=\"num\">Среднее</th><th>Статус</th></tr></thead>\
             <tbody>{sch_rows}</tbody></table>",
            sch_rows = sch_rows
        )),
        rec_line = rec_line,
        reason = esc(&rec.reason),
        score = categories_section(s),
        // Внутри раскрытой сессии заголовок уровнем ниже — `.dhead` и таблица
        // без рамки, как соседние блоки деталей.
        background = background_section(s, "<div class=\"dhead\">Фоновая нагрузка</div>", "sub",),
        probs = probs,
        mode_html = mode_html,
    )
}

/// Секция «Фоновая нагрузка»: какие процессы совпали с просадками throughput.
///
/// Раньше отчёт показывал только процент чистоты фона — число, по которому
/// непонятно, что именно мешало замеру. Сэмплер процессов уже собирает
/// коррелированные с провалами окна (`StoredRun::background`), но в отчёт они
/// не попадали, и пользователю оставалось гадать.
fn background_section(s: &SessionJson, heading: &str, table_class: &str) -> String {
    // Суммируем по паре (имя, путь): один и тот же процесс идёт в отчёт один
    // раз, даже если всплески были в прогонах разных схем.
    struct Acc {
        name: String,
        median_cpu: f64,
        peak_cpu: f64,
        windows: usize,
    }
    let mut acc: Vec<Acc> = Vec::new();
    for sch in &s.schemes {
        for run in &sch.per_run {
            for p in &run.background {
                // Ключ — только имя. Раньше добавлялся и путь, из-за чего один
                // и тот же процесс, запущенный из разных мест (например, из
                // системной папки и из каталога пользователя), попадал в отчёт
                // двумя строками.
                let key = p.name.trim();
                let hit = acc.iter_mut().find(|a| a.name.eq_ignore_ascii_case(key));
                match hit {
                    Some(a) => {
                        a.windows += p.correlated_spike_windows;
                        a.peak_cpu = a.peak_cpu.max(p.peak_cpu_percent);
                        a.median_cpu = a.median_cpu.max(p.median_cpu_percent);
                    }
                    None => acc.push(Acc {
                        name: key.to_string(),
                        median_cpu: p.median_cpu_percent,
                        peak_cpu: p.peak_cpu_percent,
                        windows: p.correlated_spike_windows,
                    }),
                }
            }
        }
    }
    // В списке только те, чья активность совпала с провалом throughput.
    acc.retain(|a| a.windows > 0);
    if acc.is_empty() {
        return String::new();
    }
    acc.sort_by(|a, b| {
        b.windows.cmp(&a.windows).then_with(|| {
            b.peak_cpu
                .partial_cmp(&a.peak_cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    // Только имя и процент нагрузки. Путь к исполняемому файлу, число
    // совпавших окон и список схем убраны: таблица из семи колонок
    // перегружала отчёт, а путь — это длинная строка, которая ломала
    // вёрстку и ничего не добавляла к пониманию причины.
    let mut rows = String::new();
    for a in &acc {
        rows.push_str(&format!(
            "<tr><td>{name}</td><td class=\"num\">{med}</td><td class=\"num\">{peak}</td></tr>",
            name = esc(&a.name),
            med = pct1(a.median_cpu),
            peak = pct1(a.peak_cpu),
        ));
    }
    format!(
        "{heading}\
         <table class=\"{cls}\"><thead><tr><th>Процесс</th>\
         <th class=\"num\">CPU, обычно</th><th class=\"num\">CPU, пик</th></tr></thead>\
         <tbody>{rows}</tbody></table>\
         <div class=\"note\">Только процессы, чья активность совпала с провалом \
         throughput. «Обычно» — медиана нагрузки за замер, «пик» — максимум. \
         Совпадение по времени не значит, что процесс вызвал провал.</div>",
        heading = heading,
        cls = table_class,
        rows = rows
    )
}
/// Пофазная таблица: медиана и P1 по каждой фазе плюс отметка снижения частоты.
///
/// Фазы различаются объёмом работы на тик, поэтому «тик/с» между ними
/// сравнивать нельзя: фаза с меньшей работой на тик даёт больше тиков в
/// секунду при меньшей реальной нагрузке. Сравнивать нужно *внутри* фазы — там
/// работа на тик одинакова, и разница схем видна честно. Именно поэтому
/// таблица показывает фазы рядом, а не прячет их за одним средним.
fn phase_table_section(s: &SessionJson) -> String {
    if s.schemes.iter().all(|x| x.phases.is_empty()) {
        return String::new();
    }
    // Фазы для шапки берутся объединением по всем схемам. Раньше шапка
    // строилась от первой схемы, а строки — от каждой своей, поэтому у схем с
    // разным набором фаз числа уезжали под чужие заголовки: над шапкой
    // «Лёгкая/Частичная» оказывалось шестнадцать ячеек.
    let mut phase_names: Vec<String> = Vec::new();
    for sch in &s.schemes {
        for p in &sch.phases {
            if !phase_names.iter().any(|n| n == &p.name) {
                phase_names.push(p.name.clone());
            }
        }
    }
    if phase_names.is_empty() {
        return String::new();
    }
    let mut rows = String::new();
    for sch in &s.schemes {
        if sch.phases.is_empty() {
            continue;
        }
        let cells: String = phase_names
            .iter()
            .map(|name| match sch.phases.iter().find(|p| &p.name == name) {
                Some(p) => format!(
                    "<td class=\"num\">{m:.1}</td><td class=\"num\">{p1:.1}</td>\
                         <td class=\"num\">{c:.1}</td><td class=\"num\">{f}</td>",
                    m = p.median_throughput,
                    p1 = p.p1_throughput,
                    c = p.consistency_percent,
                    f = frequency_cell(p)
                ),
                None => "<td class=\"num\">—</td><td class=\"num\">—</td>\
                         <td class=\"num\">—</td><td class=\"num\">—</td>"
                    .to_string(),
            })
            .collect();
        rows.push_str(&format!("<tr><td>{}</td>{cells}</tr>", esc(&name_of(sch))));
    }
    // Шапка в два уровня: «Схема» занимает обе строки, а под каждой фазой
    // повторяются четыре подписи. Без `rowspan` первая строка занимала одну
    // ячейку, вторая — четыре, и браузер развёл их по разным столбцам: на
    // скриншоте подписи «Медиана/P1/Стабильность/Частота» уезжали вправо и
    // накрывали соседние фазы.
    let head: String = phase_names
        .iter()
        .map(|n| format!("<th colspan=\"4\">{}</th>", esc(n)))
        .collect();
    let sub: String = phase_names
        .iter()
        .map(|_| {
            "<th class=\"num\">Медиана</th><th class=\"num\">P1</th>\
             <th class=\"num\">Стабильность</th><th class=\"num\">Частота</th>"
                .to_string()
        })
        .collect();
    format!(
        "<h2>Метрики по фазам</h2>\
         <div class=\"table-scroll\"><table class=\"phases\"><thead>\
         <tr><th rowspan=\"2\">Схема</th>{head}</tr><tr>{sub}</tr></thead>\
         <tbody>{rows}</tbody></table></div>\
         <div class=\"note\">Медиана и P1 — в тиках в секунду, стабильность — в \
         процентах, частота — в МГц. Стрелка ↓ означает, что в этой фазе \
         замечено снижение частоты относительно лучшей частоты этой же \
         сессии: часть фазы измерялась на пониженной частоте, и это не \
         заслуга схемы питания. Фазы отличаются по времени и нагрузке, поэтому \
         их числа нельзя складывать.</div>"
    )
}

/// Ячейка частоты фазы: сама частота, а при заметном падении — со стрелкой.
///
/// Отсчёт падения идёт от лучшей частоты сессии, а не от потолка Windows:
/// в потолок входят и штатный буст, и разгон в BIOS, поэтому на машине с
/// разгоном отсчёт от него давал бы срабатывание в каждой фазе.
fn frequency_cell(p: &crate::result::PhaseSummaryJson) -> String {
    if p.frequency_mhz <= 0.0 {
        return "—".to_string();
    }
    if p.frequency_dropped() {
        format!("{:.0} ↓{:.0}%", p.frequency_mhz, p.frequency_drop_percent)
    } else {
        format!("{:.0}", p.frequency_mhz)
    }
}
/// Секция «Условия замера»: опорная схема (дрейф машины), состояние питания и
/// фон — всё, что решает, можно ли вообще доверять ранжированию.
///
/// Раньше в отчёте не было ни одного из этих пунктов, и вердикт выглядел
/// уверенным даже тогда, когда машина грелась, фон шумел, а частоты упирались
/// в лимит мощности.
fn conditions_section(s: &SessionJson) -> String {
    let mut out = String::new();

    if s.screening {
        out.push_str("<h2>Условия замера</h2>");
        out.push_str(
            "<div class=\"note warn\"><b>Скрининг.</b> На схему меньше прогонов, чем нужно \
             для доверительного интервала: ранжирование ориентировочное, вердикт \
             не выдаётся.</div>",
        );
    }
    if let Some(r) = &s.reference {
        let name = r.scheme_name.clone().unwrap_or_else(|| r.scheme_id.clone());
        let rows: String = r
            .per_round
            .iter()
            .map(|v| format!("<td class=\"num\">{v:.1}</td>"))
            .collect();
        let header: String = r
            .per_round
            .iter()
            .enumerate()
            .map(|(i, _)| format!("<th class=\"num\">{}</th>", i + 1))
            .collect();
        out.push_str(&format!(
            "<h2>Условия замера</h2>\
             <div class=\"sub\"><div class=\"dhead\">Опорная схема — дрейф машины</div>\
             <table class=\"sub\"><thead><tr><th>Опорная</th>{header}</tr></thead>\
             <tbody><tr><td>{name}</td>{rows}</tr></tbody></table>\
             <div class=\"note\">{note}{state}</div></div>",
            name = esc(&name),
            header = header,
            rows = rows,
            note = esc(&r.note()),
            state = if r.unstable {
                " Разброс выше порога: машина плавает сильнее, чем различаются \
                 схемы, поэтому вердикт понижен."
            } else {
                " Разброс в пределах нормы — ранжирование устойчиво."
            },
        ));
    }
    // Фон — это то, что мешает замеру; частота — то, что может урезать
    // результат. Обе величины собираются по всем прогонам сессии.
    let mut bg_p50: f64 = 0.0;
    let mut bg_p95: f64 = 0.0;
    let mut dropped: Vec<String> = Vec::new();
    let mut power: Vec<String> = Vec::new();
    for sch in &s.schemes {
        for r in &sch.per_run {
            bg_p50 = bg_p50.max(r.background_cpu_p50);
            bg_p95 = bg_p95.max(r.background_cpu_p95);
            let hits = sch.phases.iter().filter(|p| p.frequency_dropped()).count();
            if hits > 0 {
                // Экранируем здесь, а не на выводе: строки собираются в
                // список и уходят в разметку через `join`, где забыть про
                // экранирование легко (регресс H45). Имя схемы приходит из
                // `powercfg /list` и может содержать теги.
                let name = esc(&sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone()));
                let entry = format!("{name} ({hits})");
                if !dropped.contains(&entry) {
                    dropped.push(entry);
                }
            }
            if let Some(p) = r.power.as_ref().and_then(|p| p.note()) {
                let entry = format!("{}: {}", esc(&name_of(sch)), esc(&p));
                if !power.contains(&entry) {
                    power.push(entry);
                }
            }
        }
    }
    // Фон печатается, если машина была занята; снижение частоты — если оно
    // замечено. Это две разные вещи, и сводная строка перечисляет обе.
    if bg_p95 > 0.0 || !power.is_empty() || !dropped.is_empty() {
        out.push_str("<h2>Условия замера</h2>");
        out.push_str("<div class=\"note\"><b>Фон и частота во время замера:</b>");
        if bg_p95 > 0.0 {
            out.push_str(&format!(
                " фон до {:.0} % CPU (медиана {:.0} %, пик {:.0} % одного ядра)",
                bg_p95, bg_p50, bg_p95
            ));
        }
        if !power.is_empty() {
            out.push_str(&format!("; {}", power.join("; ")));
        }
        if !dropped.is_empty() {
            out.push_str(&format!(
                "; замечено снижение частоты: {}",
                dropped.join(", ")
            ));
        }
        out.push_str(".</div>");
    }
    // приложение и консоль показывали, и пользователь получал два разных
    // рассказа об одном замере.
    if !s.warnings.is_empty() {
        out.push_str("<h2>Предупреждения замера</h2><ul class=\"warn-list\">");
        for w in &s.warnings {
            out.push_str(&format!("<li>{}</li>", esc(w)));
        }
        out.push_str("</ul>");
    }
    if out.is_empty() { String::new() } else { out }
}

fn name_of(sch: &crate::result::SchemeJson) -> String {
    sch.name.clone().unwrap_or_else(|| sch.scheme_id.clone())
}

/// Расклад отчёта: две категории вместо «балла по вашим весам».
///
/// Балл со свободными весами конфликтовал с рекомендацией: рядом стояли
/// «лидер» по баллу и «рекомендована» по статистике, и два разных ответа на
/// вопрос «какая схема лучше» выглядели как противоречие. Теперь
/// показываются ровно те величины, по которым выбирается рекомендация:
/// средняя пропускная способность и худшая секунда вместе со стабильностью.
fn categories_section(s: &SessionJson) -> String {
    let measured: Vec<&crate::result::SchemeJson> =
        s.schemes.iter().filter(|x| is_measured(x)).collect();
    if measured.is_empty() {
        return String::new();
    }

    // «Самое высокое AVG» — по средней throughput за прогон.
    let best_avg = measured
        .iter()
        .map(|x| x.mean_average_throughput)
        .fold(f64::MIN, f64::max);
    let top_avg = measured.iter().copied().reduce(|a, b| {
        if b.mean_average_throughput > a.mean_average_throughput {
            b
        } else {
            a
        }
    });

    // «Самое стабильное» — высокий P1 (худшая секунда) и высокая стабильность.
    // Порядок именно такой: без высокого P1 отличная стабильность среднего
    // не спасает от провала на одной секунде.
    let top_stable = measured.iter().copied().reduce(|a, b| {
        let ka = (a.median_p1_throughput, a.median_consistency_percent);
        let kb = (b.median_p1_throughput, b.median_consistency_percent);
        if kb > ka { b } else { a }
    });

    // Рекомендация: среди схем с высоким AVG берём самую стабильную.
    let avg_floor = best_avg * 0.98;
    let recommended = measured
        .iter()
        .copied()
        .filter(|x| x.mean_average_throughput >= avg_floor)
        .reduce(|a, b| {
            let ka = (a.median_p1_throughput, a.median_consistency_percent);
            let kb = (b.median_p1_throughput, b.median_consistency_percent);
            if kb > ka { b } else { a }
        })
        .or(top_stable);

    let card = |title: &str, hint: &str, sch: Option<&crate::result::SchemeJson>| -> String {
        let Some(sch) = sch else {
            return String::new();
        };
        format!(
            "<div class=\"catcard\"><div class=\"cat-title\">{title}</div>\
             <div class=\"cat-name\">{name}</div>\
             <div class=\"cat-metrics\">\
             <div><span>AVG</span><b>{avg:.0}</b> тик/с</div>\
             <div><span>P1 (худшая секунда)</span><b>{p1:.0}</b> тик/с</div>\
             <div><span>Стабильность</span><b>{cons:.0}</b> %</div></div>\
             <div class=\"cat-hint\">{hint}</div></div>",
            title = esc(title),
            name = esc(&name_of(sch)),
            avg = sch.mean_average_throughput,
            p1 = sch.median_p1_throughput,
            cons = sch.median_consistency_percent,
            hint = esc(hint),
        )
    };

    let mut cards = String::new();
    cards.push_str(&card(
        "Самое высокое AVG",
        "Наивысшая средняя пропускная способность за прогон.",
        top_avg,
    ));
    cards.push_str(&card(
        "Самое стабильное",
        "Наивысшая худшая секунда (P1) и наивысшая стабильность.",
        top_stable,
    ));

    let rec = match recommended {
        Some(r) => format!(
            "<div class=\"note ok\"><b>Рекомендуемая: {}</b> — высокий P1 среди схем \
             с высоким AVG (не ниже {:.0} % от лучшего) и лучшая стабильность.</div>",
            esc(&name_of(r)),
            98.0
        ),
        None => String::new(),
    };

    let early_note = s
        .early_stop_reason
        .as_ref()
        .map(|r| {
            format!(
                "<div class=\"note\"> Прервана досрочно ({rd}/{rp} раундов): {r}</div>",
                rd = s.rounds_completed,
                rp = s.rounds_planned,
                r = esc(r)
            )
        })
        .unwrap_or_default();

    format!(
        "<h2>Категории схем</h2>\
         <div class=\"cats\">{cards}</div>{rec}{early_note}"
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

/// Схема реально измерена: не забракована, есть законченный прогон и
/// конечная положительная медиана.
///
/// Всё, что не проходит этот фильтр, не должно попадать ни в счётчики
/// «допущено», ни в средние по истории: у схемы без прогонов нечем было
/// мериться, и её нулевая статистика портила разбивку на титуле.
fn is_measured(sch: &crate::result::SchemeJson) -> bool {
    !sch.rejected
        && sch.runs > 0
        && sch.median_throughput.is_finite()
        && sch.median_throughput > 0.0
}

/// Лучшая (незабракованная) схема сессии по median throughput.
fn best_scheme(s: &SessionJson) -> Option<&crate::result::SchemeJson> {
    let accepted: Vec<&crate::result::SchemeJson> =
        s.schemes.iter().filter(|x| is_measured(x)).collect();
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
    // Сессия без прогонов не имеет времени старта: `date_time_stamp(0)` дал бы
    // 1970 год, и в сводке это выглядело бы как реальная дата.
    if min == 0 || max == 0 {
        return ("-".to_string(), "-".to_string());
    }
    (
        date_slice(&date_time_stamp(min), 6, 8),
        date_slice(&date_time_stamp(max), 6, 8),
    )
}

/// `27.09.2026 в 03:02` из UTC-метки `YYYYMMDDTHHMMSSZ###`.
fn pretty_dt(stamp: &str) -> String {
    if stamp.len() < 13 {
        return stamp.to_string();
    }
    format!(
        "{}.{}.{} в {}:{}",
        &stamp[6..8],
        &stamp[4..6],
        &stamp[0..4],
        &stamp[9..11],
        &stamp[11..13]
    )
}

/// `дд.мм.гггг` из UTC-метки `YYYYMMDDTHHMMSSZ###`.
fn date_slice(stamp: &str, day0: usize, day1: usize) -> String {
    if stamp.len() < 8 {
        return stamp.to_string();
    }
    format!("{}.{}.{}", &stamp[day0..day1], &stamp[4..6], &stamp[0..4])
}

/// `ЧЧ:ММ` из UTC-метки `YYYYMMDDTHHMMSSZ###`.
///
/// Регресс H44. Отдельная функция, потому что срез времени использовался в
/// двух местах, и в одном из них разделитель забыли: в таблице истории
/// сессий выводилось `221320` вместо `22:13`. Метки короче 13 байт
/// (неполная дата) возвращаются как есть, а не режутся по границе UTF-8.
fn time_hm(stamp: &str) -> String {
    if stamp.len() < 13 {
        return stamp.to_string();
    }
    format!("{}:{}", &stamp[9..11], &stamp[11..13])
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
                os_build: String::new(),
                memory_gib: 0.0,
                cpu_brand: String::new(),
                affinity_mode: "p-only".into(),
                affinity_signature: "p-only:test".into(),
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
            reference: None,
            screening: false,
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

    /// Пишет отчёт в файл и возвращает его содержимое — для проверки тем же
    /// кодом, каким пользуется приложение (а не литералом в тесте).
    fn render_to_file(html: &str, tag: &str) -> (std::path::PathBuf, String) {
        let path = std::env::temp_dir().join(format!("powerbench-report-{tag}.html"));
        std::fs::write(&path, html).expect("отчёт должен записываться");
        let read = std::fs::read_to_string(&path).expect("отчёт должен читаться");
        (path, read)
    }

    /// Схема без законченных прогонов не должна попадать в счётчик
    /// «допущено» на титуле: раньше она выглядела как равная остальным.
    #[test]
    fn scheme_without_runs_is_not_counted_as_admitted() {
        let mut s = sample_session("AAA", 500.0);
        let mut skipped = SchemeJson::from_aggregate(
            "SKIPPED".to_string(),
            false,
            None,
            &empty_aggregate(0.0),
            Vec::new(),
        );
        skipped.name = Some("Схема без замеров".into());
        // Обнуляем явно, чтобы тест повторял ровно случай «ни одного
        // прогона», а не «нулевые метрики при одном прогоне».
        skipped.runs = 0;
        skipped.median_throughput = 0.0;
        s.schemes.push(skipped);

        assert!(is_measured(&s.schemes[0]));
        assert!(!is_measured(&s.schemes[1]));

        let html = build_report(std::slice::from_ref(&s));
        assert!(
            html.contains("не измерена"),
            "схема без прогонов обязана быть помечена как неизмеренная"
        );
        // В отчёте по одной сессии счётчик на титуле обязан называть их
        // отдельно, иначе «допущено» читается как «измерено и прошло».
        let session_html = build_session_report_verbose(&s);
        assert!(
            session_html.contains("без замеров: 1"),
            "счётчик на титуле должен отдельно считать неизмеренные схемы"
        );
    }

    /// Файл отчёта на диске — то же самое, что открывает браузер, поэтому
    /// проверяем именно его: кодировку, самодостаточность и навигацию.
    #[test]
    fn report_file_on_disk_is_self_contained() {
        let a = sample_session("AAA", 500.0);
        let b = sample_session("BBB", 520.0);
        let (path, html) = render_to_file(&build_report(&[a, b]), "history");
        // Без внешних ресурсов: иначе отчёт не откроется офлайн.
        assert!(!html.contains("<link "), "внешняя таблица стилей");
        assert!(!html.contains("src=\"http"), "внешний скрипт");
        assert!(html.contains("<style>"), "нет встроенных стилей");
        assert!(html.contains("<script>"), "нет встроенного скрипта");
        // Навигация по длинному списку схем.
        assert!(html.contains("data-schemes"), "нет контейнера навигации");
        assert!(html.contains("sch-search"), "нет поиска");
        assert!(html.contains("Показано"), "нет счётчика строк");
        let _ = std::fs::remove_file(&path);
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

    /// Регресс: огромные значения в истории вешали генератор отчёта
    /// (бесконечный цикл построения сетки).
    #[test]
    fn report_survives_absurd_throughput() {
        let a = sample_session("AAA", 1e300);
        let b = sample_session("BBB", 5e299);
        let html = build_report(&[a, b]);
        assert!(html.ends_with("</body></html>"));
        let lines = html.matches("<line class=\"grid\"").count();
        assert!(lines <= MAX_GRID_LINES + 2, "слишком много линий: {lines}");
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

    /// Сессия со 120 схемами: отчёт должен собраться целиком, все строки
    /// попасть в таблицу, а навигация (поиск/фильтр/сортировка/кнопка
    /// «показать все») — присутствовать, иначе список превращается в стену.
    #[test]
    fn session_report_scales_to_many_schemes() {
        let mut s = sample_session("LEAD", 900.0);
        s.schemes = (0..120)
            .map(|i| {
                let id = format!("S{i:03}");
                let rejected = i % 7 == 0;
                let mut sch = SchemeJson::from_aggregate(
                    id.clone(),
                    rejected,
                    if rejected {
                        Some("Unstable".into())
                    } else {
                        None
                    },
                    &empty_aggregate(100.0 + i as f64),
                    Vec::new(),
                );
                sch.name = Some(format!("Схема питания «{id}»"));
                sch
            })
            .collect();
        s.recommendation.recommended_scheme = Some("LEAD".into());

        let html = build_session_report_verbose(&s);
        assert!(html.ends_with("</body></html>"));
        // Все 120 схем на месте: ничего не потеряно при генерации.
        assert_eq!(html.matches("data-name=").count(), 120, "потерялись строки");
        // Навигация для длинного списка на месте.
        assert!(html.contains("data-schemes"), "нет контейнера для поиска");
        assert!(html.contains("sch-search"), "нет поля поиска");
        assert!(html.contains("sch-status"), "нет фильтра по статусу");
        assert!(html.contains("sch-sort"), "нет сортировки");
        assert!(html.contains("sch-more"), " нет кнопки «показать все»");
        assert!(html.contains("Показано"), "нет счётчика показанных строк");
        assert!(html.contains("<script"), "нет скрипта навигации");
        // Каждая строка несёт данные для поиска/фильтра/сортировки.
        assert_eq!(html.matches("data-status=").count(), 120);
        assert_eq!(html.matches("data-median=").count(), 120);
        assert_eq!(html.matches("data-runs=").count(), 120);
    }

    /// Сводный отчёт по истории: строки сессий сворачиваются (иначе 50 сессий
    /// с деталями — это десятки тысяч строк), при этом данные остаются в DOM.
    #[test]
    fn history_report_collapses_session_details() {
        let sessions: Vec<SessionJson> = (0..12)
            .map(|i| sample_session(&format!("S{i:02}"), 400.0 + i as f64 * 10.0))
            .collect();
        let html = build_report(&sessions);
        assert!(html.contains("Сессии (12)"));
        assert_eq!(html.matches("class=\"sess-row\"").count(), 12);
        // Каждая сессия сохраняет свои детали в DOM (скрывает их скрипт).
        assert_eq!(html.matches("class=\"detail\"").count(), 12);
        assert!(html.contains("class=\"to-top\""), "нет кнопки «наверх»");
        assert!(html.contains("initSessions"), "нет логики сворачивания");
    }

    /// Сводный отчёт: при большом числе схем график и сводка не превращаются
    /// в кашу, но отчёт честно сообщает, что часть схем не показана.
    #[test]
    fn history_report_caps_chart_and_summary() {
        let mut sessions = Vec::new();
        for t in 0..3 {
            let mut s = sample_session("LEAD", 900.0);
            s.schemes = (0..40)
                .map(|i| {
                    let id = format!("S{i:02}");
                    let mut sch = SchemeJson::from_aggregate(
                        id.clone(),
                        false,
                        None,
                        &empty_aggregate(100.0 + i as f64 + t as f64),
                        Vec::new(),
                    );
                    sch.name = Some(format!("План {id}"));
                    sch
                })
                .collect();
            sessions.push(s);
        }
        let html = build_report(&sessions);
        // Никаких 40 линий на графике: только лидеры.
        assert_eq!(
            html.matches("<path class=\"line\"").count(),
            CHART_MAX_SERIES,
            "на графике слишком много линий"
        );
        assert!(
            html.contains("не показаны"),
            "нет честной пометки о скрытых схемах"
        );
        // Плиток сводки не больше лимита.
        assert_eq!(html.matches("class=\"sumcard\"").count(), SUMMARY_MAX_CARDS);
        assert!(
            html.contains("Показаны"),
            "нет пометки о скрытых схемах в сводке"
        );
    }

    /// Регресс: палитра отчёта обязана совпадать с темой Graphite приложения.
    ///
    /// Раньше у отчёта были свои значения с сине-серым оттенком, и на фоне
    /// приложения он выглядел «синим». Тест сверяет ключевые токены напрямую.
    #[test]
    fn report_palette_matches_app_graphite_theme() {
        let html = build_session_report_verbose(&sample_session("AAA", 500.0));
        for token in [
            "--bg-0:#0F0F0F",
            "--bg-1:#1B1B1B",
            "--bg-2:#2B2B2B",
            "--bg-3:#303030",
            "--fg:#F3F3F3",
            "--fg-dim:#ECECEC",
            "--fg-mute:#7E7E7E",
            "--ok:#6fd0a0",
            "--warn:#e4b46f",
            "--err:#e57979",
        ] {
            assert!(html.contains(token), "в отчёте нет токена темы: {token}");
        }
        // Старые синеватые значения не должны вернуться.
        for stale in [
            "#0b0c0f", "#141519", "#1a1c22", "#23262e", "#333845", "#c9cdd6",
        ] {
            assert!(
                !html.contains(stale),
                "в отчёте остался старый синеватый цвет: {stale}"
            );
        }
    }

    /// Регресс: отчёт обязан называть процессы, совпавшие с просадками
    /// throughput. Раньше в нём был только процент чистоты фона, и по нему
    /// нельзя было понять, что именно мешало замеру.
    #[test]
    fn report_names_background_processes() {
        let mut s = sample_session("AAA", 500.0);
        let run = stored_run_with_background("aaa", "chrome.exe", 7);
        s.schemes[0].per_run = vec![run];
        let html = build_session_report_verbose(&s);
        assert!(
            html.contains("Фоновая нагрузка"),
            "в отчёте нет секции фона"
        );
        assert!(html.contains("chrome.exe"), "в отчёте нет имени процесса");
    }

    /// Процесс, который не совпал ни с одним окном просадки, в секции быть
    /// не должен: иначе отчёт пугает пользователя фоновыми процессами,
    /// которые на замер не повлияли.
    #[test]
    fn uncorrelated_process_is_not_listed() {
        let mut s = sample_session("AAA", 500.0);
        let mut run = stored_run_with_background("aaa", "chrome.exe", 0);
        run.background[0].correlated_spike_windows = 0;
        s.schemes[0].per_run = vec![run];
        let html = build_session_report_verbose(&s);
        assert!(
            !html.contains("Фоновая нагрузка"),
            "секция фона показана без единого совпадения"
        );
    }

    fn stored_run_with_background(
        scheme: &str,
        proc: &str,
        windows: usize,
    ) -> crate::checkpoint::StoredRun {
        use crate::checkpoint::{PhaseStats, StoredRun};
        use powerbench_windows::monitor::CorrelatedProcess;
        let zero: powerbench_metrics::run::RunStats =
            powerbench_metrics::run::run_stats(&[1.0; 4]).unwrap();
        StoredRun {
            key: format!("0:{scheme}"),
            round: 0,
            scheme_id: scheme.to_string(),
            scheme_name: Some(format!("План {scheme}")),
            started_at_ns: 0,
            duration_ms: 1000,
            ticks: 10,
            supercycles: 1,
            first_tick_checksums: [0; 4],
            run_checksums: [0; 4],
            phases: vec![PhaseStats {
                phase_index: 0,
                stats: zero,
                power: None,
                seconds: 4,
            }],
            combined: zero,
            cross_phase_consistency: 0.0,
            burst_retention_percent: 0.0,
            background: vec![CorrelatedProcess {
                name: proc.to_string(),
                path: format!(r"C:\Program Files\{proc}"),
                median_cpu_percent: 3.5,
                peak_cpu_percent: 41.0,
                correlated_spike_windows: windows,
                peak_memory_bytes: 1024,
                phases: vec!["Тяжёлая".to_string()],
            }],
            spike_windows: 8,
            power: None,
            scheme_dump: None,
            background_cpu_p50: 0.0,
            background_cpu_p95: 0.0,
            background_sample_seconds: 0,
            worker_count: 4,
            affinity_mode: "p-only".to_string(),
            affinity_signature: "p-only:test".to_string(),
            config_hash: "cfg-test".to_string(),
        }
    }

    /// Схема с заданными AVG, P1 и стабильностью — для проверки категорий.
    fn scheme_with(id: &str, avg: f64, p1: f64, consistency: f64) -> SchemeJson {
        let mut agg = empty_aggregate(avg);
        agg.median_p1_throughput = p1;
        agg.median_consistency_percent = consistency;
        let mut sch = SchemeJson::from_aggregate(id.to_string(), false, None, &agg, Vec::new());
        sch.name = Some(format!("План {id}"));
        sch
    }

    /// Раздел «Балл по вашим весам» заменён на три категории.
    ///
    /// Балл со свободными весами давал два разных ответа на вопрос «какая
    /// схема лучше»: «лидер» по весам и «рекомендована» по статистике стояли
    /// рядом и противоречили друг другу. Теперь категории показывают ровно те
    /// величины, по которым выбирается рекомендация.
    #[test]
    fn report_shows_three_categories_without_weighted_score() {
        let mut s = sample_session("AAA", 500.0);
        s.schemes = vec![
            // Самое высокое AVG, но худшая стабильность и худший P1.
            scheme_with("FAST", 600.0, 100.0, 80.0),
            // AVG почти такой же (в пределах 2 %), зато лучший P1 и стабильность.
            scheme_with("STEADY", 590.0, 200.0, 98.0),
        ];
        let html = build_session_report_verbose(&s);
        assert!(html.contains("Категории схем"), "нет раздела категорий");
        assert!(
            html.contains("Самое высокое AVG"),
            "нет категории по средней throughput"
        );
        assert!(
            html.contains("Самое стабильное"),
            "нет категории стабильности"
        );
        assert!(
            html.contains("Рекомендуемая: План STEADY"),
            "рекомендация не по P1 и стабильности"
        );
        assert!(
            !html.contains("Балл по вашим весам"),
            "старый раздел с баллом по весам остался в отчёте"
        );
        assert!(
            !html.contains("lead-score") && !html.contains("lead-bar"),
            "в отчёте остались элементы весового балла"
        );
    }

    /// «Самое стабильное» — это P1, а не среднее.
    ///
    /// Порядок именно такой: высокая стабильность среднего не спасает от
    /// провала на одной секунде, поэтому схема с худшим P1 не должна получать
    /// категорию стабильности, даже если её consistency выше.
    #[test]
    fn most_stable_category_prefers_p1_over_consistency() {
        let mut s = sample_session("AAA", 500.0);
        s.schemes = vec![
            scheme_with("MID", 500.0, 150.0, 99.0),
            scheme_with("P1BEST", 480.0, 220.0, 90.0),
        ];
        let html = build_session_report_verbose(&s);
        let section = html
            .split("Самое стабильное")
            .nth(1)
            .expect("нет категории стабильности");
        // Внутри карточки от подзаголовка до пояснения: там имя схемы.
        let end = section.find("cat-hint").unwrap_or(section.len());
        let card = &section[..end];
        assert!(
            card.contains("P1BEST"),
            "категория стабильности отдана схеме с худшим P1"
        );
    }

    /// Фоновый список: только имя и нагрузка, без пути.
    ///
    /// Путь к исполняемому файлу — длинная строка, которая ломала вёрстку и
    /// ничего не добавляла к пониманию причины. Плюс раньше ключом был
    /// `имя + путь`, из-за чего один процесс из разных мест давал две строки.
    #[test]
    fn background_table_shows_only_name_and_cpu() {
        let mut s = sample_session("AAA", 500.0);
        let mut run = stored_run_with_background("aaa", "svc.exe", 3);
        // Тот же процесс из другого места — должен слиться с первой строкой.
        let mut twin = run.clone();
        twin.background[0].path = r"C:\Users\test\svc.exe".to_string();
        run.background.push(twin.background[0].clone());
        s.schemes[0].per_run = vec![run];
        let html = build_session_report_verbose(&s);
        assert!(html.contains("svc.exe"), "процесс пропал из отчёта");
        assert!(
            !html.contains("Program Files"),
            "в отчёте остался путь к исполняемому файлу"
        );
        assert_eq!(
            html.matches("svc.exe</td>").count(),
            1,
            "один процесс из двух путей должен давать одну строку"
        );
        let section = html
            .split("CPU, обычно")
            .nth(1)
            .expect("нет колонки с нагрузкой");
        assert!(
            !section.contains("<th class=\"num\">Окна</th>"),
            "в фоновой таблице остался подсчёт окон"
        );
    }

    /// Шапка пофазной таблицы не разъезжается и не обрезается.
    ///
    /// Два дефекта смотрелись на скриншоте как один. Первый: «Схема» занимала
    /// одну ячейку в первой строке, а четыре подписи — вторую, поэтому браузер
    /// разводил их по разным столбцам. Второй: при разном наборе фаз у схем
    /// тело шире шапки, и таблица вылезала за край страницы.
    #[test]
    fn phase_table_header_is_two_level_and_scrollable() {
        let mut s = sample_session("AAA", 500.0);
        // У первой схемы четыре фазы, у второй — две: наборы не совпадают.
        s.schemes = vec![
            {
                let mut a = scheme_with("AAA", 500.0, 100.0, 90.0);
                a.phases = (0..4)
                    .map(|i| crate::result::PhaseSummaryJson {
                        name: format!("Фаза {i}"),
                        median_throughput: 500.0,
                        p1_throughput: 100.0,
                        consistency_percent: 90.0,
                        frequency_drop_percent: if i == 2 { 6.0 } else { 0.0 },
                        frequency_mhz: if i == 2 { 4400.0 } else { 5200.0 },
                        seconds: 10,
                        samples_used: 3000,
                        samples_raw: 3157,
                        excluded_fraction: 0.05,
                    })
                    .collect();
                a
            },
            {
                let mut b = scheme_with("BBB", 400.0, 90.0, 88.0);
                b.phases = (0..2)
                    .map(|i| crate::result::PhaseSummaryJson {
                        name: format!("Фаза {i}"),
                        median_throughput: 400.0,
                        p1_throughput: 90.0,
                        consistency_percent: 88.0,
                        frequency_drop_percent: 0.0,
                        frequency_mhz: 5200.0,
                        seconds: 10,
                        samples_used: 3000,
                        samples_raw: 3157,
                        excluded_fraction: 0.05,
                    })
                    .collect();
                b
            },
        ];
        let html = build_session_report_verbose(&s);
        let section = html
            .split("Метрики по фазам")
            .nth(1)
            .expect("нет раздела фаз");
        // Обёртка со своей прокруткой: широкая таблица не должна растягивать
        // страницу и срезаться по краю окна.
        assert!(
            section.contains("<div class=\"table-scroll\">"),
            "нет обёртки с горизонтальной прокруткой"
        );
        // «Схема» на две строки, подписи повторены под каждой фазой.
        let head = section.split("</thead>").next().unwrap_or_default();
        assert!(
            head.contains("<th rowspan=\"2\">Схема</th>"),
            "заголовок «Схема» должен занимать обе строки шапки"
        );
        assert_eq!(
            head.matches("Медиана").count(),
            4,
            "подписи метрик должны повторяться под каждой фазой, а не один раз"
        );
        // Набор фаз у схем разный, но колонки должны совпадать: недостающие
        // фазы заполняются прочерком, а не просто исчезают, иначе строка
        // снова съезжает под чужие заголовки.
        let b_row = section
            .split("<td>План BBB</td>")
            .nth(1)
            .and_then(|rest| rest.split("</tr>").next())
            .expect("нет строки схемы BBB");
        let b_cells = b_row.matches("<td").count();
        assert_eq!(
            b_cells, 16,
            "у схемы с двумя фазами из четырёх должно быть 16 ячеек \
             (4 группы × 4 метрики), а не {b_cells}"
        );
        assert_eq!(
            b_row.matches("—").count(),
            8,
            "две недостающие фазы должны дать по четыре прочерка"
        );
        // Пометка падения частоты — в самой таблице, а не только в тексте.
        assert!(
            section.contains("↓6%"),
            "в таблице нет отметки падения частоты"
        );
    }

    /// Кнопка «Показать все» обязана переключать состояние.
    ///
    /// Раньше обработчик читал `more.checked` у элемента `<button>`, у которого
    /// такого свойства нет: значение всегда `undefined`, кнопка ничего не
    /// делала, а схемы после шестидесятой оставались скрыты навсегда.
    #[test]
    fn show_all_button_uses_state_not_checked_property() {
        let mut s = sample_session("AAA", 500.0);
        s.schemes = (0..120)
            .map(|i| scheme_with(&format!("S{i}"), 500.0 - f64::from(i), 100.0, 90.0))
            .collect();
        let html = build_session_report_verbose(&s);
        assert!(
            !html.contains("more.checked"),
            "кнопка снова читает checked у <button>"
        );
        assert!(
            html.contains("showAll = !showAll"),
            "нет переключателя состояния кнопки"
        );
    }

    /// Дрейф по опорной схеме: разброс считается по раундам, вердикт при
    /// превышении порога понижается, а при одном замере оценки не выдумывается.
    /// Дрейф: накопленное изменение по тренду, а не размах.
    ///
    /// Размах растёт с числом раундов даже на стабильной машине (ожидаемо
    /// около 2.33σ при пяти раундах), поэтому решение принимается по тренду.
    /// Монотонное падение на 8 % за три раунда — дрейф, а «плавание» вверх-вниз
    /// с тем же размахом — нет.
    #[test]
    fn reference_drift_marks_unstable_and_needs_two_rounds() {
        use crate::result::ReferenceSummary;
        let r = ReferenceSummary::build("g", Some("схема".into()), vec![100.0, 96.0, 92.0], 1.5)
            .expect("два раунда минимум");
        assert!(r.unstable, "монотонное падение на 8 % — это дрейф");
        assert!(r.note().contains("размах"));

        // Размах большой, но тренда нет: машина гуляла, а не уплывала.
        let wander =
            ReferenceSummary::build("g", Some("схема".into()), vec![92.0, 100.0, 92.0], 1.5)
                .expect("два раунда минимум");
        assert!(
            !wander.unstable,
            "плавание вверх-внир не должно понижать вердикт: {}",
            wander.note()
        );

        // Один раунд сравнивать не с чем.
        assert!(ReferenceSummary::build("g", Some("схема".into()), vec![100.0], 1.5).is_none());
    }

    /// Пофазная таблица обязана быть в отчёте: без неё вердикт по смешанному
    /// среднему скрывает случай «выиграл в одной фазе, проиграл в другой».
    /// Предупреждения сессии обязаны попадать в отчёт.
    ///
    /// Их показывали приложение и консоль, а HTML-отчёт молчал: у одного
    /// замера получалось два разных рассказа, и по отчёту нельзя было понять,
    /// что машина была загружена или схема попала в карантин.
    #[test]
    fn report_lists_session_warnings() {
        let mut s = sample_session("AAA", 500.0);
        s.warnings = vec![
            "фон на загруженной машине: до 90 % CPU (пиковое, 95-й перцентиль прогона)".to_string(),
            "замечено снижение частоты: AAA (1 фаз)".to_string(),
        ];
        let html = build_session_report_verbose(&s);
        assert!(
            html.contains("Предупреждения замера"),
            "в отчёте нет раздела предупреждений"
        );
        assert!(
            html.contains("замечено снижение частоты"),
            "предупреждение о снижении частоты потерялось"
        );
        // Слово «троттлинг» в отчёте не используется: оно означало сравнение с
        // потолком Windows, а не измеренное падение частоты, и вводило в
        // заблуждение на машинах с разгоном в BIOS.
        assert!(
            !html.to_lowercase().contains("троттлинг"),
            "в отчёте осталось слово «троттлинг»"
        );
        // Событие с экранируемыми символами не должно ломать разметку.
        s.warnings = vec!["<script>alert(1)</script>".to_string()];
        let html2 = build_session_report_verbose(&s);
        assert!(
            !html2.contains("<script>alert(1)</script>"),
            "предупреждение не экранировано"
        );
    }

    /// Условия замера печатаются и при одном замеченном снижении частоты.
    ///
    /// Блок условий выводился только при фоне или снимке питания, и падение
    /// частоты в отдельной фазе оставалось в отчёте незамеченным.
    #[test]
    fn report_shows_frequency_drop_even_without_background_noise() {
        let mut s = sample_session("AAA", 500.0);
        let mut run = stored_run_with_background("aaa", "svc.exe", 3);
        run.background_cpu_p50 = 0.0;
        run.background_cpu_p95 = 0.0;
        run.background_sample_seconds = 0;
        run.phases = vec![crate::checkpoint::PhaseStats {
            phase_index: 2,
            stats: powerbench_metrics::run::run_stats(&[1.0; 4]).expect("эталон"),
            power: Some(crate::checkpoint::PowerSnapshot {
                max_mhz: 0,
                current_mhz: 0,
                throttled: true,
                thermal_throttle: false,
                policy_reason: 0,
                max_idle_minutes: 0,
                on_ac: true,
                unavailable: true,
            }),
            seconds: 4,
        }];
        s.schemes[0].per_run = vec![run];
        // Падение задаётся в сводке фаз, а не во флаге снимка питания.
        s.schemes[0].phases = vec![crate::result::PhaseSummaryJson {
            name: "Частичная".to_string(),
            median_throughput: 500.0,
            p1_throughput: 100.0,
            consistency_percent: 90.0,
            frequency_drop_percent: 6.0,
            frequency_mhz: 4400.0,
            seconds: 10,
            samples_used: 3000,
            samples_raw: 3157,
            excluded_fraction: 0.05,
        }];
        let html = build_session_report_verbose(&s);
        assert!(
            html.contains("замечено снижение частоты"),
            "снижение частоты не попало в условия замера"
        );
    }

    #[test]
    fn report_has_phase_table() {
        let mut s = sample_session("AAA", 500.0);
        let mut run = stored_run_with_background("aaa", "svc.exe", 3);
        let stats: powerbench_metrics::run::RunStats =
            powerbench_metrics::run::run_stats(&[1.0; 4]).expect("эталонная статистика");
        run.phases.push(crate::checkpoint::PhaseStats {
            phase_index: 0,
            stats,
            power: None,
            seconds: 4,
        });
        s.schemes[0].per_run = vec![run];
        s.schemes[0].phases = crate::result::SchemeJson::from_aggregate(
            "aaa".into(),
            false,
            None,
            &empty_aggregate(500.0),
            s.schemes[0].per_run.clone(),
        )
        .phases;
        let html = build_session_report_verbose(&s);
        assert!(html.contains("Метрики по фазам"), "нет таблицы фаз");
    }

    /// Шапка пофазной таблицы строится по фазам, а не по парам (схема, фаза).
    ///
    /// На одной схеме ошибка не видна: шапка и тело совпадали по ширине. При
    /// двух схемах шапка расползалась на восемь столбцов, браузер растягивал
    /// таблицу, а числа уезжали в первые четыре заголовка — ровно то, ради чего
    /// таблица и нужна («выиграл в одной фазе, проиграл в другой»).
    #[test]
    fn phase_table_header_width_matches_body() {
        let mut s = sample_session("AAA", 500.0);
        s.schemes = vec![
            s.schemes[0].clone(),
            crate::result::SchemeJson::from_aggregate(
                "BBB".into(),
                false,
                None,
                &empty_aggregate(400.0),
                Vec::new(),
            ),
        ];
        for i in 0u8..4 {
            s.schemes[0].phases.push(crate::result::PhaseSummaryJson {
                name: format!("Фаза {i}"),
                median_throughput: 500.0 - 10.0 * f64::from(i),
                p1_throughput: 100.0,
                consistency_percent: 90.0,
                frequency_drop_percent: 0.0,
                frequency_mhz: 5200.0,
                seconds: 10,
                samples_used: 3000,
                samples_raw: 3157,
                excluded_fraction: 0.05,
            });
            s.schemes[1].phases.push(crate::result::PhaseSummaryJson {
                name: format!("Фаза {i}"),
                median_throughput: 400.0 - 10.0 * f64::from(i),
                p1_throughput: 90.0,
                consistency_percent: 88.0,
                frequency_drop_percent: 0.0,
                frequency_mhz: 5200.0,
                seconds: 10,
                samples_used: 3000,
                samples_raw: 3157,
                excluded_fraction: 0.05,
            });
        }
        let html = build_session_report_verbose(&s);
        // Считаем `colspan` внутри самого раздела фаз: таблица печатается в
        // отчёте дважды (сводка и приложение), и общий подсчёт дал бы 8 даже
        // при правильной шапке из четырёх групп.
        let section = html
            .split("Метрики по фазам")
            .nth(1)
            .expect("нет раздела фаз");
        let head_row = section.split("</tr>").next().unwrap_or_default();
        let headers = head_row.matches("colspan=\"4\"").count();
        assert_eq!(
            headers, 4,
            "в шапке {headers} групп фаз вместо 4: на каждую схему фазы удвоились"
        );
        assert!(
            head_row.contains("Фаза 0") && head_row.contains("Фаза 3"),
            "шапка не содержит всех названий фаз"
        );
        // В шапке не должно быть повторов названий фаз от разных схем.
        assert!(
            !head_row.contains("Фаза 0\" class=\"num\" colspan=\"4\">Фаза 0"),
            "в шапке продублированы названия фаз"
        );
    }

    /// Отчёт обязан называть условия замера: без них «уверенный» вердикт
    /// выглядит так же, как вердикт на зашумлённой машине.
    #[test]
    fn report_shows_measurement_conditions() {
        let mut s = sample_session("AAA", 500.0);
        let mut run = stored_run_with_background("aaa", "svc.exe", 5);
        run.background_cpu_p95 = 44.0;
        run.background_cpu_p50 = 12.0;
        run.background_sample_seconds = 20;
        s.schemes[0].per_run = vec![run];
        let html = build_session_report_verbose(&s);
        assert!(html.contains("Условия замера"), "нет секции условий");
        assert!(html.contains("44"), "не показан пик фоновой нагрузки");
    }

    /// Скрининг (один прогон на схему) должен быть назван скринингом, а не
    /// «Предварительно»: обе подписи звучат уверенно, а обещают разные вещи.
    #[test]
    fn screening_session_is_labelled_as_such() {
        let mut s = sample_session("AAA", 500.0);
        s.screening = true;
        let html = build_session_report_verbose(&s);
        assert!(html.contains("Скрининг"), "скрининг не помечен в отчёте");
    }

    /// Регресс: палитра графика не должна содержать синих тонов — отчёт
    /// обязан совпадать с графитовой темой приложения.
    #[test]
    fn chart_palette_has_no_blue() {
        for c in COLORS {
            let ch = |s: &str| u8::from_str_radix(s, 16).unwrap() as f64;
            let (r, g, b) = (ch(&c[1..3]), ch(&c[3..5]), ch(&c[5..7]));
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let d = max - min;
            // Оттенок в HSL: 0° красный, 120° зелёный, 210–260° синий/фиолетовый.
            let hue = if d == 0.0 {
                0.0
            } else if max == r {
                60.0 * (((g - b) / d) % 6.0)
            } else if max == g {
                60.0 * ((b - r) / d + 2.0)
            } else {
                60.0 * ((r - g) / d + 4.0)
            };
            let hue = if hue < 0.0 { hue + 360.0 } else { hue };
            assert!(
                !(200.0..=265.0).contains(&hue),
                "цвет {c} выглядит синим (hue={hue:.0}°)"
            );
        }
    }

    /// Отчёт должен открываться в браузере как автономный файл: без внешних
    /// ресурсов, без незакрытых тегов и с работающей навигацией по длинному
    /// списку. Здесь проверяется структура, сам браузер запускает пользователь.
    #[test]
    fn report_is_self_contained_and_well_formed() {
        let mut s = sample_session("LEAD", 900.0);
        s.schemes = (0..130)
            .map(|i| {
                let id = format!("S{i:03}");
                let mut sch = SchemeJson::from_aggregate(
                    id.clone(),
                    i % 9 == 0,
                    None,
                    &empty_aggregate(100.0 + i as f64),
                    Vec::new(),
                );
                sch.name = Some(format!("Схема «{id}» — довольно длинное имя плана"));
                sch
            })
            .collect();
        let html = build_session_report_verbose(&s);

        // Автономность: только встроенные стили и скрипт, никаких ссылок наружу.
        for pattern in ["<link ", "src=\"http", "href=\"http", "@import"] {
            assert!(!html.contains(pattern), "внешний ресурс: {pattern}");
        }
        // Основные контейнеры закрыты.
        for (open, close) in [
            ("<html", "</html>"),
            ("<body", "</body>"),
            ("<table", "</table>"),
        ] {
            assert_eq!(
                html.matches(open).count(),
                html.matches(close).count(),
                "несбалансированный тег {open}"
            );
        }
        // Скрипт и стили на месте — иначе навигация по 130 схемам не появится.
        assert!(html.contains("<style>"), "нет встроенных стилей");
        assert!(html.contains("<script>"), "нет встроенного скрипта");
        assert!(html.contains("data-schemes"), "нет контейнера навигации");
        // Таблица большая, но ограничение страницы найдено: 130 строк
        // отдаются сразу в DOM, а порция на экране отсекается скриптом.
        assert!(html.contains("Показать все"), "нет кнопки показать всё");
        assert!(html.contains("PAGE = 60"), "нет порции на экран");
    }

    /// Имена схем приходят извне: спецсимволы не должны ломать разметку.
    #[test]
    fn scheme_names_are_escaped_in_toolbar_data() {
        let mut s = sample_session("LEAD", 900.0);
        let mut sch = SchemeJson::from_aggregate(
            "X".into(),
            false,
            None,
            &empty_aggregate(500.0),
            Vec::new(),
        );
        sch.name = Some("План \"><script>alert(1)</script>".into());
        s.schemes.push(sch);
        s.recommendation.recommended_scheme = Some("LEAD".into());
        let html = build_session_report_verbose(&s);
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    /// Регресс H45: имя схемы в блоке «Условия замера» тоже экранируется.
    ///
    /// Эти строки собираются в список и уходят в разметку через `join`, где
    /// `esc` легко забыть. Имя схемы приходит из `powercfg /list`, то есть это
    /// данные извне: без экранирования отчёт выполнял бы HTML из имени
    /// пользовательского плана питания.
    #[test]
    fn scheme_names_are_escaped_in_the_measurement_conditions_block() {
        let mut s = sample_session("LEAD", 900.0);
        let injection = "<img src=x onerror=alert(1)>";
        let mut sch = SchemeJson::from_aggregate(
            "COND".into(),
            false,
            None,
            &empty_aggregate(500.0),
            Vec::new(),
        );
        sch.name = Some(injection.to_string());
        // Снижение частоты обязано попасть в этот блок.
        sch.phases = vec![crate::result::PhaseSummaryJson {
            name: "Лёгкая фаза".into(),
            median_throughput: 500.0,
            p1_throughput: 500.0,
            consistency_percent: 90.0,
            frequency_drop_percent: 55.0,
            ..Default::default()
        }];
        s.schemes.push(sch);
        let html = build_session_report_verbose(&s);
        assert!(
            !html.contains(injection),
            "имя схемы попало в разметку без экранирования (HTML-инъекция)"
        );
        assert!(
            html.contains("&lt;img src=x onerror=alert(1)&gt;"),
            "имя схемы должно быть видно как текст"
        );
    }

    /// Регресс H44: время в таблице истории — `ЧЧ:ММ`, а не слипшиеся `ЧЧММСС`.
    #[test]
    fn history_time_label_is_formatted_not_raw() {
        // Метка ровно того вида, который даёт `date_time_stamp`.
        let stamp = "20260927T221320Z000";
        assert_eq!(time_hm(stamp), "22:13");
        assert_eq!(
            &stamp[9..15], "221320",
            "тест должен воспроизводить исходный дефект"
        );
        // Короткая (неполная) метка не режется по границе UTF-8.
        assert_eq!(time_hm("2026"), "2026");
    }

    /// В таблице сессий время выводится через `time_hm`, а не срезом `HHMMSS`.
    #[test]
    fn history_table_never_shows_a_raw_hhmmss_slice() {
        let src = include_str!("report.rs");
        let start = src
            .find("fn render_sessions(")
            .expect("не найден генератор таблицы истории");
        let body = &src[start..];
        let end = body[1..]
            .find("\n/// ")
            .map(|i| i + 1)
            .expect("не найден конец генератора");
        assert!(
            !body[..end].contains("9..15"),
            "сырой срез HHMMSS снова попал в отчёт: в истории будет «221320»"
        );
        assert!(
            src.contains("let time = time_hm(&stamp);"),
            "время в таблице истории обязано идти через time_hm"
        );
    }

    /// Регресс H48: рекомендация ищется среди схем регистронезависимо.
    ///
    /// `powercfg` отдаёт GUID в верхнем регистре, а чекпоинт может хранить его
    /// в нижнем. При точном `==` победитель в отчёте просто исчезал.
    #[test]
    fn winner_lookup_ignores_case() {
        let mut s = sample_session("LEAD", 900.0);
        let plain = build_session_report_verbose(&s);
        // Имя победителя обязано попасть в отчёт: при не найденной схеме
        // на его месте стоит прочерк.
        assert!(
            !plain.contains("Рекомендация: <b>-</b>"),
            "подготовка: победитель не найден даже при точном регистре"
        );
        // Рекомендация записана в другом регистре, чем идентификатор схемы.
        s.recommendation.recommended_scheme = Some("lead".to_string());
        let lower = build_session_report_verbose(&s);
        assert_eq!(
            lower, plain,
            "отчёт зависит от регистра GUID: схема с рекомендацией в другом регистре \
             выглядит иначе"
        );
        assert!(
            !lower.contains("Рекомендация: <b>-</b>"),
            "победитель потерян из-за разного регистра GUID"
        );
    }
}
