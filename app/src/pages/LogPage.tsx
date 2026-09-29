// Страница «Логи»: фильтруемый журнал запуска, ошибок и событий сессий.

import { useEffect, useMemo, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { commands, onLog, type LoggerEntry } from "../api";
import { Badge, Button, FadeScroll, Panel, Seg } from "../components/ui";
import { SearchIcon } from "../components/icons";
import { pushToast } from "../store";

type Level = "all" | "info" | "success" | "warn" | "error";

/** Сколько строк держим в «живом» буфере: больше уже не прочитать глазом. */
const LIVE_CAP = 200;
/** Сколько записей реально рисуем: журнал хранит тысячи, DOM — нет. */
const RENDER_CAP = 1000;

const LEVEL_SHORT: Record<string, string> = {
  info: "инфо",
  success: "успех",
  warn: "внимание",
  error: "ошибка",
};

const LEVEL_LABEL: Record<Level, string> = {
  all: "Все",
  info: "Инфо",
  success: "Успех",
  warn: "Внимание",
  error: "Ошибки",
};

const LEVEL_OPTIONS: Level[] = ["all", "info", "success", "warn", "error"];

/** Привести уровень из журнала к одному из четырёх отображаемых. */
function normLevel(raw: string): string {
  if (raw === "warning" || raw === "warn") return "warn";
  if (raw === "err" || raw === "error") return "error";
  if (raw === "ok" || raw === "success") return "success";
  if (raw === "info" || raw === "") return "info";
  return "info";
}

/** Строка журнала со стабильным ключом: индекс в отсортированном списке ключом быть не может. */
type LogRow = LoggerEntry & { key: string };

function timeOf(ts: number): string {
  if (!Number.isFinite(ts) || ts < 0) return "--:--:--";
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** Русские склонения: 1 участник / 2 участника / 5 участников. */
function plural(n: number, one: string, few: string, many: string): string {
  const a = Math.abs(n) % 100;
  const b = a % 10;
  if (a > 10 && a < 20) return many;
  if (b > 1 && b < 5) return few;
  if (b === 1) return one;
  return many;
}

export default function LogPage() {
  const [entries, setEntries] = useState<LogRow[] | null>(null);
  const [live, setLive] = useState<LogRow[]>([]);
  const [query, setQuery] = useState("");
  const [level, setLevel] = useState<Level>("all");
  const [saving, setSaving] = useState(false);
  // По умолчанию скрываем: отчёт заведомо уходит вовне (в чат, в issues), а
  // логин и путь `C:\Users\…` для разбора ошибки ничего не дают.
  const [redact, setRedact] = useState(true);
  const nextKey = useRef(0);

  useEffect(() => {
    let alive = true;
    commands
      .logHistory()
      .then((rows) => {
        if (!alive) return;
        nextKey.current = rows.length;
        setEntries(
          rows.map((e, i) => ({ ...e, key: `h${i}` })),
        );
      })
      .catch(() => {
        if (alive) setEntries([]);
      });
    const un = onLog((m) => {
      const row: LogRow = { ...m, key: `l${nextKey.current++}` };
      setLive((prev) => [...prev.slice(-LIVE_CAP), row]);
    });
    return () => {
      alive = false;
      un.then((f) => f()).catch(() => undefined);
    };
  }, []);

  const all = useMemo(
    () =>
      [...(entries ?? []), ...live].sort(
        (a, b) => a.ts_ms - b.ts_ms || (a.key < b.key ? -1 : 1),
      ),
    [entries, live],
  );

  const counts = useMemo(() => {
    const c = { all: all.length, info: 0, success: 0, warn: 0, error: 0 };
    for (const e of all) {
      const k = normLevel(e.level);
      if (k === "info") c.info++;
      else if (k === "success") c.success++;
      else if (k === "warn") c.warn++;
      else if (k === "error") c.error++;
    }
    return c;
  }, [all]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return all.filter((e) => {
      if (level !== "all" && normLevel(e.level) !== level) return false;
      if (q && !e.text.toLowerCase().includes(q)) return false;
      return true;
    });
  }, [all, level, query]);

  // Длинный журнал показываем с конца — свежие записи важнее, а DOM остаётся
  // ограниченным независимо от того, сколько сессий накопилось.
  const visible = shown.length > RENDER_CAP ? shown.slice(-RENDER_CAP) : shown;
  const hidden = shown.length - visible.length;

  // Сохранение отчёта для поддержки. Диалог спрашивает только путь: сам
  // отчёт собирает бэкенд, и он берёт данные из всех источников разом —
  // журнал, окружение, контрольную точку, карантин, настройки и последнюю
  // сессию. Собирать это на стороне интерфейса означало бы половину работы
  // делать вслепую и половину данных вообще не увидеть.
  const saveReport = async () => {
    if (saving) return;
    try {
      const suggested = await commands.diagnosticsFileName();
      const path = await save({
        title: "Сохранить отчёт для поддержки",
        defaultPath: suggested,
        filters: [{ name: "Текстовый отчёт (.txt)", extensions: ["txt"] }],
      });
      if (!path) return;
      setSaving(true);
      const res = await commands.saveDiagnostics(path as string, redact);
      pushToast(
        "okk",
        `Отчёт сохранён: ${res.lines} строк, замечаний: ${res.findings}. ` +
          "Приложите этот файл к обращению.",
      );
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="page fill">
      <div className="page-head">
        <h1>Логи</h1>
      </div>
      <div className="wizard-toolbar">
        <div className="search-box log-search">
          <SearchIcon />
          <input
            className="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Поиск по тексту…"
            aria-label="Поиск по тексту записи журнала"
          />
        </div>
        <Seg
          options={LEVEL_OPTIONS.map((v) => ({ value: v, label: LEVEL_LABEL[v] }))}
          value={level}
          onChange={setLevel}
        />
        <div className="spacer" />
        <div className="log-counts">
          <Badge kind="plain">{counts.all} всего</Badge>
          <Badge kind="ok">{counts.success} успех</Badge>
          <Badge kind="plain">{counts.info} инфо</Badge>
          <Badge kind="warn">{counts.warn} вним.</Badge>
          <Badge kind="danger">{counts.error} ошиб.</Badge>
        </div>
      </div>
      <div className="wizard-toolbar">
        <Button
          variant="primary"
          onClick={saveReport}
          disabled={saving}
          title="Сохранить один файл: журнал, окружение, состояние данных и последнюю сессию"
        >
          {saving ? "Готовлю отчёт…" : "Сохранить отчёт для поддержки"}
        </Button>
        <label className="check">
          <input
            type="checkbox"
            checked={redact}
            onChange={(e) => setRedact(e.target.checked)}
          />
          <span>Скрыть имя пользователя и путь к профилю</span>
        </label>
        <div className="hint">
          В отчёте: журнал, окружение, идентичность замера, состояние
          контрольной точки, карантина и настроек, последняя сессия и список
          найденных проблем. Собирайте его сразу после ошибки.
        </div>
      </div>
      <Panel
        title="Журнал"
        hint={
          entries === null
            ? undefined
            : `показано ${visible.length} из ${shown.length} ${plural(
                shown.length,
                "записи",
                "записей",
                "записей",
              )} по текущему фильтру`
        }
        className="fill-grow"
      >
        <FadeScroll className="log log-box">
          {entries === null ? (
            <div className="muted">Загрузка журнала…</div>
          ) : visible.length === 0 ? (
            <div className="muted">
              {all.length === 0
                ? "Журнал пуст. Здесь появятся события тестов и ошибки приложения."
                : "Ни одна запись не подходит под фильтр."}
            </div>
          ) : (
            visible.map((e) => (
              <div key={e.key} className={`log-line ${normLevel(e.level)}`}>
                <span className="ts">{timeOf(e.ts_ms)}</span>
                <span className="lv">{LEVEL_SHORT[normLevel(e.level)] ?? e.level}</span>
                <span className="tx">{e.text}</span>
              </div>
            ))
          )}
        </FadeScroll>
      </Panel>
      {hidden > 0 ? (
        <div className="hint" style={{ marginTop: 8 }}>
          Ещё {hidden} {plural(hidden, "запись", "записи", "записей")} выше не показаны —
          сузьте фильтр или поиск.
        </div>
      ) : null}
    </div>
  );
}
