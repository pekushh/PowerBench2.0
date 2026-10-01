// Приёмка адаптива (ТЗ XIV.4): токены, раскладка и запреты на 13 ширинах окна.
//
// Меряется РЕАЛЬНЫЙ `app/build-ui/assets/index-*.css` на скелете разметки с теми
// же классами, что и приложение (`.shell > .body > .sidebar + .main > .page`).
// Само приложение в iframe не грузится: оно вызывает Tauri-команды, которых
// в браузере нет, и дерево не рендерится. Замер идёт по CSS — а это и есть
// источник истины для формул токенов.
//
// Запуск: `npm run check:adaptive` (после `npm run build`).
const { spawn } = require("child_process");
const http = require("http");
const path = require("path");
const fs = require("fs");

const APP = path.resolve(__dirname, "..");
const BUILD = path.join(APP, "build-ui");
const OUT = path.join(process.env.TEMP || process.env.TMPDIR || APP, "powerbench-adaptive");
const PORT = Number(process.env.PORT || 8731);
// Ссылка на собранный CSS известна до запуска стенда: она нужна обеим страницам.
const indexHtml = fs.readFileSync(path.join(BUILD, "index.html"), "utf8");
const cssHref = (indexHtml.match(/href="\/assets\/([^"]+\.css)"/) || [])[1];
if (!cssHref) {
  console.log("в build-ui/index.html нет ссылки на CSS: сначала соберите фронтенд (npm run build)");
  process.exit(2);
}
const WIDTHS = [1920, 1760, 1600, 1440, 1360, 1280, 1200, 1120, 1080, 1020, 960, 900, 860];
const DENSITIES = ["compact", "normal", "roomy"];
const TEXTS = ["s", "m", "l"];

// Нормы из ТЗ II.3 при k=1, kt=1.
const EXPECT = {
  1920: { "--pad-x": 40, "--fs-caps": 10.7, "--fs-meta": 12.5, "--fs-log": 13.8, "--fs-body": 14.5, "--fs-card": 16.6, "--fs-h1": 22.6, "--fs-num": 34, "--ctrl-h": 33.6, "--sp-3": 11.8, "--sp-4": 14.8, "--sp-5": 23.4, "--sp-6": 31.1, "--gap": 10.9, "--card-pad": 16 },
  1440: { "--pad-x": 37.4, "--fs-caps": 10.1, "--fs-meta": 11.8, "--fs-log": 12.9, "--fs-body": 13.6, "--fs-card": 15.4, "--fs-h1": 19.9, "--fs-num": 28.8, "--ctrl-h": 31.2, "--sp-3": 10.9, "--sp-4": 13.3, "--sp-5": 19.5, "--sp-6": 25.8, "--gap": 9.2, "--card-pad": 16 },
  1280: { "--pad-x": 33.3, "--fs-h1": 19, "--sp-4": 12.8, "--gap": 8.7 },
  1020: { "--fs-h1": 19, "--fs-num": 26, "--sp-4": 12.1 },
  860: { "--pad-x": 22.4, "--fs-h1": 19, "--fs-num": 26, "--sp-5": 16, "--gap": 8 },
};
// Допуск: округление таблицы ТЗ до десятых.
const TOL = 0.7;
const px = (v) => Math.round(parseFloat(v) * 100) / 100;

const get = (url) => new Promise((res, rej) => {
  http.get(url, (r) => { let d = ""; r.on("data", (c) => (d += c)); r.on("end", () => res(d)); }).on("error", rej);
});

// Изолированная страница для проверки вертикального центрирования: здесь в
// колонке либо короткий блок, либо длинный. На общей странице стенда колонка
// всегда длиннее окна, и центрирование нечего проверять.
const CENTER_PAGE = `<!DOCTYPE html><html lang="ru" data-theme="Graphite" data-mode="Dark" data-density="normal" data-text="m" data-motion="on">
<meta charset="utf-8"><title>Центрирование</title>
<link rel="stylesheet" href="/assets/${cssHref}">
<style>html,body{margin:0;height:100%}</style>
<body>
<div class="shell rail-collapsed">
  <div class="body">
    <aside class="sidebar collapsed"><nav class="sidebar-nav"><button class="nav-item active"><span class="nav-glyph"><svg viewBox="0 0 24 24"></svg></span><span class="nav-label">Бенчмарк</span></button></nav></aside>
    <main class="main">
      <div class="page" id="page">
        <div class="k-card" id="short"><div class="sc-title">Короткий экран</div></div>
        <div class="launch-cta-bar" id="cta"><div class="lcb-title">Расчётное время сессии</div><div class="launch-actions"><span class="btn btn-primary">Запустить</span></div></div>
        <div id="tall">${Array.from({ length: 60 }, (_, i) => `<div class="ph-item" style="margin-bottom:8px"><div class="phi-top"><span>Строка ${i + 1}</span><span class="phi-pct">0 с</span></div></div>`).join("")}</div>
      </div>
    </main>
  </div>
</div>
<script>
// Короткий экран должен встать по центру по вертикали, длинный — прижаться
// к верху, иначе верхняя его часть уходит за область прокрутки.
// Прячем именно display: у карточек задано display:flex авторским правилом,
// оно сильнее браузерного [hidden]{display:none}, и «скрытый» блок продолжал
// занимать место в раскладке.
window.show = (which) => {
  const set = (id, on) => { document.getElementById(id).style.display = on ? "" : "none"; };
  set("short", which === "short");
  set("cta", which === "short");
  set("tall", which === "tall");
};
// Ничего не должно сжиматься по высоте: именно этим «съедался» низ карточки на
// экране «Бенчмарк» (из-за flex: 1 1 auto у .wizard).
window.measureSquash = (root) => {
  const bad = [];
  for (const el of document.querySelectorAll(root + ", " + root + " > *")) {
    if (el.style.display === "none" || !el.offsetParent) continue;
    const r = el.getBoundingClientRect();
    if (r.height > 0 && el.scrollHeight > r.height + 1) {
      bad.push((el.id || el.className.split(" ")[0]) + " " + Math.round(r.height) + "<" + el.scrollHeight);
    }
  }
  return bad;
};
window.measureCentering = () => {
  const pageEl = document.getElementById("page");
  const main = document.querySelector(".main");
  const p = pageEl.getBoundingClientRect();
  const m = main.getBoundingClientRect();
  const kids = [...pageEl.children].filter((el) => el.style.display !== "none" && el.offsetParent);
  const r0 = kids[0].getBoundingClientRect();
  const rN = kids[kids.length - 1].getBoundingClientRect();
  return {
    // Центр всего содержимого, а не первой карточки: блоков на странице
    // несколько, и по одной карточке равенство не проверить.
    shortOffset: (r0.top + rN.bottom) / 2 - (p.top + p.height / 2),
    // Отступ от ВНУТРЕННЕЙ границы .main: у .main есть верхний отступ
    // --sp-5, и сравнивать с рамкой дало бы ложные 19–20 px.
    tallOffset: r0.top - (m.top + parseFloat(getComputedStyle(main).paddingTop)),
    pageScroll: pageEl.scrollHeight - pageEl.clientHeight,
  };
};
</script></body></html>`;

const SKELETON = `
<div class="shell rail-collapsed">
  <div class="titlebar">
    <div class="titlebar-brand"><button class="titlebar-btn menu"></button><span class="mark"></span><span>PowerBench</span></div>
    <span class="split"></span><span class="titlebar-section is-in">Готово</span>
    <div class="titlebar-chips"><span class="titlebar-chip on"><span class="dot-live"></span>Тихий режим: вкл</span><span class="titlebar-chip trunc" data-prio="1">Ryzen 9 7950X · 32 ГБ</span></div>
    <div class="titlebar-controls"><button class="titlebar-toggle" data-prio="2">Анимации</button><span class="window-group"><button class="titlebar-btn"></button><button class="titlebar-btn close"></button></span></div>
  </div>
  <div class="body">
    <aside class="sidebar collapsed" id="rail">
      <nav class="sidebar-nav">
        <button class="nav-item active"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Бенчмарк</span></button>
        <button class="nav-item"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Схемы питания</span></button>
        <button class="nav-item"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Результаты</span></button>
        <button class="nav-item"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Логи</span></button>
      </nav>
      <div class="sidebar-foot"><button class="nav-item"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Настройки</span></button></div>
    </aside>
    <main class="main">
      <div class="page benchmark-view">
        <div class="page-head"><h1 class="page-title">Бенчмарк</h1><div class="sub">режим и настройки</div></div>
        <div class="mode-grid"><div class="mode-card selected"><div class="mc-pill"><small>Повторов</small><b>5 раундов</b></div></div><div class="mode-card"><div class="mc-pill time"><small>Одна схема</small><b>≈ 7 мин 11 с</b></div></div></div>
        <div class="schemes-grid" id="schemes">${Array.from({ length: 12 }, (_, i) => `<div class="scheme-card"><div class="sc-name">Схема ${i + 1}</div></div>`).join("")}</div>
        <div class="kpi-strip" id="kpis"><div class="kpi-card"><span class="kpi-name">Время тика</span><span class="kpi-val">0.971<small>мс</small></span></div><div class="kpi-card"><span class="kpi-name">Тиков в фазе</span><span class="kpi-val">2 719</span></div><div class="kpi-card"><span class="kpi-name">Фон CPU</span><span class="kpi-val">2.1<small>%</small></span></div></div>
        <div class="ph-grid" id="phases"><div class="ph-item active"><div class="phi-top"><span>1. Лёгкая</span><span class="phi-pct">13 / 36 с</span></div><div class="phi-bar"><i style="width:38%"></i></div><span class="phi-frac">38%</span></div><div class="ph-item"><div class="phi-top"><span>2. Отклик</span><span class="phi-pct">30 с</span></div><div class="phi-bar"><i style="width:0%"></i></div><span class="phi-frac">0%</span></div></div>
        <div class="log-list" id="logs"><div class="log-row" data-level="success"><span class="l-time">14:19:02</span><span class="l-badge success">Успех</span><span class="l-msg">Запуск фазы «Лёгкая» на 30 с</span></div><div class="log-row" data-level="info"><span class="l-time">14:19:32</span><span class="l-badge info">Инфо</span><span class="l-msg">Схема применена</span></div></div>
      </div>
    </main>
  </div>
</div>`;

// Кастомные свойства в `getComputedStyle` остаются строками («clamp(…)»), поэтому
// каждое значение читается через РЕАЛЬНОЕ свойство: так браузер сам считает
// формулу. Иначе проверка сравнивала бы текст формулы с числом.
const PROBES = [
  ["--pad-x", "padding-left"], ["--sp-3", "margin-left"], ["--sp-4", "margin-right"],
  ["--sp-5", "padding-left"], ["--sp-6", "padding-right"], ["--gap", "column-gap"],
  ["--card-pad", "padding-top"],
  // Все кегли меряются через `font-size` — он гарантированно принимает
  // `calc(…)`/`clamp(…)` и разрешает их в пиксели.
  ["--fs-caps", "font-size"], ["--fs-meta", "font-size"], ["--fs-log", "font-size"],
  ["--fs-body", "font-size"], ["--fs-card", "font-size"], ["--fs-h1", "font-size"],
  ["--fs-num", "font-size"],
  ["--ctrl-h", "height"], ["--icon-btn", "width"], ["--badge-h", "min-height"],
  ["--topbar-h", "max-height"], ["--log-time", "min-width"],
];

async function main() {
  console.log("css сборки:", cssHref);

  const page = `<!DOCTYPE html><html lang="ru" data-theme="Graphite" data-mode="Dark" data-density="normal" data-text="m" data-motion="on">
<meta charset="utf-8"><title>Адаптив: стенд</title>
<link rel="stylesheet" href="/assets/${cssHref}">
<style>html,body{margin:0;height:100%}#probes{position:absolute;left:-9999px;top:0;line-height:1}#probes i{display:block;width:0;height:0;font-style:normal;line-height:1}</style>
<body>${SKELETON}
<div id="probes" aria-hidden="true">${PROBES.map(([tok, prop]) =>
  // Базовый `font-size` задаётся только тем пробникам, которые меряют не кегль:
  // иначе он перебил бы `font-size: var(--fs-*)` в том же списке.
  `<i data-tok="${tok}" style="${prop}: var(${tok});${prop === "font-size" ? "" : "font-size:16px;"}"></i>`).join("")}</div>
<script>
window.setAttrs = (d, t) => {
  const r = document.documentElement;
  r.dataset.density = d; r.dataset.text = t;
};
window.collapse = (on) => {
  document.getElementById("rail").classList.toggle("collapsed", on);
  document.querySelector(".shell").classList.toggle("rail-collapsed", on);
};
window.measure = () => {
  // Значения токенов — через реальные свойства пробников.
  const tokens = {};
  for (const el of document.querySelectorAll("#probes i")) {
    tokens[el.dataset.tok] = getComputedStyle(el).getPropertyValue(
      el.style.cssText.split(":")[0],
    ).trim();
  }
  const main = document.querySelector(".main");
  const pageEl = document.querySelector(".page");
  const item = document.querySelector("#rail .nav-item");
  const glyph = document.querySelector("#rail .nav-item .nav-glyph svg");
  const cs2 = (el) => (el ? getComputedStyle(el) : null);
  const cols = (sel) => {
    const el = document.querySelector(sel);
    if (!el) return null;
    return getComputedStyle(el).gridTemplateColumns.split(" ").filter(Boolean).length;
  };
  const shown = (sel) => {
    const el = document.querySelector(sel);
    return el ? getComputedStyle(el).display !== "none" : null;
  };
  // Поля вокруг колонки: симметрия относительно ОКНА требует, чтобы левое и
  // правое были равны. Рельса съедает слева, поэтому без компенсации колонка
  // смещена вправо.
  const railEl = document.querySelector(".sidebar");
  const bodyEl = document.querySelector(".body");
  const pr = pageEl.getBoundingClientRect();
  const rr = railEl.getBoundingClientRect();
  const br = bodyEl.getBoundingClientRect();
  const inner = window.innerWidth;
  const logRow = document.querySelector(".log-row");
  return {
    tokens,
    padLeft: cs2(main).paddingLeft,
    mainOverflowX: main.scrollWidth - main.clientWidth,
    bodyOverflowX: document.body.scrollWidth - document.body.clientWidth,
    column: pageEl.getBoundingClientRect().width,
    columnLeft: pageEl.getBoundingClientRect().left,
    gapLeft: pr.left,
    gapRight: inner - (pr.left + pr.width),
    railLeft: rr.left,
    railRight: rr.right,
    bodyLeft: br.left,
    bodyRight: br.right,
    mainRight: document.querySelector(".main").getBoundingClientRect().right,
    mainLeft: document.querySelector(".main").getBoundingClientRect().left,
    innerWidth: inner,
    navItem: cs2(item).width,
    navGlyph: cs2(glyph).width,
    railWidth: document.getElementById("rail").getBoundingClientRect().width,
    bodyFont: cs2(document.body).fontSize,
    schemesCols: cols("#schemes"),
    kpiCols: cols("#kpis"),
    phaseCols: cols("#phases"),
    prio1: shown('[data-prio="1"]'),
    prio2: shown('[data-prio="2"]'),
    logCols: logRow ? getComputedStyle(logRow).gridTemplateColumns.split(" ").filter(Boolean).length : null,
  };
};
</script></body></html>`;

  // Страница «Бенчмарк» по-настоящему: шапка сверху, блоки забирают высоту.
// Именно на ней ломались обрезанный низ и пустое место под блоками.
const BENCH_PAGE = `<!DOCTYPE html><html lang="ru" data-theme="Graphite" data-mode="Dark" data-density="normal" data-text="m" data-motion="on">
<meta charset="utf-8"><title>Бенчмарк: раскладка</title>
<link rel="stylesheet" href="/assets/${cssHref}">
<style>html,body{margin:0;height:100%}</style>
<body>
<div class="shell rail-collapsed">
  <div class="body">
    <aside class="sidebar collapsed"><nav class="sidebar-nav"><button class="nav-item active"><span class="nav-glyph"><svg viewBox="0 0 24 24"></svg></span><span class="nav-label">Бенчмарк</span></button></nav></aside>
    <main class="main">
      <div class="page-host on" id="host">
      <div class="page benchmark-view" id="page">
        <div class="wizard-header" id="hdr">
          <div class="wh-left"><h1 class="page-title">Бенчмарк</h1>
            <div class="stepper"><span class="step active">1 Режим и настройки</span><span class="step">2 Схемы питания</span><span class="step">3 Запуск</span></div>
          </div>
          <div class="wh-actions"><button class="btn btn-primary" type="button">Далее →</button></div>
        </div>
        <div class="wizard" id="wizard">
          <div class="wizard-stage centered" id="stage">
          <div class="mode-grid" id="modes">
            <div class="mode-card selected"><div class="mc-top"><div><div class="mc-title-row"><h2 class="mc-title">Быстро</h2><span class="mc-tag">Скрининг</span></div><p class="mc-desc">Быстрая проверка производительности системы с минимальным временем тестирования.</p></div><span class="mc-radio"></span></div><div class="mc-stats"><div class="mc-stat"><span class="mc-k">Повторов</span><span class="mc-v">1 раунд</span></div><div class="mc-stat"><span class="mc-k">Прогон</span><span class="mc-v">30 с / 3 с</span></div><div class="mc-stat"><span class="mc-k">Одна схема</span><span class="mc-v">≈ 47 с</span></div></div></div>
            <div class="mode-card"><div class="mc-top"><div><div class="mc-title-row"><h2 class="mc-title">Детально</h2><span class="mc-tag">Рекомендуется</span></div><p class="mc-desc">Расширенное тестирование для точного сравнения схем питания и стабильных результатов.</p></div><span class="mc-radio"></span></div><div class="mc-stats"><div class="mc-stat"><span class="mc-k">Повторов</span><span class="mc-v">5 раундов</span></div><div class="mc-stat"><span class="mc-k">Прогон</span><span class="mc-v">60 с / 8 с</span></div><div class="mc-stat"><span class="mc-k">Одна схема</span><span class="mc-v">≈ 7 мин 11 с</span></div></div></div>
          </div>
          <div class="mode-caption" id="cap">Будет замерено 96 схем · примерно 1 ч 20 мин</div>
          <div class="adv-wrap open" id="adv">
            <button class="adv-toggle" type="button"><span class="adv-toggle-left"><svg viewBox="0 0 24 24" width="15" height="15"></svg><span>Параметры теста</span></span><span class="adv-toggle-right"><span class="adv-summary">Прогон 30 с · Разогрев 3 с</span><span class="adv-chevron"><i></i>Свернуть</span></span></button>
            <div class="adv-body"><div class="adv-clip"><div class="ph-grid"><div class="k-card"><span class="mc-k">Длительность прогона</span><b>30 с</b></div><div class="k-card"><span class="mc-k">Разогрев</span><b>3 с</b></div><div class="k-card"><span class="mc-k">Охлаждение</span><b>3 с</b></div></div></div></div>
          </div>
          </div>
        </div>
      </div>
      </div>
    </main>
  </div>
</div>
<script>
window.measureBench = (open) => {
  document.getElementById("adv").classList.toggle("open", open);
  const px = (v) => Math.round(v * 100) / 100;
  const main = document.querySelector(".main");
  const m = main.getBoundingClientRect();
  const padTop = parseFloat(getComputedStyle(main).paddingTop);
  const padBottom = parseFloat(getComputedStyle(main).paddingBottom);
  const hdr = document.getElementById("hdr").getBoundingClientRect();
  const modes = document.getElementById("modes");
  const mr = modes.getBoundingClientRect();
  const adv = document.getElementById("adv").getBoundingClientRect();
  const top = m.top + padTop;
  const bottom = m.bottom - padBottom;
  const last = document.getElementById("adv");
  const card = document.querySelector(".mode-card").getBoundingClientRect();
  // Сжатые блоки: высота меньше содержимого. Свёрнутую панель параметров не
// проверяем: её тело схлопнуто анимацией в ноль (grid-row 0fr) и обрезано по
// замыслу, это не поломка раскладки.
  const squashed = [];
  const advEl = document.getElementById("adv");
  const watch = [document.getElementById("stage"), modes, document.getElementById("cap")];
  if (advEl.classList.contains("open")) watch.push(advEl);
  for (const el of watch) {
    const h = el.getBoundingClientRect().height;
    if (el.scrollHeight > h + 1) squashed.push(el.id + " " + Math.round(h) + "<" + el.scrollHeight);
  }
  return {
    headerTop: px(hdr.top - top),
    modesHeight: px(mr.height),
    cardHeight: px(card.height),
    advHeight: px(adv.height),
    wizardHeight: px(document.getElementById("wizard").getBoundingClientRect().height),
    stageHeight: px(document.getElementById("stage").getBoundingClientRect().height),
    pageHeight: px(document.getElementById("page").getBoundingClientRect().height),
    hostHeight: px(document.getElementById("host").getBoundingClientRect().height),
    avail: px(m.height - padTop - padBottom),
    chain: (() => {
      const st = document.getElementById("stage");
      return getComputedStyle(st).flex + " родитель=" + getComputedStyle(st.parentElement).display;
    })(),
    freeBottom: px(bottom - (adv.top + adv.height)),
    overflowBottom: px((adv.top + adv.height) - bottom),
    squashed: squashed,
  };
};
</script></body></html>`;

  const server = http.createServer((req, res) => {
    const url = req.url.split("?")[0];
    if (url === "/" || url === "/stand.html") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return res.end(page);
    }
    if (url === "/center") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return res.end(CENTER_PAGE);
    }
    if (url === "/bench") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return res.end(BENCH_PAGE);
    }
    const file = path.join(BUILD, decodeURIComponent(url).replace(/^\/+/, ""));
    if (!fs.existsSync(file) || fs.statSync(file).isDirectory()) { res.writeHead(404); return res.end("нет"); }
    const type = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".woff2": "font/woff2" }[path.extname(file)] || "application/octet-stream";
    res.writeHead(200, { "content-type": type });
    fs.createReadStream(file).pipe(res);
  });
  await new Promise((r) => server.listen(PORT, r));
  console.log("стенд: http://127.0.0.1:" + PORT + "/");

  const local = process.env.LOCALAPPDATA || "";
  const wv = "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application";
  const candidates = [
    path.join(local, "Chromium", "Application", "chrome.exe"),
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
  ];
  if (fs.existsSync(wv)) for (const v of fs.readdirSync(wv).sort().reverse()) {
    const e = path.join(wv, v, "msedgewebview2.exe");
    if (fs.existsSync(e)) candidates.push(e);
  }
  const exe = candidates.find((p) => fs.existsSync(p));
  if (!exe) { console.log("не найден Chromium"); server.close(); return; }
  const proc = spawn(exe, [
    "--headless=new", "--remote-debugging-port=9333", "--disable-gpu", "--no-first-run",
    "--user-data-dir=" + path.join(OUT, "chrome-adaptive"), "about:blank",
  ], { stdio: "ignore" });
  const cleanup = () => { try { proc.kill(); } catch {} server.close(); };
  process.on("exit", cleanup);
  proc.on("error", (e) => { console.log("браузер не запустился:", e.message); server.close(); process.exit(1); });

  let target = null;
  for (let i = 0; i < 60 && !target; i++) {
    await new Promise((r) => setTimeout(r, 500));
    try {
      const list = JSON.parse(await get("http://127.0.0.1:9333/json/list"));
      target = list.find((t) => t.type === "page");
    } catch {}
  }
  if (!target) { console.log("CDP не поднялся"); cleanup(); return; }
  if (typeof WebSocket === "undefined") { console.log("в Node нет WebSocket"); cleanup(); return; }

  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.addEventListener("open", res, { once: true }); ws.addEventListener("error", () => rej(new Error("ws")), { once: true }); });
  let id = 0;
  const pending = new Map();
  ws.addEventListener("message", (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
  });
  const send = (method, params) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
  const evalJs = async (expr) => {
    const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true });
    if (r.result && r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 200));
    return r.result.result.value;
  };

  await send("Page.enable");
  await send("Page.navigate", { url: "http://127.0.0.1:" + PORT + "/" });
  await new Promise((r) => setTimeout(r, 2500));
  if (!(await evalJs("typeof window.measure === 'function'"))) { console.log("стенд не загрузился"); cleanup(); return; }

  let bad = 0;
  console.log("\n  ширина  колонка  рельса  схемы  метрики  фазы  лог  чип1  чип2  статус");
  for (const w of WIDTHS) {
    await send("Emulation.setDeviceMetricsOverride", { width: w, height: 900, deviceScaleFactor: 1, mobile: false });
    await new Promise((r) => setTimeout(r, 350));
    const m = await evalJs("window.measure()");
    const t = m.tokens;
    const p = [];
    if (m.mainOverflowX > 2) p.push(`переполнение .main ${m.mainOverflowX}`);
    if (m.bodyOverflowX > 2) p.push(`переполнение body ${m.bodyOverflowX}`);
    if (px(m.column) > 1121) p.push(`колонка ${px(m.column)}`);
    if (px(m.navGlyph) !== 16) p.push(`иконка ${m.navGlyph}`);
    if (Math.abs(px(m.padLeft) - px(t["--pad-x"])) > 0.6) p.push(`pad-x ${m.padLeft} != ${t["--pad-x"]}`);
    // Пороги из ТЗ III.3 и III.4.
    const rail = w >= 1080 ? 64 : w >= 900 ? 56 : 52;
    if (px(m.railWidth) !== rail) p.push(`рельса ${px(m.railWidth)}, ожидалось ${rail}`);
    if (m.prio2 !== (w >= 1280)) p.push(`чип «Анимации» ${m.prio2 ? "виден" : "скрыт"} при ${w}`);
    if (m.prio1 !== (w >= 1080)) p.push(`чип железа ${m.prio1 ? "виден" : "скрыт"} при ${w}`);
    const logCols = w >= 1080 ? 3 : 2;
    if (m.logCols !== logCols) p.push(`колонок в строке лога ${m.logCols}, ожидалось ${logCols}`);
    // Схемы: при минимуме карточки 310 px три колонки помещаются до 1120,
    // дальше две (ТЗ III.1 обещает 3 на 1080, но при 310 px и зазоре 9 px
    // три колонки требуют 948 px, а колонка на 1080 — 928 px; выигрывает
    // запрет «колонок больше, чем помещается»).
    const schemes = w >= 1120 ? 3 : 2;
    if (m.schemesCols !== schemes) p.push(`схем ${m.schemesCols}, ожидалось ${schemes}`);
    if (px(m.railWidth) === 52 && m.kpiCols !== 1) p.push(`метрик ${m.kpiCols} при рельсе 52`);
    for (const [k, v] of Object.entries(EXPECT[w] || {})) {
      const got = px(t[k]);
      if (Math.abs(got - v) > TOL) p.push(`${k}=${got}, ожидалось ${v}`);
    }
    if (p.length) bad++;
    console.log(`  ${String(w).padStart(6)}  ${String(px(m.column)).padStart(7)}  ${String(px(m.railWidth)).padStart(6)}  ${String(m.schemesCols).padStart(5)}  ${String(m.kpiCols).padStart(7)}  ${String(m.phaseCols).padStart(4)}  ${String(m.logCols).padStart(3)}  ${(m.prio1 ? "да" : "нет").padStart(4)}  ${(m.prio2 ? "да" : "нет").padStart(4)}  ${p.length ? "✗ " + p.join("; ") : "ok"}`);
  }

  // Свёрнутая рельса: иконка обязана остаться 16 px.
  await send("Emulation.setDeviceMetricsOverride", { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  const col = await evalJs("window.measure()");
  const cp = [];
  if (px(col.navGlyph) !== 16) cp.push(`иконка ${col.navGlyph}`);
  if (px(col.navItem) !== 38) cp.push(`квадрат ${px(col.navItem)}`);
  if (cp.length) bad++;
  console.log(`\n  свёрнутая рельса на 1440: иконка ${col.navGlyph}, квадрат ${col.navItem}, панель ${px(col.railWidth)}  ${cp.length ? "✗ " + cp.join("; ") : "ok"}`);
  // Развёрнутая рельса: пункт во всю колонку, иконка всё так же 16 px.
  await evalJs("window.collapse(false)");
  await new Promise((r) => setTimeout(r, 400));
  const exp2 = await evalJs("window.measure()");
  const cp2 = [];
  if (px(exp2.navGlyph) !== 16) cp2.push(`иконка ${exp2.navGlyph}`);
  if (cp2.length) bad++;
  console.log(`  развёрнутая рельса: иконка ${exp2.navGlyph}, панель ${px(exp2.railWidth)}  ${cp2.length ? "✗ " + cp2.join("; ") : "ok"}`);
  await evalJs("window.collapse(true)");

  // Вертикальное центрирование на отдельной странице: короткий экран стоит
  // по центру, длинный прижат к верху и не уводит верх за область прокрутки.
  const cc = [];
  await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/center` });
  await new Promise((r) => setTimeout(r, 1200));
  for (const [h, w] of [[900, 1440], [720, 1180]]) {
    await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await evalJs("window.show('short')");
    await new Promise((r) => setTimeout(r, 250));
    const s = await evalJs("window.measureCentering()");
    await evalJs("window.show('tall')");
    await new Promise((r) => setTimeout(r, 250));
    const t = await evalJs("window.measureCentering()");
    if (Math.abs(s.shortOffset) > 8) cc.push(`${w}×${h}: короткий экран смещён на ${px(s.shortOffset)} px`);
    if (Math.abs(t.tallOffset) > 2) cc.push(`${w}×${h}: длинный экран не прижат к верху (${px(t.tallOffset)} px)`);
    if (t.pageScroll < 0) cc.push(`${w}×${h}: длинный экран вылез вверх за прокрутку на ${px(-t.pageScroll)} px`);
    // Ничего не обрезано по высоте.
    for (const which of ["short", "tall"]) {
      await evalJs(`window.show('${which}')`);
      await new Promise((r) => setTimeout(r, 200));
      const sq = await evalJs("window.measureSquash('#page')");
      for (const b of sq) cc.push(`${w}×${h} (${which}): сжато ${b}`);
    }
    await evalJs("window.show('short')");
  }
  if (cc.length) bad++;
  console.log(`\n  центрирование по вертикали на 1440×900 и 1180×720  ${cc.length ? "✗ " + cc.join("; ") : "ok"}`);

  // Поля вокруг колонки меряются на общей странице стенда, поэтому сначала
  // возвращаемся на неё с изолированной страницы центрирования.
  await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/` });
  await new Promise((r) => setTimeout(r, 1400));

  // Раскладка «Бенчмарка»: шапка с этапами и полосой — наверху, блоки забирают
// свободную высоту, ничего не сжато.
await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/bench` });
await new Promise((r) => setTimeout(r, 1200));
const bc = [];
console.log("\n  раскладка «Бенчмарка» (шапка наверху, блоки на всю высоту):");
for (const [w, h] of [[1920, 1080], [1440, 900], [1180, 720]]) {
  await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await new Promise((r) => setTimeout(r, 300));
  for (const open of [true, false]) {
    const b = await evalJs(`window.measureBench(${open})`);
    const tag = open ? "раскрыта " : "свёрнута ";
    if (b.headerTop > 2) bc.push(`${w}×${h}: шапка опустилась на ${px(b.headerTop)} px`);
    if (b.squashed.length) bc.push(`${w}×${h}: сжато ${b.squashed.join(",")}`);
    if (b.overflowBottom > 2) bc.push(`${w}×${h}: низ ушёл за область на ${px(b.overflowBottom)} px`);
    if (b.cardHeight < 200) bc.push(`${w}×${h} (${tag}): карточки режимов не растянуты (${px(b.cardHeight)} px)`);
    // Раскрытая панель забирает высоту целиком; свёрнутая ограничена потолком
    // карточек, и небольшой воздух внизу допустим.
    const limit = open ? 20 : 360;
    if (b.freeBottom > limit) bc.push(`${w}×${h} (${tag}): пустого места внизу ${px(b.freeBottom)} px`);
    console.log(`    ${String(w).padStart(4)}×${h} ${tag}: шапка +${px(b.headerTop)}  карточки ${px(b.cardHeight)}  параметры ${px(b.advHeight)}  пусто внизу ${px(b.freeBottom)}`);
    console.log(`        цепочка: host ${px(b.hostHeight)} → page ${px(b.pageHeight)} → wizard ${px(b.wizardHeight)} → stage ${px(b.stageHeight)} (доступно ${px(b.avail)}) flex=${b.chain}`);
  }
}
if (bc.length) bad++;
console.log(`    ${bc.length ? "вњ— " + bc.join("; ") : "ok"}`);
await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/` });
await new Promise((r) => setTimeout(r, 1200));
  // симметрия относительно окна. Колонка уже `max-width`, поэтому на 1280+
  // есть что центрировать, а на 1080 она занимает всю область.
  console.log("  поля вокруг колонки (окно):");
  const gp = [];
  for (const rail of [true, false]) {
    await evalJs(`window.collapse(${rail})`);
    await new Promise((r) => setTimeout(r, 450));
    const label = rail ? "свёрнута" : "развёрнута";
    for (const w of [1920, 1440, 1280, 1080]) {
      await send("Emulation.setDeviceMetricsOverride", { width: w, height: 900, deviceScaleFactor: 1, mobile: false });
      await new Promise((r) => setTimeout(r, 300));
      const g = await evalJs("window.measure()");
      const diff = px(g.gapRight) - px(g.gapLeft);
      // Колонка во всю область не центрируется: слева её отделяет рельса.
      const fits = g.column < g.innerWidth - 300;
      if (fits && Math.abs(diff) > 1) gp.push(`${label} ${w}: разница ${px(diff)} px`);
      console.log(`    ${label.padEnd(11)} ${String(w).padStart(4)}: слева ${px(g.gapLeft)}  справа ${px(g.gapRight)}  разница ${String(px(diff)).padStart(5)}  колонка ${px(g.column)}  рельса ${px(g.railRight - g.railLeft)}  main[${px(g.mainLeft)}..${px(g.mainRight)}] body-right−main-right ${px(g.bodyRight - g.mainRight)}`);
    }
  }
  await evalJs("window.collapse(true)");
  await new Promise((r) => setTimeout(r, 350));
  if (gp.length) bad++;
  console.log(`    ${gp.length ? "✗ " + gp.join("; ") : "симметрично"}`);
  await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/` });
  await new Promise((r) => setTimeout(r, 1200));

  console.log("\n  плотность × текст на 1180×720 (норма XIV.4):");
  await send("Emulation.setDeviceMetricsOverride", { width: 1180, height: 720, deviceScaleFactor: 1, mobile: false });
  for (const d of DENSITIES) {
    for (const t of TEXTS) {
      await evalJs(`window.setAttrs(${JSON.stringify(d)}, ${JSON.stringify(t)})`);
      await new Promise((r) => setTimeout(r, 250));
      const m = await evalJs("window.measure()");
      const p = [];
      if (m.bodyOverflowX > 2) p.push(`body ${m.bodyOverflowX}`);
      if (m.mainOverflowX > 2) p.push(`main ${m.mainOverflowX}`);
      if (px(m.navGlyph) !== 16) p.push(`иконка ${m.navGlyph}`);
      if (p.length) bad++;
      console.log(`    ${d.padEnd(8)} ${t}: h1 ${String(px(m.tokens["--fs-h1"])).padEnd(5)} body ${String(px(m.bodyFont)).padEnd(5)} ctrl ${String(px(m.tokens["--ctrl-h"])).padEnd(5)} gap ${String(px(m.tokens["--gap"])).padEnd(5)} ${p.length ? "вњ— " + p.join("; ") : "ok"}`);
    }
  }
  await evalJs("window.setAttrs('normal','m')");

  console.log(bad === 0 ? "\nадаптив: расхождений с ТЗ нет" : `\nадаптив: расхождений ${bad}`);
  cleanup();
  process.exit(bad === 0 ? 0 : 1);
}

main().catch((e) => { console.log("ошибка:", e.message); console.log(e.stack); process.exit(1); });
