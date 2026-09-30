// Страница «Результаты»: история сессий, сравнение по истории, HTML-отчёты.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  commands,
  onTestFinished,
  type HistoryRow,
  type SessionJson,
} from "../api";
import { Badge, Button, FadeScroll, Modal } from "../components/ui";
import {
  ChevronIcon,
  ExportIcon,
  FolderIcon,
  ReportIcon,
  RestoreIcon,
  SearchIcon,
  TrashIcon,
} from "../components/icons";
import { pushToast } from "../store";

type SortKey = "started" | "level" | "margin" | "stability";

const LEVEL_ORDER: Record<string, number> = {
  Confirmed: 0,
  Probable: 1,
  StabilityTieBreak: 2,
  Preliminary: 3,
  // Уровни ниже тоже нужно знать: без них `Screening`, `Equivalent` и `None`
  // получали тот же код, что и нечитаемые строки, и уезжали в самый низ
  // списка вместе с битыми файлами.
  Screening: 4,
  KeepCurrent: 5,
  Equivalent: 6,
  None: 7,
};

function levelKind(level: string): "ok" | "warn" | "plain" | "danger" {
  switch (level) {
    case "Confirmed":
      return "ok";
    case "Probable":
    case "StabilityTieBreak":
      return "danger";
    case "Preliminary":
    case "KeepCurrent":
      return "warn";
    default:
      return "plain";
  }
}

/** Ключ сортировки по убыванию: пропуски уходят в конец, NaN не появляется. */
function numDesc(v: number | null | undefined): number {
  return typeof v === "number" && Number.isFinite(v) ? v : -Infinity;
}

/** Ключ сортировки по возрастанию: пропуски тоже в конец. */
function numAsc(v: number | null | undefined): number {
  return typeof v === "number" && Number.isFinite(v) ? v : Infinity;
}

/** Русские склонения: 1 запись / 2 записи / 5 записей. */
export function plural(n: number, one: string, few: string, many: string): string {
  const a = Math.abs(Math.trunc(n)) % 100;
  const b = a % 10;
  if (a > 10 && a < 20) return many;
  if (b > 1 && b < 5) return few;
  if (b === 1) return one;
  return many;
}

export function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 Б";
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} ГБ`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} МБ`;
  if (n >= 1 << 10) return `${(n / (1 << 10)).toFixed(1)} КБ`;
  return `${n} Б`;
}

/** Число для показа: нефинитное/отсутствующее → прочерк. */
function f1(v: number | null | undefined, digits = 1): string {
  return typeof v === "number" && Number.isFinite(v) ? v.toFixed(digits) : "—";
}

/** `20260927T020233Z307` → `27.09.2026 · 02:02`.
 *
 *  Диапазоны проверяются: иначе битая метка вида `20261345T9999` превращалась
 *  бы в невозможное «45.13.2026 · 99:99». */
export function fmtStamp(stamp: string): string {
  const m = /^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})/.exec(stamp);
  if (!m) return stamp;
  const [, y, mo, d, h, mi] = m;
  const year = +y;
  const month = +mo;
  const day = +d;
  const hour = +h;
  const minute = +mi;
  const valid =
    year >= 2000 &&
    year <= 2999 &&
    month >= 1 &&
    month <= 12 &&
    day >= 1 &&
    day <= 31 &&
    hour <= 23 &&
    minute <= 59;
  if (!valid) return stamp;
  // Метка приходит в UTC (так её считает `date_time_stamp`), а журнал на
  // странице «Логи» печатает локальное время. Без перевода в локальную зону
  // одна и та же сессия подписывалась двумя разными временами в двух
  // разделах приложения.
  const local = new Date(
    Date.UTC(year, month - 1, day, hour, minute),
  );
  const pad = (v: number) => String(v).padStart(2, "0");
  return `${pad(local.getDate())}.${pad(local.getMonth() + 1)}.${local.getFullYear()} · ${pad(local.getHours())}:${pad(local.getMinutes())}`;
}

/** GUID в виде `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`. */
const GUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Unix ns → `ДД.ММ.ГГГГ · ЧЧ:ММ` в локальном времени. */
function fmtNs(ns: number): string {
  if (!Number.isFinite(ns) || ns <= 0) return "Дата неизвестна";
  const d = new Date(ns / 1e6);
  if (Number.isNaN(d.getTime())) return "Дата неизвестна";
  const pad = (v: number) => String(v).padStart(2, "0");
  return `${pad(d.getDate())}.${pad(d.getMonth() + 1)}.${d.getFullYear()} · ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Дата сессии: метка старта, иначе время изменения файла на диске.
 *
 *  У прерванной сессии метка старта не пишется, и `started_label` содержит
 *  нулевой таймстамп `19700101T000000Z000` — `fmtStamp` его отбрасывает по
 *  нижней границе года, и без запасного источника в карточку попадала либо
 *  сырая метка, либо прочерк. Теперь запасной источник — время изменения
 *  файла записи.
 */
function sessionDate(r: HistoryRow): string {
  if (r.started_at_ns > 0) {
    const s = fmtStamp(r.started_label);
    if (s !== r.started_label) return s;
  }
  if (r.file_modified_at_ns > 0) return fmtNs(r.file_modified_at_ns);
  return "Дата неизвестна";
}

export default function ResultsPage() {
  const [rows, setRows] = useState<HistoryRow[]>([]);
  const [sort, setSort] = useState<SortKey>("started");
  const [mode, setMode] = useState<"all" | "screening" | "full">("all");
  const [query, setQuery] = useState("");
  // Формат экспорта по умолчанию JSON, а CSV берётся из выпадающей части
  // кнопки: раньше формат выбирался в выпадающем списке, и он не помещался
  // в шапку по макету, но убирать выбор формата нельзя.
  const [fmtOpen, setFmtOpen] = useState(false);
  const fmtRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!fmtOpen) return;
    const away = (e: PointerEvent) => {
      if (!fmtRef.current?.contains(e.target as Node)) setFmtOpen(false);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") setFmtOpen(false);
    };
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [fmtOpen]);
  const [stats, setStats] = useState<{
    free_bytes: number;
    history_bytes: number;
    max_sessions: number;
  } | null>(null);
  const [detail, setDetail] = useState<SessionJson | null>(null);
  /** GUID → название из списка схем питания: нужно, когда в файле сессии
   *  сохранился только идентификатор лидера. */
  const [schemeNames, setSchemeNames] = useState<ReadonlyMap<string, string>>(
    () => new Map(),
  );
  const busy = useRef(false);
  const reportBusy = useRef(false);

  const refresh = useCallback(() => {
    commands.historyList().then(setRows).catch((e) => pushToast("err", String(e)));
    commands
      .storageStats()
      .then((s) =>
        setStats({
          free_bytes: s.free_bytes,
          history_bytes: s.history_bytes,
          max_sessions: s.max_sessions,
        }),
      )
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    refresh();
    const unfor = onTestFinished((m) => {
      refresh();
      if (m.ok && m.plan_guid) {
        commands.historyOpen(m.plan_guid).then(setDetail).catch(() => undefined);
      }
    });
    return () => {
      unfor.then((f) => f()).catch(() => undefined);
    };
  }, [refresh]);

  useEffect(() => {
    commands
      .listSchemes()
      .then((list) => {
        const m = new Map<string, string>();
        for (const s of list) {
          const name = (s.name || "").trim();
          if (name) m.set(s.guid.toLowerCase(), name);
        }
        setSchemeNames(m);
      })
      .catch(() => undefined);
  }, []);

  const sorted = useMemo(() => {
    const arr = [...rows];
    // Ключи сортировки не должны давать NaN: `-Infinity - -Infinity` = NaN, и
    // Array.sort с таким компаратором оставляет порядок произвольным.
    // Нечитаемые строки уходят в конец списка.
    const time = (r: HistoryRow) =>
      r.readable && Number.isFinite(r.started_at_ns) ? r.started_at_ns : -Infinity;
    switch (sort) {
      case "started":
        arr.sort((a, b) => {
          const d = time(b) - time(a);
          return Number.isNaN(d) ? 0 : d;
        });
        break;
      case "level":
        arr.sort(
          (a, b) =>
            (a.readable ? (LEVEL_ORDER[a.level] ?? 99) : 99) -
            (b.readable ? (LEVEL_ORDER[b.level] ?? 99) : 99),
        );
        break;
      // Балл лидера в истории бесполезен: он нормализован внутри своей
      // сессии, поэтому у лучшей схемы каждой сессии он всегда ровно 100 и
      // «100 против 100» ничего не значит. Сортируем по перевесу — чем он
      // меньше, тем надёжнее измерение, и это единственное, что сравнимо
      // между сессиями.
      case "margin":
        arr.sort((a, b) => {
          // Меньший перевес = надёжнее измерение, но только если он вообще
          // измерен. При одном прогоне на схему доверительный интервал не
          // строится, `margin` равен 0.0, и сортировка ставила такую сессию
          // первой — как будто у неё идеальная точность. Такие уходят в конец.
          const aHas = a.rounds_completed >= 2 && Number.isFinite(a.margin);
          const bHas = b.rounds_completed >= 2 && Number.isFinite(b.margin);
          if (aHas !== bHas) return aHas ? -1 : 1;
          return numAsc(a.margin) - numAsc(b.margin);
        });
        break;
      case "stability":
        arr.sort((a, b) => numDesc(b.stability) - numDesc(a.stability));
        break;
    }
    return arr;
  }, [rows, sort]);

  const openSession = (guid: string) => {
    commands.historyOpen(guid).then(setDetail).catch((e) => pushToast("err", String(e)));
  };

  // HTML-отчёт сессии: бэкенд сохраняет .html рядом с историей и открывает
  // его в браузере, сюда возвращается путь — показываем только имя файла.
  const openReport = (guid: string) => {
    if (reportBusy.current) return;
    reportBusy.current = true;
    commands
      .sessionReport(guid)
      .then((p) => {
        const name = p.split(/[/\\]/).pop() ?? p;
        pushToast("okk", `HTML-отчёт открыт в браузере: ${name}`);
      })
      .catch((e) => pushToast("err", String(e)))
      .finally(() => {
        reportBusy.current = false;
      });
  };

  const deleteRow = async (r: HistoryRow) => {
    // Подтверждение спрашиваем всегда: нечитаемый файл тоже удаляется, а
    // раньше он удалялся молча — самое обидное было потерять его не зная почему.
    const what = r.readable
      ? `Удалить запись «${r.plan_guid}»? Это действие необратимо.`
      : "Запись не читается. Удалить её файл безвозвратно?";
    if (!window.confirm(what)) return;
    try {
      await commands.historyDelete(r.file_name);
      pushToast("okk", "Запись удалена");
      if (detail?.plan_guid === r.plan_guid) setDetail(null);
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    }
  };

  const exportAll = async (format: "json" | "csv") => {
    if (busy.current) return;
    busy.current = true;
    try {
      const dir = await open({ directory: true, title: "Каталог для экспорта" });
      if (!dir) return;
      const paths = await commands.historyExportTo("all", format, dir as string);
      const note = paths.find((p) => p.startsWith("| "));
      pushToast("okk", `Экспортировано файлов: ${paths.length - (note ? 1 : 0)}`);
      if (note) pushToast("err", note.slice(2));
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      busy.current = false;
    }
  };

  const lowDisk = stats != null && stats.free_bytes < 250 * 1024 * 1024;

  // Режим замера различаем по числу завершённых раундов, а не по строке
  // уровня: при одном прогоне на схему доверительный интервал не строится, и
  // такой результат ничего не утверждает, даже если уровень называется иначе.
  const modeOf = (r: HistoryRow): "screening" | "full" =>
    r.rounds_completed >= 2 ? "full" : "screening";

  /** Есть ли у сессии итоговый результат. */
  const hasResult = (r: HistoryRow): boolean =>
    typeof r.throughput === "number" && Number.isFinite(r.throughput) && r.throughput > 0;

  /** Название схемы сессии.
   *
   *  В записи может сохраниться только GUID лидера, и тогда в карточке
   *  видно `1e600a58-9c04-…` вместо имени. По этому GUID ищем название в
   *  списке схем питания; если схемы уже нет в системе, показываем GUID
   *  укороченным — но уже с пометкой, что это идентификатор.
   */
  const schemeLabel = (r: HistoryRow): string => {
    const raw = (r.scheme_name || "").trim();
    if (raw && raw !== "-" && !GUID_RE.test(raw)) return raw;
    const guid = r.leader_scheme_guid || (GUID_RE.test(raw) ? raw : "");
    const known = guid ? schemeNames.get(guid.toLowerCase()) : undefined;
    if (known) return known;
    if (guid) return `Схема ${guid.slice(0, 8)}…`;
    return raw && raw !== "-" ? raw : "Сессия без схемы";
  };

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sorted.filter((r) => {
      if (mode !== "all" && !r.readable) return false;
      if (mode !== "all" && modeOf(r) !== mode) return false;
      if (q) {
        const hay = [
          schemeLabel(r),
          r.file_name,
          r.started_label,
          sessionDate(r),
          r.level_label,
        ]
          .join(" ")
          .toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
    // `schemeNames` в зависимостях: подстановка названия приходит позже
    // списка схем, и без этого отфильтрованный список не пересчитывался.
  }, [sorted, mode, query, schemeNames]);

  return (
    <div className="page fill results-page">
      <div className="page-head">
        <h1>Результаты</h1>
        {/* Счётчик живёт в строке заголовка: отдельной строкой он занимал
            высоту и отодвигал тулбар вниз на пустом месте. */}
        <span className="sub">
          <b>{rows.length}</b>
          {stats && stats.max_sessions > 0 ? ` из ${stats.max_sessions}` : ""} сессий
          {stats ? ` · ${fmtBytes(stats.history_bytes)}` : ""}
        </span>
        <div className="actions">
          {lowDisk ? <Badge kind="warn">Место на диске заканчивается</Badge> : null}
          <div className="split-act" ref={fmtRef}>
            <button
              type="button"
              className="act-secondary"
              title="Экспортировать результаты в JSON"
              onClick={() => void exportAll("json")}
            >
              <ExportIcon />
              Экспорт…
            </button>
            <button
              type="button"
              className="act-secondary caret"
              title="Выбрать формат экспорта"
              aria-label="Формат экспорта"
              aria-expanded={fmtOpen}
              onClick={() => setFmtOpen((v) => !v)}
            >
              <ChevronIcon />
            </button>
            {fmtOpen ? (
              <div className="mini-menu" role="menu">
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setFmtOpen(false);
                    void exportAll("json");
                  }}
                >
                  JSON
                </button>
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setFmtOpen(false);
                    void exportAll("csv");
                  }}
                >
                  CSV
                </button>
              </div>
            ) : null}
          </div>
          <button
            type="button"
            className="act-secondary"
            title="Открыть папку с отчётами в Проводнике"
            onClick={() => commands.historyOpenFolder().catch((e) => pushToast("err", String(e)))}
          >
            <FolderIcon />
            Папка
          </button>
          <button
            type="button"
            className="act-secondary"
            title="Пересканировать папку результатов"
            onClick={refresh}
          >
            <RestoreIcon />
            Обновить
          </button>
        </div>
      </div>

      <div className="results-toolbar">
        <div className="search-box res-search">
          <SearchIcon />
          <input
            className="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Поиск по схеме или дате…"
            aria-label="Поиск по результатам"
          />
        </div>
        <div className="toolbar-right">
          <div className="mode-tabs" role="tablist" aria-label="Режим замера">
            {(
              [
                { value: "all", label: "Все" },
                { value: "screening", label: "Скрининг" },
                { value: "full", label: "Полный замер" },
              ] as const
            ).map((t) => (
              <button
                key={t.value}
                type="button"
                role="tab"
                aria-selected={mode === t.value}
                className={`mtab${mode === t.value ? " active" : ""}`}
                onClick={() => setMode(t.value)}
              >
                {t.label}
              </button>
            ))}
          </div>
          <select
            className="sort-select"
            value={sort}
            aria-label="Сортировка"
            onChange={(e) => setSort(e.target.value as SortKey)}
          >
            <option value="started">По дате</option>
            <option value="level">По уровню</option>
            <option value="margin">По перевесу</option>
            <option value="stability">По стабильности</option>
          </select>
        </div>
      </div>

      {shown.length === 0 ? (
        <div className="empty-card">
          {rows.length === 0
            ? "Завершённые сессии с данными по схемам появятся здесь."
            : "Подходящих сессий не найдено"}
        </div>
      ) : (
        <FadeScroll className="session-list fill-list">
          {shown.map((r) => (
            <article
              key={r.file_name}
              className={`session-card${r.readable ? "" : " is-unreadable"}`}
              // Карточка открывает сессию кликом; раньше это был `<div>` без
              // роли и клавиатуры, то есть недоступный с клавиатуры элемент.
              role={r.readable ? "button" : undefined}
              tabIndex={r.readable ? 0 : undefined}
              onClick={() => r.readable && openSession(r.plan_guid)}
              onKeyDown={(e) => {
                if (!r.readable) return;
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  openSession(r.plan_guid);
                }
              }}
            >
              <div className="sc-main">
                <div className="sc-top">
                  {hasResult(r) ? (
                    <span className="sc-winner-tag">Лидер</span>
                  ) : (
                    <span className="sc-stop-tag" title="Сессия прервана до получения результата">
                      Прервана
                    </span>
                  )}
                  <span className="sc-title">
                    {r.readable ? schemeLabel(r) : r.file_name}
                  </span>
                </div>
                <div className="sc-sub">
                  <span>{sessionDate(r)}</span>
                  <span className="dot-sep">·</span>
                  <span className="mode-pill">
                    {!r.readable
                      ? "Не завершено"
                      : modeOf(r) === "full"
                        ? "Полный"
                        : "Скрининг"}
                  </span>
                  {r.early_stopped ? <span className="mode-pill warn">ранняя остановка</span> : null}
                  {!r.readable && r.error ? (
                    <span className="mode-pill err" title={r.error}>
                      {r.error}
                    </span>
                  ) : null}
                </div>
              </div>

              <div className="sc-metrics">
                <div className="m-col" title="Медианный throughput лидера">
                  <span className="m-label">Результат</span>
                  <span className={`m-val${hasResult(r) ? " score" : ""}`}>
                    {hasResult(r) ? f1(r.throughput, 0) : "—"}
                    {hasResult(r) ? <small>тик/с</small> : null}
                  </span>
                </div>
                <div className="m-col">
                  <span className="m-label">Схем</span>
                  <span className="m-val">{r.schemes}</span>
                </div>
                <div className="m-col" title="Доверительный интервал (разброс оценки)">
                  <span className="m-label">Погрешность</span>
                  <span className="m-val">
                    {r.margin != null && Number.isFinite(r.margin)
                      ? `±${r.margin.toFixed(1)}`
                      : "—"}
                  </span>
                </div>
              </div>

              <div className="sc-actions">
                {r.readable ? (
                  <button
                    type="button"
                    className="btn-report"
                    title="Открыть HTML-отчёт в браузере"
                    onClick={(e) => {
                      e.stopPropagation();
                      openReport(r.plan_guid);
                    }}
                  >
                    <ReportIcon />
                    Отчёт HTML
                  </button>
                ) : null}
                <button
                  type="button"
                  className="btn-del"
                  title="Удалить запись"
                  aria-label={`Удалить запись ${r.readable ? schemeLabel(r) : r.file_name}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    void deleteRow(r);
                  }}
                >
                  <TrashIcon />
                </button>
              </div>
            </article>
          ))}
        </FadeScroll>
      )}

      <Modal open={detail !== null} title="Результат сессии" wide onClose={() => setDetail(null)}>
        {detail ? (
          <SessionDetail
            s={detail}
            fileName={rows.find((r) => r.plan_guid === detail.plan_guid)?.file_name ?? ""}
            onDelete={() => {
              setDetail(null);
              refresh();
            }}
          />
        ) : null}
      </Modal>
    </div>
  );
}

function SessionDetail({
  s,
  fileName,
  onDelete,
}: {
  s: SessionJson;
  fileName: string;
  onDelete: () => void;
}) {
  const rec = s.recommendation;
  const probs = rec.probabilities;
  const [opening, setOpening] = useState(false);

  const del = async () => {
    if (!fileName) {
      pushToast("err", "Файл записи не найден");
      return;
    }
    if (!window.confirm("Удалить запись?")) return;
    try {
      await commands.historyDelete(fileName);
      pushToast("okk", "Запись удалена");
      onDelete();
    } catch (e) {
      pushToast("err", String(e));
    }
  };

  const openHtml = () => {
    if (opening) return;
    setOpening(true);
    commands
      .sessionReport(s.plan_guid)
      .then((p) => {
        const name = p.split(/[/\\]/).pop() ?? p;
        pushToast("okk", `HTML-отчёт открыт в браузере: ${name}`);
      })
      .catch((e) => pushToast("err", String(e)))
      .finally(() => setOpening(false));
  };

  // Лидер: рекомендованная схема, иначе лучшая по медиане среди допущенных.
  const tie = rec.level === "Equivalent" || rec.level === "KeepCurrent";
  const admitted = s.schemes.filter((x) => !x.rejected);
  const leader =
    (!tie && rec.recommended_scheme
      ? s.schemes.find((x) => x.scheme_id === rec.recommended_scheme)
      : undefined) ??
    [...admitted].sort((a, b) => b.median_throughput - a.median_throughput)[0] ??
    null;
  const leaderName = leader ? (leader.name ?? leader.scheme_id) : null;
  const margin = rec.expected_margin_percent;
  const marginText =
    margin != null && Number.isFinite(margin) ? `${margin >= 0 ? "+" : ""}${margin.toFixed(2)}%` : "—";
  const marginKind = margin == null || !Number.isFinite(margin) ? "" : margin >= 1 ? "ok" : "warn";

  // Таблица: лидер первым, затем остальные по убыванию медианы; забракованные — в конце.
  const tableRows = [...s.schemes].sort((a, b) => {
    if (a.rejected !== b.rejected) return a.rejected ? 1 : -1;
    return b.median_throughput - a.median_throughput;
  });
  const rejectedCount = s.schemes.length - admitted.length;
  const totalRuns = s.schemes.reduce((n, x) => n + x.runs, 0);

  // Доверие к лидерству берём из `recommend()` — того же расчёта, который
  // поставил бейдж «Предварительно» рядом. Раньше здесь стояла своя проверка
  // «прогонов >= 2», и при двух прогонах на схему бейдж говорил
  // «Предварительно», а плашка под ним — «Лидеру можно верить»: два
  // взаимоисключающих вывода на одном экране.
  const trustNote = !leader
    ? { kind: "warn" as const, title: "Сравнивать нечего", items: ["Все схемы забракованы."] }
    : {
        kind: rec.level === "Confirmed" || rec.level === "Probable" ? ("ok" as const) : ("warn" as const),
        title:
          rec.level === "Confirmed"
            ? "Лидерство подтверждено"
            : rec.level === "Probable"
              ? "Лидеру можно верить"
              : rec.level === "StabilityTieBreak"
                ? "Лидер выбран по стабильности, не по скорости"
                : rec.level === "Preliminary"
                  ? "Вердикт предварительный"
                  : (rec.level_label ?? "Вердикт не определён"),
        items: [
          rec.level === "Preliminary" || rec.level === "StabilityTieBreak"
            ? `Повторите сессию в режиме «Детально» — у лидера ${leader.runs} ${plural(
                leader.runs,
                "прогон",
                "прогона",
                "прогонов",
              )}.`
            : `У лидера ${leader.runs} ${plural(leader.runs, "прогон", "прогона", "прогонов")}.`,
        ],
      };

  return (
    <div className="rd">
      <div className="rd-hero">
        <div className="rd-kicker">{tie ? "ничья" : "лидер сессии"}</div>
        <div className="rd-lead ok">{leaderName ? `«${leaderName}»` : (rec.level_label ?? "—")}</div>
        <div className="row wrap gap-2" style={{ justifyContent: "center" }}>
          <Badge kind={levelKind(rec.level)} big>
            {rec.level_label ?? rec.level}
          </Badge>
          <Badge kind="plain">раундов {s.rounds_completed}/{s.rounds_planned}</Badge>
        </div>
      </div>

      <div className="rd-stats">
        <div className="rd-stat">
          <div className="k">Медиана лидера</div>
          <div className="v ok">{f1(leader?.median_throughput)}</div>
          <div className="s">тик/с</div>
        </div>
        <div className="rd-stat">
          {/* Хэш конфигурации определяет сопоставимость сессий: без него
              нельзя понять, что два замера мерили одно и то же. */}
          <div className="k">Конфигурация</div>
          <div className="v mono break">{s.identity.config_hash || "—"}</div>
          <div className="s">
            seed {s.identity.seed_hex || "-"} · воркеров {s.identity.worker_count} /{" "}
            {s.identity.logical_cpus}
          </div>
        </div>
        <div className="rd-stat">
          <div className="k">Перевес</div>
          <div className={`v ${marginKind}`}>{marginText}</div>
          <div className="s">ожидаемый</div>
        </div>
        <div className="rd-stat">
          <div className="k">Схем</div>
          <div className="v">{admitted.length}</div>
          <div className="s">
            {rejectedCount > 0
              ? `забраковано: ${rejectedCount} ${plural(rejectedCount, "схема", "схемы", "схем")}`
              : "все допущены"}
          </div>
        </div>
        <div className="rd-stat">
          <div className="k">Прогонов</div>
          <div className="v">{totalRuns}</div>
          <div className="s">всего</div>
        </div>
      </div>

      {/* Состав машины вынесен под карточки: в узкой плитке строка с CPU, ОС и
          памятью обрезалась многоточием и половина условий была не видна. */}
      <div className="rd-machine">
        <span>
          {s.identity.cpu_brand || s.identity.cpu_identifier}
        </span>
        <span>ОС {s.identity.os_build || "—"}</span>
        <span>
          {s.identity.memory_gib > 0
            ? `${s.identity.memory_gib.toFixed(0)} ГБ ОЗУ`
            : "память неизвестна"}
        </span>
      </div>

      <div className={`rd-note ${trustNote.kind}`}>
        <b>{trustNote.title}</b>
        <ul>
          {trustNote.items.map((t, i) => (
            <li key={i}>{t}</li>
          ))}
        </ul>
        {rec.reason ? <span className="why">{rec.reason}</span> : null}
      </div>
      {/* Условия замера: без них «уверенный» вердикт выглядит так же, как
          вердикт на зашумлённой и перегретой машине. Про сам скрининг уже
          сказано в плашке выше, поэтому здесь только измерения. */}
      {s.reference ? (
        <div className={`rd-note ${s.reference.unstable ? "warn" : ""}`}>
          <b>Опорная схема:</b> {s.reference.scheme_name ?? s.reference.scheme_id} —{" "}
          {s.reference.per_round.map((v) => f1(v, 0)).join(" → ")} тик/с по раундам.{" "}
          разброс {s.reference.span_percent.toFixed(1)} % (порог{" "}
          {s.reference.span_limit_percent.toFixed(1)} %), тренд{" "}
          {s.reference.trend_percent_per_round >= 0 ? "+" : ""}
          {s.reference.trend_percent_per_round.toFixed(1)} %/раунд.
          {s.reference.unstable
            ? " Машина плавает сильнее, чем различаются схемы: вердикт понижен."
            : " Разброс в пределах нормы."}
        </div>
      ) : null}
      {s.warnings.length > 0 ? (
        <div className="rd-note warn">{s.warnings.join(" ")}</div>
      ) : null}

      {/* Метрики по фазам: внутри фазы схемы сравнимы, между фазами «тик/с» —
          нет, потому что работа на тик различается. */}
      {leader && leader.phases.length > 0 ? (
        <div className="modal-scrollx">
          <table className="grid evid-table rd-table">
            <thead>
              <tr>
                <th>Фаза</th>
                <th className="num">Медиана, тик/с</th>
                <th className="num">P1, тик/с</th>
                <th className="num">Стабильность</th>
                <th>Частота</th>
              </tr>
            </thead>
            <tbody>
              {leader.phases.map((p) => (
                <tr
                  key={p.name}
                  className={p.frequency_drop_percent >= 5 ? "warn-row" : undefined}
                >
                  <td className="nm">{p.name}</td>
                  <td className="num">{f1(p.median_throughput)}</td>
                  <td className="num">{f1(p.p1_throughput)}</td>
                  <td className="num">{f1(p.consistency_percent, 2)}</td>
                  <td>
                    {p.frequency_mhz > 0
                      ? `${Math.round(p.frequency_mhz)}${
                          p.frequency_drop_percent >= 5
                            ? ` ↓${Math.round(p.frequency_drop_percent)}%`
                            : ""
                        }`
                      : "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="field-hint">
            Лидер по фазам. Внутри фазы схемы сравнимы столбик к столбику; между
            разными фазами «тик/с» сравнивать нельзя — у лёгкой фазы работы на
            тик меньше. Стрелка ↓ означает, что в этой фазе замечено снижение
            частоты относительно лучшей частоты сессии: часть фазы измерялась
            на пониженной частоте, и это не заслуга схемы питания.
          </div>
        </div>
      ) : null}
      {/* Что было до и после сессии: пользователю важно знать, вернулась ли
          система к прежней схеме питания. */}
      {s.original_scheme_guid || s.original_restored ? (
        <div className="rd-note">
          <b>Системная схема:</b>{" "}
          {s.original_scheme_guid ? (
            <>
              <span className="mono break">{s.original_scheme_guid}</span>{" "}
              {s.original_restored
                ? "— восстановлена после сессии"
                : "— НЕ восстановлена, верните её вручную"}
            </>
          ) : (
            "не менялась"
          )}
        </div>
      ) : null}

      {s.early_stop_reason ? (
        <div className="rd-note warn">
          <b>Ранняя остановка:</b> {s.early_stop_reason}
        </div>
      ) : null}

      {probs ? (
        <div className="rd-note">
          <b>Уверенность:</b> P(лучший) = {f1(probs[0], 2)} · P(перевес &gt; 0) ={" "}
          {f1(probs[1], 2)} · P(перевес &gt; 1%) = {f1(probs[2], 2)}
        </div>
      ) : null}

      <div className="modal-scrollx">
        <table className="grid evid-table rd-table">
          <thead>
            <tr>
              <th>Схема</th>
              <th className="num">Медиана, тик/с</th>
              <th className="num">ДИ 95%</th>
              <th className="num">Прогонов</th>
              <th>Статус</th>
            </tr>
          </thead>
          <tbody>
            {tableRows.map((sch) => {
              const isLeader = leader != null && sch.scheme_id === leader.scheme_id;
              return (
                <tr key={sch.scheme_id} className={isLeader ? "lead" : sch.rejected ? "dead" : undefined}>
                  <td className="nm">
                    {isLeader ? "★ " : ""}
                    {sch.name ?? sch.scheme_id}
                  </td>
                  <td className="num">{f1(sch.median_throughput)}</td>
                  <td className="num">
                    {sch.ci_95[0] > 0 ? `[${f1(sch.ci_95[0])}; ${f1(sch.ci_95[1])}]` : "—"}
                  </td>
                  <td className="num">{sch.runs}</td>
                  <td>
                    {sch.rejected ? (
                      // Причина брака — главное, ради чего строка и есть:
                      // «забракована» без объяснения ни о чём не говорит.
                      <Badge kind="danger" title={sch.rejection_reason ?? undefined}>
                        забракована
                        {sch.rejection_reason ? `: ${sch.rejection_reason}` : ""}
                      </Badge>
                    ) : isLeader ? (
                      <Badge kind="ok">лидер</Badge>
                    ) : (
                      <Badge kind="plain">допущена</Badge>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <div className="rd-foot">
        <Button variant="primary" disabled={opening} onClick={openHtml}>
          {opening ? "Открываю…" : "Открыть HTML-отчёт"}
        </Button>
        <span className="spacer" />
        <Button variant="danger" onClick={() => void del()}>
          Удалить запись
        </Button>
      </div>
    </div>
  );
}
