// Страница «Логи»: фильтруемый журнал запуска, ошибок и событий сессий.
//
// Раскладка сверху вниз: заголовок с кнопкой выгрузки, один ряд фильтров
// (поиск и уровни со счётчиками в одном месте), затем журнал на всю оставшуюся
// высоту. Раньше фильтры и счётчики стояли в двух рядах плюс отдельная строка
// с длинным пояснением — четыре полосы интерфейса на одну функцию.

import { useEffect, useMemo, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { commands, onLog, type LoggerEntry } from "../api";
import { SearchIcon } from "../components/icons";
import { pushToast } from "../store";

/** Четыре уровня журнала плюс «все». */
type Level = "all" | "info" | "success" | "warn" | "error";

/** Сколько строк держим в «живом» буфере: больше уже не прочитать глазом. */
const LIVE_CAP = 200;
/** Сколько записей реально рисуем: журнал хранит тысячи, DOM — нет. */
const RENDER_CAP = 1000;

const LEVEL_LABEL: Record<Level, string> = {
  all: "Все",
  info: "Инфо",
  success: "Успех",
  warn: "Внимание",
  error: "Ошибки",
};

/** Уровни, для которых вкладка показывает свой счётчик. */
const LEVEL_TABS: Exclude<Level, "all">[] = ["info", "success", "warn", "error"];

/** Плашка уровня в строке журнала: заглавными, как в макете. */
const LEVEL_BADGE: Record<string, string> = {
  info: "Инфо",
  success: "Успех",
  warn: "Внимание",
  error: "Ошибка",
};

/** Привести уровень из журнала к одному из четырёх отображаемых. */
function normLevel(raw: string): string {
  if (raw === "warning" || raw === "warn") return "warn";
  if (raw === "err" || raw === "error") return "error";
  if (raw === "ok" || raw === "success") return "success";
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

export default function LogPage() {
  const [entries, setEntries] = useState<LogRow[] | null>(null);
  const [live, setLive] = useState<LogRow[]>([]);
  const [query, setQuery] = useState("");
  const [level, setLevel] = useState<Level>("all");
  const [saving, setSaving] = useState(false);
  // По умолчанию скрываем: отчёт заведомо уходит вовне (в чат, в issues), а
  // логин и путь `C:\Users\…` для разбора ошибки ничего не дают.
  const [redact, setRedact] = useState(true);
  const [copied, setCopied] = useState(false);
  const nextKey = useRef(0);
  const listRef = useRef<HTMLDivElement | null>(null);
  const copyTimer = useRef<number | null>(null);
  // Держим ли взгляд на свежих записях. Пока пользователь не отлистал вверх —
  // да, и новые строки подтягивают список вниз; отлистал — остаём на месте,
  // иначе он читал бы старое событие, пока список уезжает под пальцем.
  const stickToBottom = useRef(true);
  const firstPaint = useRef(true);

  useEffect(() => {
    let alive = true;
    commands
      .logHistory()
      .then((rows) => {
        if (!alive) return;
        nextKey.current = rows.length;
        setEntries(rows.map((e, i) => ({ ...e, key: `h${i}` })));
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
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
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

  // Журнал открывают ради последнего события, а не ради первого за сессию, и
  // без прокрутки вниз на экране просто самые старые записи. Первая отрисовка
  // и появление новых строк ведут вниз; если человек отлистал вверх —
  // оставляем его там, иначе он читал бы одно, пока список уезжает.
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    if (firstPaint.current || stickToBottom.current) {
      el.scrollTop = el.scrollHeight;
      stickToBottom.current = true;
    }
    firstPaint.current = false;
  }, [visible.length]);

  const onListScroll = () => {
    const el = listRef.current;
    if (!el) return;
    // Порог в 48px: «у нижнего края» с небольшим запасом, иначе колесо мыши
    // на одно колесо выключало бы слежение.
    stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  };

  const scrollToBottom = () => {
    const el = listRef.current;
    if (!el) return;
    stickToBottom.current = true;
    el.scrollTop = el.scrollHeight;
  };

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

  // Копирование видимых строк: в буфер обмена попадает ровно то, что человек
  // видит на экране, — с теми же фильтром и поиском.
  const copyVisible = async () => {
    const text = visible
      .map((e) => {
        const k = normLevel(e.level);
        return `${timeOf(e.ts_ms)} [${LEVEL_BADGE[k] ?? e.level}] ${e.text}`;
      })
      .join("\n");
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1200);
    } catch (e) {
      pushToast("err", String(e));
    }
  };

  const meta =
    hidden > 0
      ? `Показаны последние ${visible.length} из ${shown.length} записей · ` +
        `для более старых уточните фильтр или поиск`
      : `Показано ${shown.length} из ${all.length} записей`;

  return (
    <div className="page fill logs-page">
      <div className="page-head">
        <h1>Логи</h1>
        <div className="actions">
          <div
            className="support-bar"
            title="В отчёт войдут журнал, конфигурация, состояние контрольной точки, карантин и последняя сессия"
          >
            <label className="privacy-toggle">
              <input
                type="checkbox"
                checked={redact}
                onChange={(e) => setRedact(e.target.checked)}
              />
              <span>Скрыть имя ПК и пути профиля</span>
            </label>
            <button
              type="button"
              className="export-btn"
              onClick={() => void saveReport()}
              disabled={saving}
              title="Сохранить один файл: журнал, окружение, состояние данных и последнюю сессию"
            >
              {saving ? "Готовлю отчёт…" : "Сохранить отчёт для поддержки"}
            </button>
          </div>
        </div>
      </div>

      <div className="log-controls">
        <div className="search-box log-search">
          <SearchIcon />
          <input
            className="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Поиск по тексту лога…"
            aria-label="Поиск по тексту записи журнала"
          />
        </div>
        <div className="filter-tabs" role="tablist" aria-label="Фильтр по уровню">
          <button
            type="button"
            role="tab"
            aria-selected={level === "all"}
            className={`ftab${level === "all" ? " active" : ""}`}
            data-level="all"
            onClick={() => setLevel("all")}
          >
            <span>Все</span>
            <span className="cnt">{counts.all}</span>
          </button>
          {LEVEL_TABS.map((lv) => (
            <button
              key={lv}
              type="button"
              role="tab"
              aria-selected={level === lv}
              className={`ftab${level === lv ? " active" : ""}`}
              data-level={lv}
              onClick={() => setLevel(lv)}
            >
              <span>{LEVEL_LABEL[lv]}</span>
              <span className="cnt">{counts[lv]}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="log-card">
        <div className="log-card-head">
          <span className="log-meta">{meta}</span>
          <div className="log-tools">
            <button
              type="button"
              className="tool-btn"
              onClick={() => void copyVisible()}
              disabled={visible.length === 0}
            >
              {copied ? "Скопировано" : "Копировать видимые"}
            </button>
            <button
              type="button"
              className="tool-btn"
              onClick={scrollToBottom}
            >
              В конец ↓
            </button>
          </div>
        </div>

        {entries === null ? (
          <div className="log-list">
            <div className="empty-state">Загрузка журнала…</div>
          </div>
        ) : visible.length === 0 ? (
          <div className="empty-state">
            {all.length === 0
              ? "Журнал пуст. Здесь появятся события тестов и ошибки приложения."
              : "Записей по выбранному фильтру не найдено"}
          </div>
        ) : (
          <div className="log-list" ref={listRef} onScroll={onListScroll}>
            {visible.map((e) => {
              const k = normLevel(e.level);
              return (
                <div key={e.key} className="log-row" data-level={k}>
                  <span className="l-time">{timeOf(e.ts_ms)}</span>
                  <span className={`l-badge ${k}`}>{LEVEL_BADGE[k] ?? e.level}</span>
                  <span className="l-msg">{e.text}</span>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
