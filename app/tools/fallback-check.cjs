// Фолбэк без контейнерных запросов (Приложение C.2): тот же замер, но блоки
// `@container` вырезаны, а `@supports not (container-type: inline-size)` раскрыт
// в `@media all` — так эмулируется старый WebView2.
//
// Запуск: `npm run check:fallback` (после `npm run build`).
const { spawn } = require("child_process");
const http = require("http");
const path = require("path");
const fs = require("fs");

const APP = path.resolve(__dirname, "..");
const BUILD = path.join(APP, "build-ui");
const OUT = path.join(process.env.TEMP || process.env.TMPDIR || APP, "powerbench-adaptive");
const PORT = 8733;
const WIDTHS = [1920, 1440, 1280, 1080, 1020, 900, 860];
const px = (v) => Math.round(parseFloat(v) * 100) / 100;
const get = (url) => new Promise((res, rej) => {
  http.get(url, (r) => { let d = ""; r.on("data", (c) => (d += c)); r.on("end", () => res(d)); }).on("error", rej);
});

const readCss = () => {
  const indexHtml = fs.readFileSync(path.join(BUILD, "index.html"), "utf8");
  const href = (indexHtml.match(/href="\/assets\/([^"]+\.css)"/) || [])[1];
  return fs.readFileSync(path.join(BUILD, "assets", href), "utf8");
};

/** Удаляет блок, начинающийся с `marker`, по балансу скобок. */
function dropBlock(css, marker, from = 0) {
  const i = css.indexOf(marker, from);
  if (i < 0) return { css, next: -1 };
  const open = css.indexOf("{", i);
  let depth = 0;
  for (let j = open; j < css.length; j++) {
    if (css[j] === "{") depth++;
    else if (css[j] === "}") { depth--; if (depth === 0) return { css: css.slice(0, i) + css.slice(j + 1), next: j + 1 }; }
  }
  return { css, next: -1 };
}

async function main() {
  const css = readCss();
  // Эмуляция движка без контейнерных запросов. Подменить только
  // `container-type` нельзя: `@supports` спрашивает возможности ДВИЖКА, а не
  // наличие объявления, поэтому в Chromium фолбэк не включится никогда.
  // Нужно убрать сами блоки `@container` и раскрыть `@supports not (...)`,
  // заменив его на `@media all` (его содержимое тогда применяется всегда).
  let patched = css.replace(/@supports not \(container-type: inline-size\)/, "@media all");
  let guard = 0;
  for (;;) {
    const r = dropBlock(patched, "@container app", 0);
    if (r.next < 0) break;
    patched = r.css;
    if (++guard > 20) { console.log("слишком много блоков @container"); return; }
  }
  const hasContainer = /@container/.test(patched);
  const hasFallback = /@media all/.test(patched);
  console.log("блоков @container осталось:", hasContainer, "| фолбэк раскрыт:", hasFallback);
  if (hasContainer || !hasFallback) { console.log("не удалось подготовить фолбэк"); return; }
  console.log("container-type отключён, css:", (patched.length / 1024).toFixed(0), "КБ");

  const page = `<!DOCTYPE html><html lang="ru" data-theme="Graphite" data-mode="Dark" data-density="normal" data-text="m">
<meta charset="utf-8"><link rel="stylesheet" href="/patched.css">
<style>html,body{margin:0;height:100%}</style>
<body>
<div class="shell">
  <div class="titlebar"><div class="titlebar-chips"><span class="titlebar-chip on">Тихий режим</span><span class="titlebar-chip trunc" data-prio="1">Ryzen 9 7950X · 32 ГБ</span></div><div class="titlebar-controls"><button class="titlebar-toggle" data-prio="2">Анимации</button></div></div>
  <div class="body">
    <aside class="sidebar collapsed" id="rail"><nav class="sidebar-nav"><button class="nav-item active"><span class="nav-glyph"><svg viewBox="0 0 24 24"></svg></span><span class="nav-label">Бенчмарк</span></button></nav></aside>
    <main class="main"><div class="page">
      <div class="schemes-grid" id="schemes">${Array.from({ length: 12 }, () => '<div class="scheme-card">Схема</div>').join("")}</div>
      <div class="log-list" id="logs"><div class="log-row"><span class="l-time">14:19:02</span><span class="l-badge success">Успех</span><span class="l-msg">Запуск фазы</span></div></div>
    </div></main>
  </div>
</div>
<script>
window.measure = () => {
  const cs = getComputedStyle(document.documentElement);
  const main = document.querySelector(".main");
  const cols = (s) => getComputedStyle(document.querySelector(s)).gridTemplateColumns.split(" ").filter(Boolean).length;
  const shown = (s) => getComputedStyle(document.querySelector(s)).display !== "none";
  const probe = document.createElement("i");
  probe.style.cssText = "position:absolute;left:-9999px;font-size:var(--fs-h1)";
  document.body.appendChild(probe);
  const h1 = getComputedStyle(probe).fontSize;
  const probe2 = document.createElement("i");
  probe2.style.cssText = "position:absolute;left:-9999px;padding-left:var(--pad-x)";
  document.body.appendChild(probe2);
  const pad = getComputedStyle(probe2).paddingLeft;
  return {
    h1, pad,
    rail: document.getElementById("rail").getBoundingClientRect().width,
    schemes: cols("#schemes"),
    logCols: getComputedStyle(document.querySelector(".log-row")).gridTemplateColumns.split(" ").filter(Boolean).length,
    prio1: shown('[data-prio="1"]'),
    prio2: shown('[data-prio="2"]'),
    overflowX: main.scrollWidth - main.clientWidth,
  };
};
</script></body></html>`;

  const server = http.createServer((req, res) => {
    const url = req.url.split("?")[0];
    if (url === "/patched.css") {
      res.writeHead(200, { "content-type": "text/css" });
      return res.end(patched);
    }
    if (url === "/") { res.writeHead(200, { "content-type": "text/html; charset=utf-8" }); return res.end(page); }
    res.writeHead(404); res.end("нет");
  });
  await new Promise((r) => server.listen(PORT, r));

  const local = process.env.LOCALAPPDATA || "";
  const candidates = [
    path.join(local, "Chromium", "Application", "chrome.exe"),
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  ].filter((p) => fs.existsSync(p));
  if (!candidates.length) { console.log("нет браузера"); server.close(); return; }
  const proc = spawn(candidates[0], [
    "--headless=new", "--remote-debugging-port=9334", "--disable-gpu", "--no-first-run",
    "--user-data-dir=" + path.join(OUT, "chrome-fallback"), "about:blank",
  ], { stdio: "ignore" });
  const cleanup = () => { try { proc.kill(); } catch {} server.close(); };
  process.on("exit", cleanup);

  let target = null;
  for (let i = 0; i < 60 && !target; i++) {
    await new Promise((r) => setTimeout(r, 500));
    try {
      const list = JSON.parse(await get("http://127.0.0.1:9334/json/list"));
      target = list.find((t) => t.type === "page");
    } catch {}
  }
  if (!target) { console.log("CDP не поднялся"); cleanup(); return; }
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((res) => ws.addEventListener("open", res, { once: true }));
  let id = 0; const pending = new Map();
  ws.addEventListener("message", (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  });
  const send = (method, params) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
  const evalJs = async (e) => (await send("Runtime.evaluate", { expression: e, returnByValue: true })).result.result.value;

  await send("Page.enable");
  await send("Page.navigate", { url: `http://127.0.0.1:${PORT}/` });
  await new Promise((r) => setTimeout(r, 2000));

  let bad = 0;
  console.log("\n  ширина  h1     pad-x   рельса  схемы  лог  чип1  чип2  статус");
  for (const w of WIDTHS) {
    await send("Emulation.setDeviceMetricsOverride", { width: w, height: 900, deviceScaleFactor: 1, mobile: false });
    await new Promise((r) => setTimeout(r, 300));
    const m = await evalJs("window.measure()");
    const p = [];
    if (m.overflowX > 2) p.push(`переполнение ${m.overflowX}`);
    if (px(m.h1) < 18.5 || px(m.h1) > 25) p.push(`h1 ${m.h1}`);
    if (m.prio2 !== (w >= 1280)) p.push(`чип2 ${m.prio2 ? "виден" : "скрыт"}`);
    if (m.prio1 !== (w >= 1080)) p.push(`чип1 ${m.prio1 ? "виден" : "скрыт"}`);
    const rail = w >= 1080 ? 64 : w >= 900 ? 56 : 52;
    // Полпикселя: ширина рельсы — дробная (flex-basis + дробные отступы),
    // и без допуска ровное 64 приходило как 63.98.
    if (Math.abs(px(m.rail) - rail) > 0.5) p.push(`рельса ${px(m.rail)} != ${rail}`);
    if (p.length) bad++;
    console.log(`  ${String(w).padStart(6)}  ${String(px(m.h1)).padEnd(6)}  ${String(px(m.pad)).padEnd(6)}  ${String(px(m.rail)).padStart(6)}  ${String(m.schemes).padStart(5)}  ${String(m.logCols).padStart(3)}  ${(m.prio1 ? "да" : "нет").padStart(4)}  ${(m.prio2 ? "да" : "нет").padStart(4)}  ${p.length ? "✗ " + p.join("; ") : "ok"}`);
  }
  console.log(bad === 0 ? "\nфолбэк: работает" : `\nфолбэк: расхождений ${bad}`);
  cleanup();
  process.exit(bad === 0 ? 0 : 1);
}
main().catch((e) => { console.log("ошибка:", e.message); process.exit(1); });