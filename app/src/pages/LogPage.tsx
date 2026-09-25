// Страница «Логи»: старт приложения, ошибки и события сессий.

import { useEffect, useMemo, useState } from "react";
import { commands, fmtTime, onLog, type LoggerEntry } from "../api";
import { Badge, Panel, Seg } from "../components/ui";
import { SearchIcon } from "../components/icons";

type Level = "all" | "info" | "success" | "warn" | "error";

const LEVEL_SHORT: Record<string, string> = {
  info: "инфо",
  success: "успех",
  warn: "вним.",
  error: "ошиб.",
};

export default function LogPage() {
  const [entries, setEntries] = useState<LoggerEntry[] | null>(null);
  const [live, setLive] = useState<LoggerEntry[]>([]);
  const [query, setQuery] = useState("");
  const [level, setLevel] = useState<Level>("all");

  useEffect(() => {
    commands.logHistory().then(setEntries).catch(() => setEntries([]));
    const un = onLog((m) =>
      setLive((prev) => [...prev.slice(-200), { level: m.level, text: m.text, ts_ms: m.ts_ms }]),
    );
    return () => {
      un.then((f) => f());
    };
  }, []);

  const all = useMemo(
    () => [...(entries ?? []), ...live].sort((a, b) => a.ts_ms - b.ts_ms),
    [entries, live],
  );

  const counts = useMemo(() => {
    const c = { all: all.length, info: 0, success: 0, warn: 0, error: 0, other: 0 };
    for (const e of all) {
      const k = e.level === "warning" ? "warn" : e.level === "err" ? "error" : e.level;
      if (k === "info") c.info++;
      else if (k === "success") c.success++;
      else if (k === "warn") c.warn++;
      else if (k === "error") c.error++;
      else c.other++;
    }
    return c;
  }, [all]);

  const norm = (l: string) => (l === "warning" ? "warn" : l === "err" ? "error" : l);

  const shown = all.filter((e) => {
    if (level !== "all" && norm(e.level) !== level) return false;
    const q = query.trim().toLowerCase();
    if (q && !e.text.toLowerCase().includes(q)) return false;
    return true;
  });

  return (
    <div className="page">
      <div className="page-head">
        <h1>Логи</h1>
        <span className="sub">старт приложения, ошибки и события сессий</span>
      </div>
      <div className="wizard-toolbar">
        <div className="search-box log-search">
          <SearchIcon />
          <input
            className="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Поиск по тексту…"
          />
        </div>
        <Seg
          options={[
            { value: "all", label: "Все" },
            { value: "info", label: "Инфо" },
            { value: "success", label: "Успех" },
            { value: "warn", label: "Внимание" },
            { value: "error", label: "Ошибки" },
          ]}
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
      <Panel
        title="Журнал"
        hint={entries ? `показаны последние ${shown.length} записей — с фильтром обновляются` : undefined}
      >
        <div className={`log log-box${entries ? " fade-bottom" : ""}`}>
          {!entries ? (
            <div className="muted">Загрузка журнала…</div>
          ) : shown.length === 0 ? (
            <div className="muted">Журнал пуст. Тест или событие — и оно появится здесь.</div>
          ) : (
            shown.map((e, i) => (
              <div key={`${e.ts_ms}-${i}`} className={`log-line ${norm(e.level)}`}>
                <span className="ts">{fmtTime(e.ts_ms)}</span>
                <span className="lv">{LEVEL_SHORT[norm(e.level)] ?? e.level}</span>
                <span className="tx">{e.text}</span>
              </div>
            ))
          )}
        </div>
      </Panel>
    </div>
  );
}
