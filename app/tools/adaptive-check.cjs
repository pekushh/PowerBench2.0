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

// Изолированная страница для проверки вертикального положения содержимого:
// здесь в колонке либо короткий блок, либо длинный. На общей странице стенда
// колонка всегда длиннее окна, и положение нечего проверять.
//
// Проверяется одно правило на всех экранах: содержимое начинается сразу под
// верхним отступом `.main`. Раньше короткая страница центрировалась по
// вертикали, а длинная прижималась к верху, поэтому переключение вкладки
// сдвигало заголовок на сотни пикселей.
const CENTER_PAGE = `<!DOCTYPE html><html lang="ru" data-theme="Graphite" data-mode="Dark" data-density="normal" data-text="m" data-motion="on">
<meta charset="utf-8"><title>Положение содержимого</title>
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
  // Отступ от ВНУТРЕННЕЙ границы .main: у .main есть верхний отступ
  // --sp-5, и сравнивать с рамкой дало бы ложные 19–20 px.
  const top = m.top + parseFloat(getComputedStyle(main).paddingTop);
  return {
    // Короткий экран: расстояние от отступа до первого блока. Раньше он
    // стоял по центру, и это число равнялось половине свободного места.
    // Дробная часть сверстана: вёрстка кеглей и отступов даёт полупиксели, и
    // сравнивать их с нулём бессмысленно — важен сдвиг целиком, а не дробь.
    shortTop: r0.top - top,
    // Длинный экран: то же самое. Должно совпадать с shortTop до пикселя.
    tallTop: r0.top - top,
    pageTop: p.top - top,
    pageScroll: pageEl.scrollHeight - pageEl.clientHeight,
  };
};
</script></body></html>`;

// Пять настоящих шапок — по одной с каждого экрана, — в одной колонке. Меряется
// положение заголовка: после правок макета оно обязано совпадать до пикселя,
// иначе при переключении вкладки заголовок прыгает.
const HEAD_PAGE = `<!DOCTYPE html><html lang="ru" data-theme="Graphite" data-mode="Dark" data-density="normal" data-text="m" data-motion="on">
<meta charset="utf-8"><title>Шапки экранов</title>
<link rel="stylesheet" href="/assets/${cssHref}">
<style>html,body{margin:0;height:100%}</style>
<body>
<div class="shell rail-collapsed">
  <div class="body">
    <aside class="sidebar collapsed"><nav class="sidebar-nav"><button class="nav-item active"><span class="nav-glyph"><svg viewBox="0 0 24 24"></svg></span><span class="nav-label">Бенчмарк</span></button></nav></aside>
    <main class="main">
      <div class="page">
        <div class="page-host on"><div class="page" id="p1"><div class="page-head"><h1>Бенчмарк</h1><nav class="stepper"><span class="step">1 Режим и настройки</span></nav><div class="actions"><span class="btn btn-primary">Далее →</span></div></div><div class="wizard"><div class="wizard-stage centered"><div class="mode-grid"><div class="mode-card selected"><div class="mc-pill"><small>Повторов</small><b>5 раундов</b></div></div><div class="mode-card"><div class="mc-pill"><small>Повторов</small><b>1 раунд</b></div></div></div></div></div></div></div>
        <div class="page-host on"><div class="page" id="p2"><div class="page-head"><h1>Схемы питания</h1><span class="sub">96 схем</span><div class="actions"><span class="act-primary">Импорт</span></div></div><div class="schemes-grid"><div class="scheme-card"><div class="sc-name">Схема</div></div></div></div></div>
        <div class="page-host on"><div class="page fill" id="p3"><div class="page-head"><h1>Результаты</h1><span class="sub">24 сессии</span><div class="actions"><span class="act-secondary">Обновить</span></div></div><div class="session-list fill-list"><article class="session-card"><div class="sc-main"><div class="sc-top"><span class="sc-title">Схема</span></div></div></article></div></div></div>
        <div class="page-host on"><div class="page fill" id="p4"><div class="page-head"><h1>Логи</h1><div class="actions"><span class="export-btn">Сохранить</span></div></div><div class="log-controls"><div class="filter-tabs pb-pill-host"><button class="ftab active">Все</button></div></div><div class="log-card"><div class="log-list"><div class="log-row"><span class="l-time">14:19:02</span><span class="l-badge info">Инфо</span><span class="l-msg">Схема применена</span></div></div></div></div></div>
        <div class="page-host on"><div class="page set-page" id="p5"><div class="page-head"><h1>Настройки</h1><div class="actions"><span class="folder-btn">Папка результатов</span></div></div><section class="set-sec"><div class="section-head"><h2 class="section-title">Внешний вид</h2></div><div class="set-group"><div class="set-row"><div class="set-left"><span class="set-icon"></span><div class="set-text"><div class="set-title">Тема оформления</div></div></div><div class="seg"><button class="on">Тёмная</button><button>Светлая</button></div></div></div></div></section></div></div>
        <div class="results-toolbar" id="toolbar"><div class="split-act" id="split"><button class="act-secondary" id="ex">Экспорт…</button><button class="act-secondary caret" id="fmt">▾</button><div class="mini-menu" id="menu"><button>JSON</button><button>CSV</button></div></div></div>
        <div class="filter-tabs pb-pill-host" id="tabs"><button class="ftab active" id="tab">Все</button><button class="ftab" id="tab2">Скрининг</button></div>
      </div>
    </main>
  </div>
</div>
<script>
// Положение заголовка каждого экрана относительно верхнего отступа .main
// и одинаковые метрики самого заголовка: вес и межбуквенный интервал
// раньше различались (700 / −0.3 px против 650 / −0.2 px).
window.measureLayers = () => {
  // У каждого, кто обязан перекрывать что-то, должен быть СОБСТВЕННЫЙ контекст
  // наложения. Без isolation:isolate (или z-index не auto) его z-index
  // считается в корневом контексте и конкурирует со всем приложением: любой
  // позиционированный элемент, идущий позже в DOM, перекрывал его вместо
  // того, чтобы быть перекрытым.
  const need = [
    [".pb-pill-host", "группа вкладок"],
    [".modal-overlay", "оверлей модалки"],
    [".split-act", "кнопка экспорта с меню"],
  ];
  const bad = [];
  for (const [sel, name] of need) {
    const el = document.querySelector(sel);
    if (!el) continue;
    const cs = getComputedStyle(el);
    if (cs.isolation !== "isolate" && cs.zIndex === "auto") bad.push(name + " (" + sel + ")");
  }
  // Порядок слоёв зафиксирован и не должен ломаться: тосты над модалкой,
  // оверлей над меню формата.
  const z = (sel) => {
    const el = document.querySelector(sel);
    return el ? Number(getComputedStyle(el).zIndex) || 0 : null;
  };
  const menu = z(".mini-menu"), overlay = z(".modal-overlay"), toasts = z(".toasts");
  if (menu != null && overlay != null && menu > overlay) bad.push("меню экспорта выше оверлея модалки");
  if (overlay != null && toasts != null && overlay > toasts) bad.push("оверлей модалки выше тостов");
  return bad;
};
// Меню формата экспорта и пилюля вкладок — два элемента, которые перекрывают
// соседей. Проверяем, что их никто не срезает: выпадающее меню обязано целиком
// показываться поверх содержимого под ним, а не обрезаться родителем с
// overflow и не уезжать за границу рабочей области.
window.measureOverlap = () => {
  const bad = [];
  const main = document.querySelector(".main");
  const mainBox = main.getBoundingClientRect();

  // Меню: ни один предок не должен его обрезать, и оно обязано помещаться
  // в горизонтальные границы рабочей области.
  const menu = document.getElementById("menu");
  const mr = menu.getBoundingClientRect();
  if (mr.height < 10) bad.push("меню формата схлопнулось в ноль (" + Math.round(mr.height) + " px)");
  if (mr.left < mainBox.left - 1 || mr.right > mainBox.right + 1) {
    bad.push("меню формата вылезло за границу рабочей области");
  }
  for (let el = menu.parentElement; el && el !== main; el = el.parentElement) {
    const cs = getComputedStyle(el);
    const clips = cs.overflow !== "visible" || cs.overflowY !== "visible" || cs.overflowX !== "visible";
    if (clips && el.id !== "split") bad.push("меню формата обрезает предок ." + (el.className || el.id || el.tagName));
  }

  // Пилюля вкладок: лежит под своей кнопкой, а не поверх неё, и не выходит за
  // границы своей группы.
  const host = document.getElementById("tabs");
  const pill = host.querySelector(".pb-pill");
  if (pill) {
    const pr = pill.getBoundingClientRect();
    const hr = host.getBoundingClientRect();
    if (pr.width <= 0) bad.push("пилюля вкладок не имеет ширины");
    if (pr.left < hr.left - 1 || pr.right > hr.right + 1) bad.push("пилюля вылезла за границы группы вкладок");
    if (getComputedStyle(pill).zIndex >= getComputedStyle(host.firstElementChild).zIndex) {
      bad.push("пилюля вкладок не под своей кнопкой");
    }
  }
  return bad;
};

window.measureHeads = () => {
  const main = document.querySelector(".main");
  const out = {};
  for (const id of ["p1", "p2", "p3", "p4", "p5"]) {
    const page = document.querySelector("#" + id);
    const h1 = page.querySelector(".page-head h1");
    const cs = getComputedStyle(h1);
    const r = h1.getBoundingClientRect();
    // Отсчёт от верхнего отступа СВОЕЙ колонки: на стенде все пять шапок
    // стоят друг за другом, и отступ от верха .main у них разный по
    // определению. Проверяем ровно то, что требуется от макета: заголовок
    // находится на одном расстоянии от начала своей страницы.
    const pageTop = page.getBoundingClientRect().top + parseFloat(getComputedStyle(page).paddingTop);
    out[id] = {
      top: Math.round((r.top - pageTop) * 100) / 100,
      left: Math.round((r.left - page.getBoundingClientRect().left) * 100) / 100,
      weight: cs.fontWeight,
      spacing: cs.letterSpacing,
      size: cs.fontSize,
      // Ширина шапки: последний блок (кнопки) не должен заезжать за правый
      // край колонки — при переносе он вылезал за границу фрейма.
      headRight: Math.round(page.querySelector(".page-head").getBoundingClientRect().right * 100) / 100,
      pageRight: Math.round(page.getBoundingClientRect().right * 100) / 100,
      overflow: main.scrollWidth - main.clientWidth,
    };
  }
  return out;
};
</script></body></html>`;

const SKELETON = `
<div class="shell rail-collapsed">
  <div class="titlebar">
    <div class="titlebar-brand"><button class="titlebar-btn menu"></button><span class="mark"></span><span>PowerBench</span></div>
    <span class="split"></span><span class="titlebar-section is-in">Готово</span>
    <div class="titlebar-chips"><span class="titlebar-chip on"><span class="dot-live"></span>Тихий режим: вкл</span><span class="titlebar-chip trunc" data-prio="1">Ryzen 9 7950X · 32 ГБ</span></div>
    <div class="titlebar-controls"><span class="window-group"><button class="titlebar-btn"></button><button class="titlebar-btn close"></button></span></div>
  </div>
  <div class="body">
    <aside class="sidebar collapsed" id="rail">
<nav class="sidebar-nav" id="nav">
        <button class="nav-item active" data-tip="Бенчмарк"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Бенчмарк</span></button>
        <button class="nav-item" data-tip="Схемы питания"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Схемы питания</span></button>
        <button class="nav-item" data-tip="Результаты"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Результаты</span></button>
        <button class="nav-item" data-tip="Логи"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Логи</span></button>
      </nav>
      <div class="sidebar-foot" id="foot"><button class="nav-item" data-tip="Настройки"><span class="nav-glyph">${'<svg viewBox="0 0 24 24"><path d="M4 6h16" stroke="currentColor" fill="none"/></svg>'}</span><span class="nav-label">Настройки</span></button></div>
    </aside>
    <main class="main">
      <div class="page">
        <div class="page-head"><h1>Бенчмарк</h1><span class="sub">режим и настройки</span></div>
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
  const mainEl = document.querySelector(".main");
  const pr = pageEl.getBoundingClientRect();
  const rr = railEl.getBoundingClientRect();
  const br = bodyEl.getBoundingClientRect();
  const inner = window.innerWidth;
  const logRow = document.querySelector(".log-row");
  // Границы ПОЛЕЗНОЙ области .main: рамка и отступы сняты, а зарезервированный
  // под полосу прокрутки глоб (scrollbar-gutter: stable) — тоже. Именно внутри
  // этой области колонка должна стоять симметрично: глоб отнимается справа, и
  // сравнение с рамкой панели давало бы постоянные «10 px асимметрии», которых
  // на экране нет.
  // (Комментарий без обратных кавычек: он лежит внутри шаблонного литерала.)
  const mcs = getComputedStyle(mainEl);
  const mRect = mainEl.getBoundingClientRect();
  const bL = parseFloat(mcs.borderLeftWidth) || 0;
  const pL = parseFloat(mcs.paddingLeft) || 0;
  const pR = parseFloat(mcs.paddingRight) || 0;
  // clientWidth — ширина ПЕДИНГ-бокса минус полоса/глоб, то есть отсчёт
  // идёт от внутреннего края левой рамки. Поэтому правый край = левый край
  // рамки + clientWidth минус правый отступ.
  const contentLeft = mRect.left + bL + pL;
  const contentRight = mRect.left + bL + mainEl.clientWidth - pR;
  return {
    tokens,
    contentLeft: Math.round(contentLeft * 100) / 100,
    contentRight: Math.round(contentRight * 100) / 100,
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
    // Геометрия нижней кнопки «Настройки» сравнивается с верхней: обе
    // обязаны быть одного размера, с одинаковыми отступами от краёв рельсы
    // и одинаковым радиусом. Считаем отступы ОТ РЕЛЬСЫ, а не абсолютные
    // координаты — те сдвинуты на её ширину и ничего не сравнивают.
    footItem: (() => {
      const f = document.querySelector("#foot .nav-item");
      if (!f) return null;
      const fr = f.getBoundingClientRect();
      const ir = item.getBoundingClientRect();
      const rr2 = railEl.getBoundingClientRect();
      const fcs = getComputedStyle(f);
      return {
        w: Math.round(fr.width * 100) / 100,
        h: Math.round(fr.height * 100) / 100,
        insetL: Math.round((fr.left - rr2.left) * 100) / 100,
        gapR: Math.round((rr2.right - fr.right) * 100) / 100,
        radius: fcs.borderRadius,
        padT: fcs.paddingTop,
        padB: fcs.paddingBottom,
        // Ширина блока-контейнера: если подвал шире списка, кнопка внутри него
        // тоже шире, даже когда отступы совпадают.
        footBoxW: Math.round(document.querySelector("#foot").getBoundingClientRect().width * 100) / 100,
        topBoxW: Math.round(document.querySelector("#nav").getBoundingClientRect().width * 100) / 100,
        topInsetL: Math.round((ir.left - rr2.left) * 100) / 100,
        topGapR: Math.round((rr2.right - ir.right) * 100) / 100,
        // Высота — отдельно: у подвала своё выравнивание по умолчанию, и при
        // растягивании кнопка во всю высоту блока, а у списка блок по
        // контенту. На глаз это читается как «кнопка внизу крупнее».
        topH: Math.round(ir.height * 100) / 100,
        topMinH: getComputedStyle(item).minHeight,
        topAlign: getComputedStyle(document.querySelector("#nav")).alignItems,
        footAlign: getComputedStyle(document.querySelector("#foot")).alignItems,
        footPadAll: getComputedStyle(document.querySelector("#foot")).padding,
        navPadAll: getComputedStyle(document.querySelector("#nav")).padding,
      };
    })(),
    railWidth: document.getElementById("rail").getBoundingClientRect().width,
    bodyFont: cs2(document.body).fontSize,
    schemesCols: cols("#schemes"),
    kpiCols: cols("#kpis"),
    phaseCols: cols("#phases"),
    prio1: shown('[data-prio="1"]'),
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
      <div class="page" id="page">
        <div class="page-head" id="hdr">
          <h1>Бенчмарк</h1>
          <div class="stepper"><span class="step active">1 Режим и настройки</span><span class="step">2 Схемы питания</span><span class="step">3 Запуск</span></div>
          <div class="actions"><button class="btn btn-primary" type="button">Далее →</button></div>
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
// Переключение панели параметров и её замер — РАЗНЫЕ шаги.
//
// Раньше measureBench делал и то и другое одним вызовом, то есть мерил высоту
// в тот же кадр, в который менялся класс .open. Но раскрытие анимируется
// (grid-template-rows, --t-3 = .26s), и мгновенный замер попадал в середину
// перехода: панель выглядела обрезанной (54 px при содержимом 84 px), и проверка
// писала «сжато adv 54<84». После окончания перехода (348 px при содержимом
// 346 px на 1440x900) обрезки нет — ломалась сама проверка, а не вёрстка.
window.setBench = (open) => {
  document.getElementById("adv").classList.toggle("open", open);
};
window.measureBench = () => {
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

// ===== Стенд «Логи» =====
// Разметка повторяет `LogPage`: колонка `.page.fill` с шапкой, рядом фильтров
// и карточкой журнала, внутри шапка карточки и лента записей. Лента длинная —
// именно случай, на котором ломалась геометрия: колонка без определённой
// высоты росла вместе с записями, и прокручивал `.main`, а не лента.
// Строки собираются отдельно: вложенный шаблонный литерал внутри шаблонного
// литерала закрывает внешний обратной кавычкой.
// 200 записей — заведомо больше окна любой высоты, которое проверяет стенд.
const LOG_ROWS = Array.from({ length: 200 }, (_, i) =>
  '<div class="log-row" data-level="info"><span class="l-time">14:' +
  String(i % 60).padStart(2, "0") + ':02</span><span class="l-badge info">Инфо</span>' +
  '<span class="l-msg">Запись номер ' + i +
  ' — достаточно длинный текст, чтобы строка переносилась и список был выше окна</span></div>',
).join("");

// ===== Стенд «Результаты» =====
// Разметка повторяет `ResultsPage`: колонка `.page.fill`, тулбар и список
// карточек `.session-list` внутри `.fade-list`. Нужен, чтобы поймать
// горизонтальную прокрутку и срезание карточки при подъёме: на общей
// странице стенда этих блоков нет, а ломаются они именно здесь.
//
// Карточки собираются склейкой строк: вложенный шаблонный литерал внутри
// шаблонного литерала закрыл бы внешний обратной кавычкой.
const RESULT_CARDS = Array.from({ length: 4 }, (_, i) =>
  '<article class="session-card"><div class="sc-main"><div class="sc-top">' +
  '<span class="sc-winner-tag">ЛИДЕР</span><span class="sc-title">Powerplan</span></div>' +
  '<div class="sc-sub"><span>01.10.2026 · 20:03</span><span class="dot-sep">·</span>' +
  '<span class="badge">Скрининг</span></div></div>' +
  '<div class="sc-metrics"><div><small>РЕЗУЛЬТАТ</small><b class="num">' +
  (3515 - i) +
  '</b><i>тик/с</i></div><div><small>СХЕМ</small><b class="num">2</b>' +
  '<i>— мало данных</i></div></div>' +
  '<div class="sc-actions"><button class="btn-report">Отчёт HTML</button></div></article>',
).join("");

const RESULTS_PAGE = `<!doctype html><html lang="ru" data-theme="Graphite" data-mode="Dark" data-density="normal" data-text="m" data-motion="on">
<head><meta charset="utf-8">
<link rel="stylesheet" href="/assets/${cssHref}">
<style>html,body{margin:0;height:100%}</style>
</head><body>
<div class="shell"><div class="titlebar"><div class="titlebar-brand"><span>PowerBench</span></div></div>
<div class="body">
<aside class="sidebar collapsed"><nav class="sidebar-nav"><button class="nav-item"><span class="nav-glyph"><svg viewBox="0 0 24 24"></svg></span><span class="nav-label">Результаты</span></button></nav></aside>
<main class="main">
<div class="page-host on pb-host-in"><div class="page fill">
  <div class="page-head"><h1>Результаты</h1><span class="sub">3 из 200 сессий · 793.4 КБ</span>
    <div class="actions"><button class="act-secondary">Экспорт…</button><button class="act-secondary">Папка</button></div>
  </div>
  <div class="res-toolbar">
    <div class="search-box res-search"><svg viewBox="0 0 24 24"></svg><input class="search" placeholder="Поиск по схеме или дате."></div>
    <div class="filter-tabs pb-pill-host"><button class="ftab active">Все</button><button class="ftab">Скрининг</button></div>
    <select class="dd-btn">По дате</select>
  </div>
  <div class="session-list fill-list">${RESULT_CARDS}</div>
</div></div>
</main>
</div></div>
<script>
window.measureResults = () => {
  const box = (sel) => {
    const el = document.querySelector(sel);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    return {
      l: Math.round(r.left), r: Math.round(r.right), w: Math.round(r.width),
      sw: el.scrollWidth, cw: el.clientWidth,
      ovx: cs.overflowX, ovy: cs.overflowY,
      overX: el.scrollWidth - el.clientWidth,
    };
  };
  return {
    html: box("html"), body: box("body"), main: box(".main"),
    host: box(".page-host"), page: box(".page"),
    list: box(".session-list"), card: box(".session-card"),
    docScrollW: document.documentElement.scrollWidth,
    docClientW: document.documentElement.clientWidth,
  };
};
// Положение карточки ДО наведения: подъём измеряется как разница.
window.armLift = () => {
  window.__liftTop = document.querySelector(".session-card").getBoundingClientRect().top;
};
window.liftGap = () => {
  const list = document.querySelector(".session-list");
  const card = document.querySelector(".session-card");
  const lr = list.getBoundingClientRect();
  const cr = card.getBoundingClientRect();
  return {
    gap: Math.round(cr.top - lr.top),
    lift: Math.round(window.__liftTop - cr.top),
  };
};
window.hoverCard = () => {
  const r = document.querySelector(".session-card").getBoundingClientRect();
  return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
};
</script></body></html>`;

const LOG_PAGE = `<!doctype html><html lang="ru"><head><meta charset="utf-8">
<link rel="stylesheet" href="/assets/${cssHref}"></head><body>
<div class="shell"><div class="titlebar"><div class="titlebar-brand"><span>PowerBench</span></div></div>
<div class="body">
  <aside class="sidebar collapsed"><nav class="sidebar-nav"><button class="nav-item active"><span class="nav-glyph"><svg viewBox="0 0 24 24"></svg></span><span class="nav-label">Логи</span></button></nav></aside>
  <main class="main">
    <div class="page-host on pb-host-in"><div class="page fill" id="lp">
      <div class="page-head"><h1>Логи</h1></div>
      <div class="log-controls"><div class="filter-tabs pb-pill-host"><button class="ftab active">Все</button></div></div>
      <div class="log-card">
        <div class="log-card-head"><span class="log-meta">5000 записей</span></div>
        <div class="log-list" id="ll">${LOG_ROWS}</div>
      </div>
    </div></div>
  </main>
</div></div>
<script>
window.measureLog = () => {
  const px = (v) => Math.round(v * 100) / 100;
  const main = document.querySelector(".main");
  const list = document.getElementById("ll");
  const head = document.querySelector(".log-card-head");
  const card = document.querySelector(".log-card");
  const m = main.getBoundingClientRect();
  const padBottom = parseFloat(getComputedStyle(main).paddingBottom);
  const bottom = m.bottom - padBottom;
  return {
    // Лента обязана прокручиваться сама: иначе список либо не виден вовсе,
    // либо страницу прокручивает .main и колесо мыши уезжает из ленты.
    // Комментарии внутри этого литерала не должны содержать обратную
    // кавычку: она закрыла бы шаблон и остаток файла стал бы кодом.
    listScrolls: list.scrollHeight > list.clientHeight + 1,
    listOverflowY: getComputedStyle(list).overflowY,
    mainScrolls: main.scrollHeight > main.clientHeight + 1,
    // Нижняя граница карточки не должна уезжать за нижний край рабочей области.
    cardBottom: px(card.getBoundingClientRect().bottom - bottom),
    listH: px(list.clientHeight),
    // Шапка карточки зафиксирована: она не уезжает при прокрутке ленты.
    headBefore: head.getBoundingClientRect().top,
    headAfter: (() => { list.scrollTop = list.scrollHeight; return head.getBoundingClientRect().top; })(),
  };
};
</script></body></html>`;

  // ===== Стенд «Результат сессии» (модальное окно) =====
// Разметка повторяет `SessionDetail`: баннер замечаний, блок лидера с
// метриками, таблица фаз и — по переключателю — таблица сравнения. Проверяем
// три вещи, которые ломались по отдельности: баннер не занимает треть окна
// (три плашки занимали), у внутренних блоков нет жёстких рамок, и при одной
// схеме таблица сравнения не выводится вовсе — сравнивать не с чем.
const SESSION_MODAL_PAGE = `<!doctype html><html lang="ru" data-theme="Graphite" data-mode="Dark" data-density="normal" data-text="m" data-motion="on">
<head><meta charset="utf-8">
<link rel="stylesheet" href="/assets/${cssHref}">
<style>html,body{margin:0;height:100%}</style>
</head><body>
<div class="modal-overlay">
  <div class="modal session-modal">
    <div class="modal-head"><span class="modal-title"><span class="sm-title">Результат сессии<span class="sm-hw-meta">01.10.2026 20:03 &middot; Ryzen 9 7950X (16 ядер) &middot; 32 ГБ ОЗУ</span></span></span><button class="modal-close">×</button></div>
    <div class="modal-body">
      <div class="alerts-banner">
        <span class="alerts-mark">⚠</span>
        <div class="alerts-body">
          <span class="alerts-title">Замечания к результату</span>
          <span class="alert-row"><b>Скрининг &middot; 1 прогон:</b><span>нужно от 3 для доверительного интервала</span></span>
          <span class="alert-row is-strong"><b>Фон до 98 % CPU:</b><span>95-й перцентиль фоновой нагрузки выше порога.</span></span>
          <span class="alert-row is-strong"><b>Замечание:</b><span>замер проведён с риском: гейт отложенных прогонов отключён по вашему выбору</span></span>
          <span class="alert-more">ещё <b>2</b> замечания</span>
        </div>
      </div>
      <section class="leader-hero">
        <div class="lh-left">
          <div class="lh-eyebrow">
            <span class="badge-leader">★ Лидер сессии</span>
            <span class="badge-pill">Скрининг, 1 прогон</span>
            <span class="badge-pill">Уверенность: н/д</span>
          </div>
          <h3 class="lh-name">Максимальная производительность</h3>
        </div>
        <div class="lh-metrics">
          <div class="m-box"><small>Медиана лидера</small><b class="green">3515</b><span>тик/с</span></div>
          <div class="m-box"><small>Перевес</small><b class="green">+2.14%</b></div>
          <div class="m-box"><small>Замерено схем</small><b>1</b><span>из 1</span></div>
          <div class="m-box"><small>Прогонов</small><b>1</b><span>всего</span></div>
        </div>
      </section>
      <section class="section-card">
        <div class="sc-bar"><h4 class="sc-title">Показатели лидера по фазам</h4></div>
        <table class="phases-tbl">
          <thead><tr><th>Фаза нагрузки</th><th>Медиана, тик/с</th><th>P1 (мин. 1%), тик/с</th><th>Стабильность</th><th>Частота CPU</th></tr></thead>
          <tbody>
            <tr><td>1. Лёгкая</td><td>2980.2</td><td>2901.1</td><td>99.1 %</td><td>4200 МГц</td></tr>
            <tr><td>2. Частичная</td><td>3402.8</td><td>3310.4</td><td>98.4 %</td><td>4200 МГц</td></tr>
            <tr><td>3. Тяжёлая</td><td>3515.0</td><td>3444.9</td><td>97.9 %</td><td>3900 МГц ↓7%</td></tr>
            <tr><td>4. Отклик</td><td>2710.5</td><td>2690.2</td><td>99.5 %</td><td>4200 МГц</td></tr>
          </tbody>
        </table>
      </section>
      <section class="section-card" id="compare">
        <div class="schemes-toolbar"><div class="st-left"><h4 class="sc-title">Сравнение схем питания</h4></div></div>
        <div class="schemes-scroll">
          <table class="schemes-tbl">
            <thead><tr><th>Схема питания</th><th>Медиана, тик/с</th><th>ДИ 95%</th><th>Прогонов</th><th>Статус</th></tr></thead>
            <tbody>
              <tr class="is-leader"><td><span class="rk-num">#1</span>★ Максимальная производительность<span class="guid-tail">381b4222…</span></td><td><span class="score-cell"><span class="score-bar"><i style="width:100%"></i></span><b>3515.0</b></span></td><td class="mut">[3480.1; 3549.9]</td><td>1</td><td><span class="st-badge leader">лидер</span></td></tr>
            </tbody>
          </table>
        </div>
      </section>
    </div>
    <div class="modal-foot"><div class="sm-foot-inner"><div class="sm-foot-meta"><span>✓ Исходная схема «Сбалансированная» восстановлена</span></div><div class="sm-foot-actions"><button class="btn-delete">Удалить запись</button><button class="btn-open-report">Открыть HTML-отчёт</button></div></div></div>
  </div>
</div>
<script>
// Доли окна, которые занимают баннер замечаний и таблица сравнения. Раньше
// три плашки занимали около трети окна, поэтому порог задан жёстко.
window.measureSession = () => {
  const px = (v) => Math.round(v * 100) / 100;
  const modal = document.querySelector(".session-modal");
  const body = document.querySelector(".modal-body");
  const box = (sel) => {
    const el = document.querySelector(sel);
    return el ? px(el.getBoundingClientRect().height) : null;
  };
  // Рамки внутренних блоков: у лидера, метрик и баннера их быть не должно —
  // вместо них заливка/разделитель.
  const borders = {};
  for (const [name, sel] of [["hero", ".leader-hero"], ["box", ".m-box"], ["alerts", ".alerts-banner"], ["chip", ".alert-row"]]) {
    const el = document.querySelector(sel);
    if (!el) continue;
    const cs = getComputedStyle(el);
    borders[name] = [
      cs.borderTopWidth, cs.borderRightWidth, cs.borderBottomWidth, cs.borderLeftWidth,
    ].join(" ");
  }
  return {
    modalH: px(modal.getBoundingClientRect().height),
    bodyH: px(body.getBoundingClientRect().height),
    alertsH: box(".alerts-banner"),
    compareH: box("#compare"),
    heroH: box(".leader-hero"),
    // Высота тулбара секции сравнения: по ней видно, ушёл ли блок целиком, а не
    // осталась ли его шапка над пустым местом.
    toolbarH: box("#compare .schemes-toolbar"),
    // Подсказка скрытых замечаний: значок обязан быть, иначе часть причин
    // исчезла бы из окна молча.
    hasMore: !!document.querySelector(".alert-more"),
    borders,
  };
};
// Показать/скрыть таблицу сравнения — переключатель проверки правила
// «при одной схеме таблицы нет».
window.setCompare = (on) => {
  document.getElementById("compare").style.display = on ? "" : "none";
};
</script></body></html>`;

  // ===== Стенд подсказки =====
// Проверяет ровно тот баг, который видел пользователь: подсказка появлялась
// «в углу экрана», а не рядом со значком. Корневая причина — `position: fixed`
// не значит «от окна»: любой предок с `transform`, `filter`, `will-change:
// transform`, `perspective` или `contain` становится containing block для
// фиксированного потомка, и координаты окна применяются к нему со смещением.
//
// В приложении подсказка выносится порталом в `document.body`, поэтому у неё
// нет ни одного предка кроме body и перехватить позиционирование нечему.
// Стенд повторяет именно эту разметку: значок — внутри модалки, пузырь —
// прямым потомком body. Так проверяется то, что действительно поставляется.
const TIP_PAGE = `<!doctype html><html lang="ru" data-theme="Graphite" data-mode="Dark" data-density="normal" data-text="m" data-motion="on">
<head><meta charset="utf-8">
<link rel="stylesheet" href="/assets/${cssHref}">
<style>html,body{margin:0;height:100%}</style>
</head><body>
<div class="modal-overlay">
  <div class="modal session-modal" style="width:min(620px,92vw)">
    <div class="modal-head"><span class="modal-title">Схема</span><button class="modal-close">×</button></div>
    <div class="modal-body">
      <div class="section-card">
        <div class="sc-bar"><h4 class="sc-title">Показатели по фазам</h4>
          <span class="tt" id="anchor"><span class="tt-icon" tabindex="0" role="img">i</span></span>
        </div>
      </div>
      <div class="section-card">
        <div class="sc-bar"><h4 class="sc-title">Блок у самого низа окна</h4>
          <span class="tt" id="anchor2"><span class="tt-icon" tabindex="0" role="img">i</span></span>
        </div>
      </div>
    </div>
  </div>
</div>
<span class="tt-bubble glass float" id="bubble" role="tooltip" style="visibility:hidden">Значения сравнимы только внутри одной фазы. Стрелка вниз означает снижение частоты CPU — это и есть эффект схемы.</span>
<span class="tt-bubble glass float" id="bubble2" role="tooltip" style="visibility:hidden">Нижняя подсказка тоже должна стоять у своего значка, а не в углу окна.</span>
<script>
// Позиционирование подсказки целиком на JS — ровно так, как это делает
// components/Tooltip.tsx: координаты берутся у значка и задаются пузырю.
//
// Сторона выбирается ТАК ЖЕ, как в компоненте: сначала запрошенная, а если в
// ней не хватает места — противоположная. Раньше стенд знал только нижнюю
// сторону и прижимал пузырь к нижнему краю окна, поэтому на 1180×720 (где
// значок у самого низа) он «наезжал» на значок и вылезал вбок — и проверка
// писала «пузырь на 98 px ниже значка» и «упирается в край окна». Раскладка
// при этом была в порядке: расходились стенд и компонент.
window.placeTip = (anchorId, bubbleId) => {
  const a = document.getElementById(anchorId).querySelector(".tt-icon").getBoundingClientRect();
  const b = document.getElementById(bubbleId);
  b.style.visibility = "visible";
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const w = Math.min(340, Math.max(b.offsetWidth, 160), vw - 16);
  const h = b.offsetHeight;
  const GAP = 8, EDGE = 8;
  const roomBottom = vh - a.bottom;
  const roomTop = a.top;
  let side = "bottom";
  if (roomBottom < h + GAP + EDGE && roomTop > roomBottom) side = "top";
  let top = side === "bottom" ? a.bottom + GAP : a.top - h - GAP;
  if (top + h > vh - EDGE) top = vh - EDGE - h;
  if (top < EDGE) top = EDGE;
  let left = a.left;
  if (left + w > vw - EDGE) left = vw - EDGE - w;
  if (left < EDGE) left = EDGE;
  // Переходы на позиции выключены: иначе заданное значение и нарисованное
  // расходились бы на время анимации, и замер видел бы не то место, куда
  // подсказка встала.
  const prevTransition = b.style.transition;
  b.style.transition = "none";
  b.style.top = top + "px";
  b.style.left = left + "px";
  b.style.transition = prevTransition;
  return true;
};
window.measureTip = (anchorId, bubbleId) => {
  const a = document.getElementById(anchorId).querySelector(".tt-icon").getBoundingClientRect();
  const el = document.getElementById(bubbleId);
  const b = el.getBoundingClientRect();
  const cs = getComputedStyle(el);
  // Диагностика containing block: при top:0/left:0 фиксированный элемент
  // обязан лечь в (0,0). Любое смещение — значит, позиционирование
  // перехватил какой-то предок.
  //
  // Перед замером переходы выключаются: иначе getComputedStyle и
  // getBoundingClientRect() возвращают текущее значение АНИМИРУЕМОГО
  // top/left, а не заданное, и замер читает предыдущую позицию пузыря.
  const prevTop = el.style.top, prevLeft = el.style.left, prevTransition = el.style.transition;
  el.style.transition = "none";
  el.style.top = "0px";
  el.style.left = "0px";
  const origin = el.getBoundingClientRect();
  el.style.top = prevTop;
  el.style.left = prevLeft;
  el.style.transition = prevTransition;
  return {
    dy: Math.round((b.top - a.bottom) * 100) / 100,
    dx: Math.round((b.left - a.left) * 100) / 100,
    insideX: b.left >= -0.5 && b.right <= window.innerWidth + 0.5,
    insideY: b.top >= -0.5 && b.bottom <= window.innerHeight + 0.5,
    // Отступы от краёв окна: подсказка не должна упираться в границу.
    marginL: Math.round(b.left * 100) / 100,
    marginR: Math.round((window.innerWidth - b.right) * 100) / 100,
    w: Math.round(b.width), h: Math.round(b.height),
    position: cs.position,
    // Портал: пузырь — прямой потомок body. Пока это так, ни один элемент
    // страницы не может стать для него containing block.
    inBody: el.parentElement === document.body,
    originX: Math.round(origin.left), originY: Math.round(origin.top),
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
    if (url === "/heads") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return res.end(HEAD_PAGE);
    }
    if (url === "/bench") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return res.end(BENCH_PAGE);
    }
    if (url === "/results") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return res.end(RESULTS_PAGE);
    }
    if (url === "/tip") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return res.end(TIP_PAGE);
    }
    if (url === "/session") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return res.end(SESSION_MODAL_PAGE);
    }
    if (url === "/log") {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return res.end(LOG_PAGE);
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
  console.log("\n  ширина  колонка  рельса  схемы  метрики  фазы  лог  чип1  статус");
  for (const w of WIDTHS) {
    await send("Emulation.setDeviceMetricsOverride", { width: w, height: 900, deviceScaleFactor: 1, mobile: false });
    await new Promise((r) => setTimeout(r, 350));
    const m = await evalJs("window.measure()");
    const t = m.tokens;
    const p = [];
    if (m.mainOverflowX > 2) p.push(`переполнение .main ${m.mainOverflowX}`);
    if (m.bodyOverflowX > 2) p.push(`переполнение body ${m.bodyOverflowX}`);
    if (px(m.column) > 1121) p.push(`колонка ${px(m.column)}`);
    if (px(m.navGlyph) !== 20) p.push(`иконка ${m.navGlyph}`);
    if (Math.abs(px(m.padLeft) - px(t["--pad-x"])) > 0.6) p.push(`pad-x ${m.padLeft} != ${t["--pad-x"]}`);
    // Пороги из ТЗ III.3 и III.4.
    const rail = 64;
    if (px(m.railWidth) !== rail) p.push(`рельса ${px(m.railWidth)}, ожидалось ${rail}`);
    // Кнопки «Анимации» в шапке больше нет — проверять её скрытие нечего,
    // и порог 1280 px остался только за чипом железа.
    if (m.prio1 !== (w >= 1080)) p.push(`чип железа ${m.prio1 ? "виден" : "скрыт"} при ${w}`);
    const logCols = w >= 1080 ? 3 : 2;
    if (m.logCols !== logCols) p.push(`колонок в строке лога ${m.logCols}, ожидалось ${logCols}`);
    // Схемы: при минимуме карточки 310 px три колонки помещаются до 1120,
    // дальше две (ТЗ III.1 обещает 3 на 1080, но при 310 px и зазоре 9 px
    // три колонки требуют 948 px, а колонка на 1080 — 928 px; выигрывает
    // запрет «колонок больше, чем помещается»).
    const schemes = w >= 1120 ? 3 : 2;
    if (m.schemesCols !== schemes) p.push(`схем ${m.schemesCols}, ожидалось ${schemes}`);
    // Рельса теперь 64 px на всех ширинах, поэтому порог привязан к ширине
    // окна, а не к ширине рельсы: ниже 900 px метрики обязаны стать одной
    // колонкой, иначе три карточки не поместятся.
    if (w < 900 && m.kpiCols !== 1) p.push(`метрик ${m.kpiCols} на ${w}`);
    for (const [k, v] of Object.entries(EXPECT[w] || {})) {
      const got = px(t[k]);
      if (Math.abs(got - v) > TOL) p.push(`${k}=${got}, ожидалось ${v}`);
    }
    if (p.length) bad++;
    console.log(`  ${String(w).padStart(6)}  ${String(px(m.column)).padStart(7)}  ${String(px(m.railWidth)).padStart(6)}  ${String(m.schemesCols).padStart(5)}  ${String(m.kpiCols).padStart(7)}  ${String(m.phaseCols).padStart(4)}  ${String(m.logCols).padStart(3)}  ${(m.prio1 ? "да" : "нет").padStart(4)}  ${p.length ? "✗ " + p.join("; ") : "ok"}`);
  }

  // Свёрнутая рельса: иконка обязана остаться 16 px.
  await send("Emulation.setDeviceMetricsOverride", { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  const col = await evalJs("window.measure()");
  const cp = [];
  // Рельса — ровно 64 px, кнопки в ней — ровно 40×40 и по центру, иконка
  // 20 px в обоих состояниях: размеры активной и неактивных обязаны
  // совпадать, иначе белая плашка вылезает за пределы рельсы.
  if (px(col.navGlyph) !== 20) cp.push(`иконка ${col.navGlyph}`);
  if (px(col.navItem) !== 40) cp.push(`пункт ${px(col.navItem)}`);
  // Подсказка свёрнутого меню: подпись схлопнута, и пункт безымянен. Она
  // рисуется на CSS и выходит за панель вправо, поэтому пункт обязан быть
  // без `overflow:hidden` — иначе подсказка срезалась бы по краю рельсы.
  const tip = await evalJs(
    "(() => { const el = document.querySelectorAll('#rail .nav-item')[2];" +
      " const cs = getComputedStyle(el, '::after');" +
      " return { tip: el.getAttribute('data-tip'), content: cs.content, overflow: getComputedStyle(el).overflow }; })()",
  );
  if (!tip.tip) cp.push("нет data-tip");
  if (!tip.content || tip.content === '""' || tip.content === "none") cp.push("подсказка не задана");
  if (tip.overflow !== "visible") cp.push(`пункт обрезает подсказку (${tip.overflow})`);
  if (cp.length) bad++;
  console.log(
    `\n  свёрнутая рельса на 1440: иконка ${col.navGlyph}, пункт ${col.navItem}, панель ${px(col.railWidth)}, подсказка «${tip.tip}» ${cp.length ? "✗ " + cp.join("; ") : "ok"}`,
  );
  // Развёрнутая рельса: пункт во всю колонку, иконка всё так же 20 px.
  await evalJs("window.collapse(false)");
  await new Promise((r) => setTimeout(r, 400));
  const exp2 = await evalJs("window.measure()");
  const cp2 = [];
  if (px(exp2.navGlyph) !== 20) cp2.push(`иконка ${exp2.navGlyph}`);
  // «Настройки» в подвале обязана совпадать с верхними пунктами по габаритам и
  // отступам. Раньше подвал имел собственные `padding`, из-за чего активная
  // белая плашка выглядела отдельной, более крупной кнопкой, и меню
  // читалось как две несвязанные части.
  const f = exp2.footItem;
  if (f) {
    if (Math.abs(f.w - exp2.navItem) > 0.5) cp2.push(`ширина «Настроек» ${f.w} против ${px(exp2.navItem)}`);
    if (Math.abs(f.h - f.topH) > 0.5) cp2.push(`высота «Настроек» ${f.h} против ${f.topH}`);
    if (Math.abs(f.insetL - f.topInsetL) > 0.5) cp2.push(`отступ слева ${f.insetL} против ${f.topInsetL}`);
    if (Math.abs(f.gapR - f.topGapR) > 0.5) cp2.push(`отступ справа ${f.gapR} против ${f.topGapR}`);
    if (Math.abs(f.footBoxW - f.topBoxW) > 0.5) cp2.push(`блок подвала ${f.footBoxW} против списка ${f.topBoxW}`);
    if (Math.abs(parseFloat(f.padT) - parseFloat(f.padB)) > 0.5) cp2.push(`вертикальные отступы ${f.padT}/${f.padB}`);
    // Отступы блоков обязаны совпадать: свои у `.sidebar-nav` и
    // `.sidebar-foot` — источник «нижняя кнопка вылезла за верхние».
    if (f.navPadAll !== f.footPadAll) cp2.push(`padding блоков ${f.navPadAll} против ${f.footPadAll}`);
  } else {
    cp2.push("нет кнопки «Настройки» в подвале");
  }
  if (cp2.length) bad++;
  console.log(
    `  развёрнутая рельса: иконка ${exp2.navGlyph}, панель ${px(exp2.railWidth)}` +
      (f ? `, «Настройки» ${f.w}×${f.h} отступ ${f.insetL}/${f.gapR}` : "") +
      `  ${cp2.length ? "✗ " + cp2.join("; ") : "ok"}`,
  );
  await evalJs("window.collapse(true)");

// Положение содержимого на отдельной странице: и короткий, и длинный экран
  // начинаются сразу под верхним отступом `.main`. Разница между ними должна
  // быть нулевой — раньше короткий экран центрировался и прыгал на пол-окна.
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
    if (Math.abs(s.shortTop) > 1) cc.push(`${w}×${h}: короткий экран опущен на ${px(s.shortTop)} px`);
    if (Math.abs(t.tallTop) > 1) cc.push(`${w}×${h}: длинный экран опущен на ${px(t.tallTop)} px`);
    if (Math.abs(s.shortTop - t.tallTop) > 1) cc.push(`${w}×${h}: короткий и длинный экраны расходятся на ${px(s.shortTop - t.tallTop)} px`);
    if (Math.abs(s.pageTop) > 1) cc.push(`${w}×${h}: колонка смещена на ${px(s.pageTop)} px`);
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
  console.log(`\n  положение содержимого на 1440×900 и 1180×720  ${cc.length ? "✗ " + cc.join("; ") : "ok"}`);

  // Пять шапок в одной колонке: заголовок обязан стоять на одной высоте и с
  // одинаковыми метриками на всех экранах — иначе переключение вкладки
  // заметно дёргает заголовок.
  const hc = [];
  console.log("\n  заголовок на всех пяти экранах (должен совпадать до пикселя):");
  await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/heads` });
  await new Promise((r) => setTimeout(r, 1200));
  for (const [h, w] of [[900, 1920], [900, 1440], [720, 1180]]) {
    await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await new Promise((r) => setTimeout(r, 350));
    const hs = await evalJs("window.measureHeads()");
    const ids = ["p1", "p2", "p3", "p4", "p5"];
    const base = hs[ids[0]];
    for (const id of ids) {
      const m = hs[id];
      // Допуск 1 px: вёрстка кеглей даёт полупиксельные координаты, и на
      // соседних ширинах округление `--fs-h1` по-разному ложится на сетку.
      // Всё, что крупнее, — настоящий скачок макета.
      if (Math.abs(Math.round(m.top) - Math.round(base.top)) > 1) hc.push(`${w}×${h} ${id}: заголовок на ${px(m.top)} px вместо ${px(base.top)}`);
      if (m.left !== base.left) hc.push(`${w}×${h} ${id}: левый край ${px(m.left)} вместо ${px(base.left)}`);
      if (m.weight !== base.weight) hc.push(`${w}×${h} ${id}: вес ${m.weight} вместо ${base.weight}`);
      if (m.spacing !== base.spacing) hc.push(`${w}×${h} ${id}: интервал ${m.spacing} вместо ${base.spacing}`);
      if (m.headRight > m.pageRight + 1) hc.push(`${w}×${h} ${id}: шапка вылезла за колонку на ${px(m.headRight - m.pageRight)} px`);
      if (m.overflow > 2) hc.push(`${w}×${h} ${id}: переполнение .main ${m.overflow}`);
    }
    console.log(`    ${String(w).padStart(4)}×${h}: ` + ids.map((id) => `${id} ${px(hs[id].top)}`).join("  ") + `  ${base.weight} / ${base.spacing}`);
  }
  if (hc.length) bad++;
  console.log(`    ${hc.length ? "✗ " + hc.join("; ") : "совпадает"}`);

  // Слои наложения: у элементов, которые обязаны перекрывать соседей, должен
  // быть собственный контекст, иначе их `z-index` считается в корневом.
  const lc = [];
  console.log("\n  слои наложения:");
  await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/heads` });
  await new Promise((r) => setTimeout(r, 1200));
  for (const [h, w] of [[900, 1440], [720, 1180]]) {
    await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await new Promise((r) => setTimeout(r, 350));
    const lay = await evalJs("window.measureLayers()");
    const ov = await evalJs("window.measureOverlap()");
    if (lay.length) lc.push(`${w}×${h}: ${lay.join(", ")}`);
    if (ov.length) lc.push(`${w}×${h}: ${ov.join(", ")}`);
    console.log(`    ${String(w).padStart(4)}×${h}: контексты ${lay.length ? "✗ " + lay.join(", ") : "ok"}, перекрытия ${ov.length ? "✗ " + ov.join(", ") : "ok"}`);
  }
  if (lc.length) bad++;
  console.log(`    ${lc.length ? "✗ " + lc.join("; ") : "ok"}`);

  // Поля вокруг колонки меряются на общей странице стенда, поэтому сначала
  // возвращаемся на неё с изолированных страниц.
  await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/` });
  await new Promise((r) => setTimeout(r, 1400));

  // Раскладка «Бенчмарка»: шапка с этапами и полосой — наверху, блоки забирают
// свободную высоту, ничего не сжато.
await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/bench` });
await new Promise((r) => setTimeout(r, 1200));
const bc = [];
// Высоты карточек режимов, сгруппированные по ширине окна. Раньше здесь стояла
// обратная проверка (`cardHeight < 200` — плохо), она требовала растянутых
// карточек: на 1920×1080 сетка отдавала им всю высоту шага, и блок из двух
// строк текста с рядом плашек занимал пол-экрана.
const cardByWidth = new Map();
const pushH = (w, h) => {
  if (!cardByWidth.has(w)) cardByWidth.set(w, []);
  cardByWidth.get(w).push(h);
};
console.log("\n  раскладка «Бенчмарка» (шапка наверху, блоки на всю высоту):");
for (const [w, h] of [[1920, 1080], [1440, 900], [1180, 720]]) {
  await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await new Promise((r) => setTimeout(r, 300));
  for (const open of [true, false]) {
    await evalJs(`window.setBench(${open})`);
    // Ждём окончания раскрытия/схлопывания панели параметров: замер на
    // середине перехода даёт ложную обрезку (см. `window.setBench`).
    await new Promise((r) => setTimeout(r, 600));
    const b = await evalJs(`window.measureBench()`);
    const tag = open ? "раскрыта " : "свёрнута ";
    if (b.headerTop > 2) bc.push(`${w}×${h}: шапка опустилась на ${px(b.headerTop)} px`);
    if (b.squashed.length) bc.push(`${w}×${h}: сжато ${b.squashed.join(",")}`);
    if (b.overflowBottom > 2) bc.push(`${w}×${h}: низ ушёл за область на ${px(b.overflowBottom)} px`);
    // Карточка не должна ни схлопнуться, ни растянуться: сжатие срезает
    // заголовок, растяжение оставляет пустоту внутри карточки.
    if (b.cardHeight < 100) bc.push(`${w}×${h} (${tag}): карточка схлопнулась (${px(b.cardHeight)} px)`);
    if (b.cardHeight > 260) bc.push(`${w}×${h} (${tag}): карточка растянута (${px(b.cardHeight)} px)`);
    pushH(w, px(b.cardHeight));
    // Раскрытая панель забирает высоту целиком; свёрнутая ограничена потолком
    // карточек, и небольшой воздух внизу допустим.
    const limit = open ? 20 : 360;
    if (b.freeBottom > limit) bc.push(`${w}×${h} (${tag}): пустого места внизу ${px(b.freeBottom)} px`);
    console.log(`    ${String(w).padStart(4)}×${h} ${tag}: шапка +${px(b.headerTop)}  карточки ${px(b.cardHeight)}  параметры ${px(b.advHeight)}  пусто внизу ${px(b.freeBottom)}`);
    console.log(`        цепочка: host ${px(b.hostHeight)} → page ${px(b.pageHeight)} → wizard ${px(b.wizardHeight)} → stage ${px(b.stageHeight)} (доступно ${px(b.avail)}) flex=${b.chain}`);
  }
}
// Высота карточки задана контентом: растянутая карточка меняла бы высоту
// вместе с окном. Сравнивать нужно при ОДНОЙ ширине и разной высоте: при
// разной ширине описание переносится на строку больше, и сама по себе эта
// разница высот — не растяжение, а перенос текста.
let stretched = false;
const allCardHeights = [];
for (const [w, hs] of cardByWidth) {
  const spread = Math.max(...hs) - Math.min(...hs);
  allCardHeights.push(...hs);
  if (spread > 2) {
    stretched = true;
    bc.push(`карточки тянутся за высотой окна при ширине ${w}: ${Math.min(...hs)}…${Math.max(...hs)} px`);
  }
}
const lo = Math.min(...allCardHeights);
const hi = Math.max(...allCardHeights);
if (bc.length) bad++;
console.log(`    карточки режимов: ${stretched ? "растянуты" : "высота по контенту"}, ${lo}…${hi} px  ${bc.length ? "✗ " + bc.join("; ") : "ok"}`);

// Прокрутка «Логов»: лента записей обязана прокручиваться сама, шапка —
// оставаться на месте, а `.main` не прокручиваться. Иначе колесо мыши
// уезжает из ленты на страницу, а низ списка не достаётся.
console.log("\n  прокрутка лога:");
await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/log` });
await new Promise((r) => setTimeout(r, 1200));
const lcs = [];
for (const [w, h] of [[1440, 900], [1180, 720]]) {
  await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await new Promise((r) => setTimeout(r, 350));
  const lg = await evalJs("window.measureLog()");
  if (!lg.listScrolls) lcs.push(`${w}×${h}: лента не прокручивается`);
  if (lg.listOverflowY !== "auto") lcs.push(`${w}×${h}: overflow-y ленты ${lg.listOverflowY}`);
  if (lg.mainScrolls) lcs.push(`${w}×${h}: прокручивается страница вместо ленты`);
  if (lg.cardBottom > 2) lcs.push(`${w}×${h}: низ карточки ушёл на ${px(lg.cardBottom)} px за край`);
  if (Math.abs(lg.headAfter - lg.headBefore) > 1) lcs.push(`${w}×${h}: шапка лента уводит (${px(lg.headBefore)} → ${px(lg.headAfter)})`);
  console.log(`    ${String(w).padStart(4)}×${h}: лента ${px(lg.listH)} px, overflow-y ${lg.listOverflowY}, страница ${lg.mainScrolls ? "прокручивается" : "на месте"}, шапка ${Math.abs(lg.headAfter - lg.headBefore) <= 1 ? "на месте" : "уезжает"}  ${lcs.length ? "✗ " + lcs.join("; ") : "ok"}`);
}
if (lcs.length) bad++;
console.log(`    ${lcs.length ? "✗ " + lcs.join("; ") : "ok"}`);

// «Результаты»: список карточек не должен прокручиваться по горизонтали, а
// подъём карточки при наведении — срезаться верхней кромкой списка.
console.log("\n  «Результаты»: переполнение и подъём карточки:");
await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/results` });
await new Promise((r) => setTimeout(r, 1500));
const rcs = [];
for (const [w, h] of [[1440, 900], [1160, 910], [1180, 720]]) {
  await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await new Promise((r) => setTimeout(r, 400));
  const m = await evalJs("window.measureResults()");
  for (const key of ["html", "body", "main", "host", "page", "list", "card"]) {
    const b = m[key];
    if (b.overX > 2) rcs.push(`${w}×${h}: ${key} прокручивается по X на ${b.overX}`);
  }
  if (m.docScrollW > m.docClientW + 2) rcs.push(`${w}×${h}: документ шире окна (${m.docScrollW} > ${m.docClientW})`);
  // Колонка обязана лежать внутри панели: сдвиг `-рельса/2` выносил её за
  // левый край, и заголовок экрана срезался краем окна.
  if (m.page.l < m.main.l - 1) rcs.push(`${w}×${h}: колонка вылезает влево на ${px(m.main.l - m.page.l)} px`);
  if (m.page.r > m.main.r + 1) rcs.push(`${w}×${h}: колонка вылезает вправо на ${px(m.page.r - m.main.r)} px`);
  if (m.list.ovx !== "hidden") rcs.push(`${w}×${h}: overflow-x списка ${m.list.ovx}`);

  // Подъём карточки: карточка под курсором едет вверх на 2 px, и список
  // должен иметь на это место, иначе верхняя кромка срезается.
  await evalJs("window.armLift()");
  const cb = await evalJs("window.hoverCard()");
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: cb.x, y: cb.y });
  await new Promise((r) => setTimeout(r, 600));
  const lift = await evalJs("window.liftGap()");
  if (lift.gap < 0) rcs.push(`${w}×${h}: карточка срезана сверху на ${px(-lift.gap)} px`);
  if (lift.lift === 0) rcs.push(`${w}×${h}: подъём карточки не сработал`);
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: w - 30, y: h - 30 });
  await new Promise((r) => setTimeout(r, 300));
  console.log(`    ${String(w).padStart(4)}×${h}: вынос ${m.page.l - m.main.l}px, overflow-x списка ${m.list.ovx}, подъём ${lift.lift}px, запас сверху ${lift.gap}px  ${rcs.length ? "✗ " + rcs.join("; ") : "ok"}`);
}
if (rcs.length) bad++;
console.log(`    ${rcs.length ? "✗ " + rcs.join("; ") : "ok"}`);

// Модальное окно «Результат сессии»: баннер замечаний не должен съедать
// треть окна (три отдельные плашки занимали именно столько), внутренние
// блоки — без жёстких рамок, а строка «ещё N замечаний» обязана быть: без
// неё часть причин исчезла бы из окна молча.
console.log("\n  модалка «Результат сессии»:");
await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/session` });
await new Promise((r) => setTimeout(r, 1500));
const mcs2 = [];
for (const [w, h] of [[1440, 900], [1180, 720]]) {
  await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await new Promise((r) => setTimeout(r, 400));
  const s = await evalJs("window.measureSession()");
  const share = (s.alertsH / s.bodyH) * 100;
  if (share > 22) mcs2.push(`${w}×${h}: баннер замечаний ${px(share)} % тела (порог 22 %)`);
  if (!s.hasMore) mcs2.push(`${w}×${h}: нет строки «ещё N замечаний»`);
  // Внутренние блоки держатся на заливке и разделителях, а не на рамках:
  // рамка 1 px вокруг лидера, метрик и баннера — это ровно тот визуальный шум,
  // который окно полировкой снимало.
  for (const name of ["hero", "alerts", "chip"]) {
    const bw = (s.borders[name] || "").split(" ").map(parseFloat);
    if (bw.some((v) => v > 0.01)) mcs2.push(`${w}×${h}: у .${name} есть рамка ${s.borders[name]}`);
  }
  console.log(`    ${String(w).padStart(4)}×${h}: окно ${s.modalH}, тело ${s.bodyH}, замечания ${s.alertsH} (${px(share)} %), лидер ${s.heroH}, сравнение ${s.compareH}  ${mcs2.length ? "✗" : "ok"}`);
}
// Правило «сравнивать не с чем»: при одной замере схеме таблица не выводится.
// Саму условную логику проверяет код, а стенд — что блок уходит ЦЕЛИКОМ, а не
// наполовину: освободиться должно больше, чем занимает один только тулбар
// секции. Иначе «скрытие» оставило бы шапку с поиском и сортировкой висеть
// над пустотой — то есть ровно тот визуальный мусор, который правило убирает.
await evalJs("window.setCompare(false)");
await new Promise((r) => setTimeout(r, 300));
const without = await evalJs("window.measureSession()");
await evalJs("window.setCompare(true)");
await new Promise((r) => setTimeout(r, 300));
const withTbl = await evalJs("window.measureSession()");
const freed = withTbl.bodyH - without.bodyH;
if (freed < withTbl.toolbarH) {
  mcs2.push(`скрытие таблицы освободило ${px(freed)} px — меньше одного тулбара (${px(withTbl.toolbarH)} px)`);
}
console.log(`    скрытие блока при одной схеме: +${px(freed)} px тела (тулбар занимает ${px(withTbl.toolbarH)} px)  ${freed >= withTbl.toolbarH ? "ok" : "✗"}`);
if (mcs2.length) bad++;
console.log(`    ${mcs2.length ? "✗ " + mcs2.join("; ") : "ok"}`);

// Снимок окна — чтобы правку баннера и метрик можно было посмотреть глазами,
// а не только по числам. Пишется в TEMP, в репозиторий не попадает.
if (process.env.PB_SHOT) {
  try {
    const shot = await send("Page.captureScreenshot", { format: "png" });
    require("fs").writeFileSync(path.join(OUT, "session-modal.png"), Buffer.from(shot.result.data, "base64"));
    console.log(`    снимок: ${path.join(OUT, "session-modal.png")}`);
  } catch (e) {
    console.log("    снимок не сделан:", e.message);
  }
}

// Подсказка внутри модалки: главный регресс этой правки. Подсказка обязана
// стоять у своего значка и быть порталом в `body` — иначе она снова уедет в
// угол, как только предок получит `transform` или `contain`.
console.log("\n  подсказка внутри модалки:");
await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/tip` });
await new Promise((r) => setTimeout(r, 1500));
const tps = [];
for (const [w, h] of [[1440, 900], [1180, 720]]) {
  await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: 1, mobile: false });
  await new Promise((r) => setTimeout(r, 400));
  // Перезагрузка после смены размера окна — обязательна именно здесь.
  // `setDeviceMetricsOverride` меняет окно, но НЕ переносит уже
  // отрисовавшиеся элементы `position: fixed`: пузырь оставался на старом
  // месте (981/468 при новом `window.innerWidth` 1180), хотя его inline-стиль
  // уже был верным. Замер читал именно старый прямоугольник и сообщал
  // «пузырь на 98 px ниже значка» и «упирается в край окна». Остальные
  // разделы проверки так не страдают: там нет фиксированных элементов,
  // положение которых переносилось бы при смене окна.
  await send("Page.reload");
  await new Promise((r) => setTimeout(r, 900));
  for (const [a, b] of [["anchor", "bubble"], ["anchor2", "bubble2"]]) {
    await evalJs(`window.placeTip(${JSON.stringify(a)}, ${JSON.stringify(b)})`);
    const t = await evalJs(`window.measureTip(${JSON.stringify(a)}, ${JSON.stringify(b)})`);
    if (t.dy < 6 || t.dy > 10) tps.push(`${w}×${h} ${a}: пузырь на ${t.dy} px ниже значка (ожидалось 8)`);
    // По горизонтали пузырь у значка только когда помещается справа; у правого
    // края окна он прижимается к нему — это и есть штатная прижимка, а не
    // ошибка. Поэтому проверяем не сдвиг, а результат: пузырь целиком в окне и
    // не упирается в его границы.
    if (!t.insideX || !t.insideY) tps.push(`${w}×${h} ${a}: пузырь вышел за окно`);
    if (t.marginL < 7.5 || t.marginR < 7.5) tps.push(`${w}×${h} ${a}: пузырь упирается в край окна (${t.marginL}/${t.marginR})`);
    if (t.position !== "fixed") tps.push(`${w}×${h} ${a}: position ${t.position}, ожидался fixed`);
    if (!t.inBody) tps.push(`${w}×${h} ${a}: пузырь не портал в body — позиционирование снова могут перехватить`);
    if (t.originX !== 0 || t.originY !== 0) tps.push(`${w}×${h} ${a}: containing block со смещением (${t.originX},${t.originY})`);
    console.log(`    ${String(w).padStart(4)}×${h} ${a}: сдвиг ${t.dx}/${t.dy} px, ${t.w}×${t.h}, начало ${t.originX}/${t.originY}, портал ${t.inBody ? "да" : "нет"}`);
  }
}
if (tps.length) bad++;
console.log(`    ${tps.length ? "✗ " + tps.join("; ") : "ok"}`);

// Ловушка, из-за которой подсказка уезжала в угол, стоит в самом CSS:
// `fill-mode: forwards|both` сохраняет последний кадр анимации, и если в нём
// `transform` НЕ `none`, элемент навсегда остаётся containing block для
// `position: fixed` потомков. Проверяем по тексту собранного CSS: у каждого
// keyframes, который где-либо играет с удержанием, последний кадр обязан
// заканчиваться на `transform: none` (или вообще без transform).
const cssText = fs.readFileSync(path.join(BUILD, "assets", cssHref), "utf8");
const kf = {};
const kfRe = /@keyframes\s+([\w-]+)\s*\{/g;
let m2;
while ((m2 = kfRe.exec(cssText))) kf[m2[1]] = m2.index;
const holders = new Set();
for (const mm of cssText.matchAll(/animation(?:-name)?\s*:\s*([^;}]+)/g)) {
  const v = mm[1];
  if (!/\b(forwards|both)\b/.test(v)) continue;
  for (const name of v.matchAll(/([\w-]+)/g)) {
    const n = name[1];
    if (kf[n] !== undefined) holders.add(n);
  }
}
const badKf = [];
// Опасна только УДЕРЖИВАЕМАЯ анимация, которая заканчивается ВИДИМОЙ:
// элемент остаётся в потоке и продолжает быть containing block для
// position:fixed потомков. Выходные кадры ( Ripple, уезжающий тост и
// уходящая вкладка) заканчиваются `opacity: 0` — их элемент либо убирается из
// DOM, либо прозрачен, и перехват позиционирования через него невозможен.
// Именно так выглядел баг с подсказкой: `.modal` играл `pb-pop-in` с
// `both`, последний кадр был `opacity: 1; transform: scale(1)`.
for (const name of holders) {
  // Тело keyframes вырезается по балансу фигурных скобок: вложенных блоков
  // внутри keyframes не бывает.
  const start = kf[name];
  const rest = cssText.slice(start);
  const open = rest.indexOf("{");
  let depth = 0, end = open;
  for (let i = open; i < rest.length; i++) {
    if (rest[i] === "{") depth++;
    else if (rest[i] === "}") {
      depth--;
      if (depth === 0) { end = i; break; }
    }
  }
  const body = rest.slice(open + 1, end);
  // Последний ключевой кадр: `to{...}` либо `100%{...}`.
  const blocks = [...body.matchAll(/([\w%]+)\s*\{([^}]*)\}/g)];
  if (!blocks.length) continue;
  const last = blocks[blocks.length - 1][2];
  const tf = last.match(/transform\s*:\s*([^;}]+)/);
  if (!tf) continue;
  const val = tf[1].trim();
  if (val === "none") continue;
  const op = last.match(/opacity\s*:\s*([^;}]+)/);
  const visible = !op || parseFloat(op[1]) > 0.01;
  if (visible) badKf.push(`${name} (последний кадр transform: ${val}, opacity ${op ? op[1].trim() : "1"})`);
}
if (badKf.length) {
  bad++;
  console.log(`\n  удержание анимации оставляет containing block: ${badKf.join("; ")}`);
  console.log(`    эти keyframes играют с forwards/both и заканчиваются не на transform:none —`);
  console.log(`    любой position:fixed потомок внутри них уедет на смещение предка.`);
} else {
  console.log(`\n  удерживаемых анимаций: ${holders.size}, ни одна не оставляет containing block  ok`);
}

await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/` });
await new Promise((r) => setTimeout(r, 1200));

await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/` });
await new Promise((r) => setTimeout(r, 1200));
  // Симметрия колонки и отсутствие горизонтального выноса.
  //
  // Проверяется симметрия ВНУТРИ панели `.main`, а не относительно окна.
  // Раньше колонка сдвигалась на половину рельсы, чтобы её центр совпал с
  // центром окна; с отдельными панелями такой сдвиг выносил колонку за левый
  // край панели (замер: `.main` с 84 px, `.page` с −3 px), заголовок срезался
  // краем окна, а вынос давал горизонтальную прокрутку. Теперь колонка
  // центрируется внутри своей панели, и симметрия считается от её краёв.
  console.log("  поля вокруг колонки (панель):");
  const gp = [];
  for (const rail of [true, false]) {
    await evalJs(`window.collapse(${rail})`);
    await new Promise((r) => setTimeout(r, 450));
    const label = rail ? "свёрнута" : "развёрнута";
    for (const w of [1920, 1440, 1280, 1080]) {
      await send("Emulation.setDeviceMetricsOverride", { width: w, height: 900, deviceScaleFactor: 1, mobile: false });
      await new Promise((r) => setTimeout(r, 300));
      const g = await evalJs("window.measure()");
      const mainW = px(g.mainRight - g.mainLeft);
      // Поля колонки внутри ПОЛЕЗНОЙ области панели (без глоба под полосу).
      const gapL = px(g.columnLeft - g.contentLeft);
      const gapR = px(g.contentRight - (g.columnLeft + g.column));
      const diff = gapR - gapL;
      // Колонка во всю область не центрируется — тогда поля равны нулю.
      const fits = g.column < mainW - 300;
      if (fits && Math.abs(diff) > 1) gp.push(`${label} ${w}: разница полей ${px(diff)} px`);
      // Колонка обязана целиком лежать внутри панели: иначе она срезается.
      if (gapL < -1) gp.push(`${label} ${w}: колонка вылезает влево на ${px(-gapL)} px`);
      if (g.mainOverflowX > 2) gp.push(`${label} ${w}: панель прокручивается по X на ${g.mainOverflowX}`);
      if (g.bodyOverflowX > 2) gp.push(`${label} ${w}: документ прокручивается по X на ${g.bodyOverflowX}`);
      console.log(`    ${label.padEnd(11)} ${String(w).padStart(4)}: слева ${gapL}  справа ${gapR}  разница ${String(px(diff)).padStart(5)}  колонка ${px(g.column)}  панель ${mainW}  вынос ${g.mainOverflowX}/${g.bodyOverflowX}`);
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
      if (px(m.navGlyph) !== 20) p.push(`иконка ${m.navGlyph}`);
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
