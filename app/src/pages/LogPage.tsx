// Страница «Журнал»: полный живой лог событий сессии с фильтрами по уровню.

import { useEffect, useRef, useState } from "react";
import { fmtTime, onLog, type LogMsg } from "../api";
import { Button, Glass, Seg } from "../components/ui";

type LevelFilter = "all" | "info" | "warn" | "success";

export default function LogPage() {
  const [entries, setEntries] = useState<LogMsg[]>([]);
  const [filter, setFilter] = useState<LevelFilter>("all");
  const [autoScroll, setAutoScroll] = useState(true);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const unfor = onLog((m) => setEntries((e) => [...e.slice(-999), m]));
    return () => {
      unfor.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (autoScroll && boxRef.current) {
      boxRef.current.scrollTop = boxRef.current.scrollHeight;
    }
  }, [entries, autoScroll]);

  const visible = entries.filter((e) => filter === "all" || e.level === filter);

  return (
    <div className="page">
      <div className="page-head">
        <h1>Журнал</h1>
        <span className="sub">{entries.length} событий за сеанс</span>
        <div className="grow" />
        <Seg
          options={[
            { value: "all", label: "Все" },
            { value: "success", label: "Успех" },
            { value: "info", label: "Инфо" },
            { value: "warn", label: "Внимание" },
          ]}
          value={filter}
          onChange={setFilter}
        />
        <label className="row" style={{ color: "var(--text-2)" }}>
          <input type="checkbox" checked={autoScroll} onChange={(e) => setAutoScroll(e.target.checked)} />
          автопрокрутка
        </label>
        <Button
          onClick={() => {
            setEntries([]);
          }}
        >
          Очистить
        </Button>
      </div>

      <Glass>
        <div
          ref={boxRef}
          className="log"
          style={{ maxHeight: "calc(100vh - 220px)", overflowY: "auto", paddingRight: 8 }}
        >
          {visible.length === 0 ? (
            <div className="sub" style={{ color: "var(--text-3)" }}>
              Журнал пуст. Запустите тест — события появятся здесь.
            </div>
          ) : (
            visible.map((l, i) => (
              <div key={i} className={`log-line ${l.level}`}>
                <span className="ts">{fmtTime(l.ts_ms)}</span>
                <span className="tx">{l.text}</span>
              </div>
            ))
          )}
        </div>
      </Glass>
    </div>
  );
}