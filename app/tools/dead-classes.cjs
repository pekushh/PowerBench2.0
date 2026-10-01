// Аудит мёртвых классов: объявлены в styles.css, но не встречаются в разметке.
//
// Класс может собираться динамически (`badge ${kind}`, `is-${tone}`), поэтому
// значения, приходящие из данных, перечислены в DYNAMIC. Всё остальное, чего
// нет ни в одном .tsx/.ts, — кандидат на удаление. `woff2` попадает в список
// из расширений в `url(/fonts/*.woff2)`, а не из селектора, и пропускается.
//
// Запуск: node tools/dead-classes.cjs
// Выход: 0 — пусто или остался только `woff2`, иначе 1.
const fs = require("fs");
const path = require("path");

const SRC = path.resolve(__dirname, "..", "src");
const css = fs
  .readFileSync(path.join(SRC, "styles.css"), "utf8")
  .replace(/\/\*[\s\S]*?\*\//g, "")
  // `url()` и `@font-face` не содержат селекторов.
  .replace(/url\([^)]*\)/g, "")
  .replace(/@font-face\s*\{[\s\S]*?\}/g, "");

const classes = new Set();
for (const m of css.matchAll(/\.(-?[_a-zA-Z][_a-zA-Z0-9-]*)/g)) classes.add(m[1]);

const files = [];
(function walk(dir) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p);
    else if (/\.(tsx?|jsx?)$/.test(e.name)) files.push(p);
  }
})(SRC);

let blob = "";
for (const f of files) blob += fs.readFileSync(f, "utf8") + "\n";

// Только значения, приходящие из данных или вычисляемые. Имена классов,
// написанные в разметке прямо, сюда НЕ входят: для них проверка точная.
const DYNAMIC = new Set([
  "ok", "warn", "error", "info", "plain", "accent", "success",
  "is-ok", "is-bad", "is-in", "is-on", "is-instant",
  "dup", "orig", "active", "done", "selected", "checked", "open", "back",
  "centered", "top", "left", "right", "collapsing", "collapsed", "leaving",
  "spot-on", "pb-cascade", "pb-pop-list", "pb-ripple", "pb-nav-ind",
  "st-badge", "skipped", "rejected", "leader",
  "on-fav", "on-ex", "on-del", "ts-act",
  "list-mode", "off", "on", "bad", "edited", "sys", "ex", "empty",
]);

const dead = [];
for (const c of [...classes].sort()) {
  if (DYNAMIC.has(c)) continue;
  const esc = c.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp("(^|[^A-Za-z0-9_-])" + esc + "([^A-Za-z0-9_-]|$)");
  if (!re.test(blob)) dead.push(c);
}

console.log("классов в CSS:", classes.size);
console.log("не встречаются в разметке:", dead.length);
for (const c of dead) console.log("  " + c);
process.exit(dead.length ? 1 : 0);