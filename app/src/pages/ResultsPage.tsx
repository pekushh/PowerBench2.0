// Страница «Результаты»: история сессий, сравнение по истории, HTML-отчёты.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  commands,
  onTestFinished,
  type HistoryRow,
  type SessionJson,
} from "../api";
import { Badge, FadeScroll, Modal } from "../components/ui";
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
  const [reportOpening, setReportOpening] = useState(false);
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

  const openReportFromModal = useCallback((guid: string) => {
    if (reportBusy.current) return;
    reportBusy.current = true;
    setReportOpening(true);
    commands
      .sessionReport(guid)
      .then((p) => {
        const name = p.split(/[/\\]/).pop() ?? p;
        pushToast("okk", `HTML-отчёт открыт в браузере: ${name}`);
      })
      .catch((e) => pushToast("err", String(e)))
      .finally(() => {
        reportBusy.current = false;
        setReportOpening(false);
      });
  }, []);

  const detailRow = useMemo(
    () => rows.find((r) => r.plan_guid === detail?.plan_guid) ?? null,
    [rows, detail],
  );

  // Удаление записи из подвала модалки: файл закрытой записи известен только
  // по строке списка, поэтому удаляем через неё.
  const deleteDetail = async () => {
    const file = detailRow?.file_name;
    if (!detailRow || !file) {
      pushToast("err", "Файл записи не найден");
      return;
    }
    if (!window.confirm("Удалить запись?")) return;
    try {
      await commands.historyDelete(file);
      pushToast("okk", "Запись удалена");
      setDetail(null);
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    }
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

      <Modal
        open={detail !== null}
        wide
        className="session-modal"
        title={
          detail ? (
            <span className="sm-title">
              Результат сессии
              <span className="sm-hw-meta">{hwMeta(detail, detailRow)}</span>
            </span>
          ) : (
            "Результат сессии"
          )
        }
        onClose={() => setDetail(null)}
        footer={
          detail ? (
            <SessionFooter
              s={detail}
              schemeNames={schemeNames}
              opening={reportOpening}
              onReport={() => openReportFromModal(detail.plan_guid)}
              onDelete={() => void deleteDetail()}
            />
          ) : null
        }
      >
        {detail ? <SessionDetail s={detail} schemeNames={schemeNames} /> : null}
      </Modal>
    </div>
  );
}

/** Копирование значения в буфер: чипы в подвале показывают его обрезанным. */
async function copyText(value: string, what: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(value);
    pushToast("okk", `${what} скопирован`);
  } catch (e) {
    pushToast("err", String(e));
  }
}

/** Дата и состав машины одной строкой в шапке модалки. */
function hwMeta(s: SessionJson, row: HistoryRow | null): string {
  const id = s.identity;
  const cpu = (id.cpu_brand || id.cpu_identifier || "процессор неизвестен").trim();
  // Число ядер — часть названия процессора, а не отдельный пункт: через `·`
  // строка выходила «Процессор · (6 ядер)».
  const cores = id.logical_cpus > 0 ? ` (${id.logical_cpus} ${plural(id.logical_cpus, "ядро", "ядра", "ядер")})` : "";
  const parts = [`${cpu}${cores}`];
  if (id.memory_gib > 0) parts.push(`${id.memory_gib.toFixed(0)} ГБ ОЗУ`);
  if (id.os_build) parts.push(`ОС ${id.os_build}`);
  const date = row ? sessionDate(row) : "";
  return date ? `${date} · ${parts.join(" · ")}` : parts.join(" · ");
}

/** Подвал модалки: тех-детали чипами слева, действия справа.
 *
 *  Кнопки и метаданные уехали сюда из тела: при 109 схемах модалка
 *  прокручивалась вместе с ними, и действия оказывались в самом низу
 *  страницы. */
function SessionFooter({
  s,
  schemeNames,
  opening,
  onReport,
  onDelete,
}: {
  s: SessionJson;
  schemeNames: ReadonlyMap<string, string>;
  opening: boolean;
  onReport: () => void;
  onDelete: () => void;
}) {
  const originalName = s.original_scheme_guid
    ? (schemeNames.get(s.original_scheme_guid.toLowerCase()) ?? s.original_scheme_guid)
    : null;
  return (
    <div className="sm-foot-inner">
      <div className="sm-foot-meta">
        {originalName ? (
          <span>
            {s.original_restored ? "✓" : "⚠"} Исходная схема «{originalName}»{" "}
            {s.original_restored ? "восстановлена" : "НЕ восстановлена"}
          </span>
        ) : (
          <span>Системная схема не менялась</span>
        )}
        {s.identity.config_hash ? (
          <button
            type="button"
            className="meta-chip"
            title={`Конфигурация: ${s.identity.config_hash} · нажмите, чтобы скопировать`}
            onClick={() => void copyText(s.identity.config_hash, "Хэш конфигурации")}
          >
            Конфиг: {s.identity.config_hash.slice(0, 8)}…
          </button>
        ) : null}
        {s.identity.seed_hex ? (
          <button
            type="button"
            className="meta-chip"
            title={`Seed: ${s.identity.seed_hex} · нажмите, чтобы скопировать`}
            onClick={() => void copyText(s.identity.seed_hex, "Seed")}
          >
            Seed: {s.identity.seed_hex.slice(0, 8)}…
          </button>
        ) : null}
      </div>
      <div className="sm-foot-actions">
        <button type="button" className="btn-delete" onClick={onDelete}>
          Удалить запись
        </button>
        <button type="button" className="btn-open-report" disabled={opening} onClick={onReport}>
          {opening ? "Открываю…" : "Открыть HTML-отчёт"}
        </button>
      </div>
    </div>
  );
}

function SessionDetail({
  s,
  schemeNames,
}: {
  s: SessionJson;
  schemeNames: ReadonlyMap<string, string>;
}) {
  const rec = s.recommendation;
  const probs = rec.probabilities;

  // Лидер: рекомендованная схема, иначе лучшая по медиане среди допущенных.
  const tie = rec.level === "Equivalent" || rec.level === "KeepCurrent";
  const admitted = s.schemes.filter((x) => !x.rejected);
  const leader =
    (!tie && rec.recommended_scheme
      ? s.schemes.find((x) => x.scheme_id === rec.recommended_scheme)
      : undefined) ??
    [...admitted].sort((a, b) => b.median_throughput - a.median_throughput)[0] ??
    null;
  // Имя лидера: в записи может сохраниться только GUID — тогда берём
  // название из списка схем питания.
  const leaderName = leader
    ? ((leader.name ?? "").trim() ||
      schemeNames.get(leader.scheme_id.toLowerCase()) ||
      `Схема ${leader.scheme_id.slice(0, 8)}…`)
    : (rec.level_label ?? "—");
  const margin = rec.expected_margin_percent;
  const marginText =
    margin != null && Number.isFinite(margin)
      ? `${margin >= 0 ? "+" : ""}${margin.toFixed(2)}%`
      : "—";
  const marginGreen = margin != null && Number.isFinite(margin) && margin >= 1;

  // Таблица: лидер первым, затем остальные по убыванию медианы; забракованные — в конце.
  const tableRows = [...s.schemes].sort((a, b) => {
    if (a.rejected !== b.rejected) return a.rejected ? 1 : -1;
    return b.median_throughput - a.median_throughput;
  });
  const rejectedCount = s.schemes.length - admitted.length;
  const totalRuns = s.schemes.reduce((n, x) => n + x.runs, 0);
  const testedCount = s.schemes.filter((x) => x.runs > 0).length;
  const leaderMedian = leader?.median_throughput ?? 0;
  const modeName = s.screening ? "Скрининг" : "Детально";

  // Фильтр таблицы: пустые схемы (0 прогонов) по умолчанию скрыты, иначе
  // сессия на сотню схем выглядит как таблица из нулей.
  const [tblFilter, setTblFilter] = useState<"tested" | "all">("tested");
  const [schemeQuery, setSchemeQuery] = useState("");
  const [tblSort, setTblSort] = useState<"speed" | "alpha">("speed");
  const hasEmpty = testedCount < s.schemes.length;
  const shownSchemes = useMemo(() => {
    const q = schemeQuery.trim().toLowerCase();
    let list = tblFilter === "tested" ? tableRows.filter((x) => x.runs > 0) : tableRows;
    if (q) {
      list = list.filter(
        (x) =>
          (x.name ?? "").toLowerCase().includes(q) ||
          x.scheme_id.toLowerCase().includes(q),
      );
    }
    if (tblSort === "alpha") {
      list = [...list].sort((a, b) =>
        (a.name ?? a.scheme_id).localeCompare(b.name ?? b.scheme_id, "ru", {
          sensitivity: "base",
        }),
      );
    }
    return list;
  }, [tableRows, tblFilter, schemeQuery, tblSort]);

  // Пиковый фон CPU: считаем по всем прогонам сессии, как это делает
  // бэкенд при выдаче предупреждений.
  const bgP95 = useMemo(() => {
    let max = 0;
    let has = false;
    for (const sch of s.schemes) {
      for (const r of sch.per_run) {
        const v = r.background_cpu_p95;
        if (typeof v === "number" && Number.isFinite(v) && (r.background_sample_seconds ?? 0) > 0) {
          has = true;
          if (v > max) max = v;
        }
      }
    }
    return has ? max : null;
  }, [s.schemes]);

  const alerts: { title: string; text: string }[] = [];
  const leaderRuns = leader?.runs ?? 0;
  if (leaderRuns > 0 && leaderRuns < 3) {
    alerts.push({
      title: `${modeName} (${leaderRuns} ${plural(leaderRuns, "прогон", "прогона", "прогонов")}):`,
      text: "для расчёта доверительного интервала нужно от 3 прогонов.",
    });
  }
  // Отформатированный чип про фон CPU идёт первым, а следом бэкенд присылает
  // ту же фактическую строку («фон на загруженной машине: до 55 % CPU…»).
  // Показывали оба — полоса из трёх плашек говорила одно и то же дважды.
  // Порог тот же, что и у бэкенда (`BACKGROUND_P95_NOTE_PERCENT`): иначе при
  // 18 % показывался бы наш чип без его предупреждения, а при 22 % — наоборот.
  const showBgChip = bgP95 != null && bgP95 > 20;
  if (showBgChip) {
    alerts.push({
      title: `Фон до ${bgP95.toFixed(0)} % CPU:`,
      text: "95-й перцентиль фоновой нагрузки выше порога.",
    });
  }
  for (const w of s.warnings) {
    if (showBgChip && /фон/i.test(w) && /%/i.test(w)) continue;
    const cut = w.indexOf(": ");
    alerts.push(
      cut > 0 && cut < 46
        ? { title: w.slice(0, cut + 1), text: w.slice(cut + 2) }
        : { title: "Замечание:", text: w },
    );
  }
  if (s.early_stop_reason) {
    alerts.push({ title: "Ранняя остановка:", text: s.early_stop_reason });
  }

  const trustTitle = !leader
    ? "Сравнивать нечего: все схемы забракованы."
    : rec.level === "Confirmed"
      ? "Лидерство подтверждено."
      : rec.level === "Probable"
        ? "Лидеру можно верить."
        : rec.level === "StabilityTieBreak"
          ? "Лидер выбран по стабильности, а не по скорости."
          : rec.level === "Preliminary"
            ? "Вердикт предварительный."
            : (rec.level_label ?? "Вердикт не определён.");

  return (
    <div className="sm-body">
      <section className="leader-hero">
        <div className="lh-left">
          <div className="lh-eyebrow">
            <span className="badge-leader" title={trustTitle}>
              {tie ? "★ Ничья" : leader ? "★ Лидер сессии" : "★ Лидер не определён"}
            </span>
            <span className="badge-pill">
              {modeName} · {s.rounds_completed}/{s.rounds_planned} раунд
            </span>
            {probs ? (
              // Вердикт убрали из отдельного бейджа: у скрининга он совпадал с
              // режимом, и рядом стояли два одинаковых «Скрининг». Остался
              // здесь, в подсказке, вместе с вероятностями.
              <span
                className="badge-pill"
                title={`Вердикт: ${rec.level_label ?? rec.level}. ${
                  probs
                    ? `P(лучший)=${f1(probs[0], 2)} · P(перевес>1%)=${f1(probs[2], 2)}`
                    : ""
                }`}
              >
                Уверенность {Math.round(Math.min(1, Math.max(0, probs[0])) * 100)}%
              </span>
            ) : null}
          </div>
          <h3 className="lh-name">{leaderName}</h3>
        </div>

        <div className="lh-metrics">
          <div className="m-box">
            <small>Медиана лидера</small>
            <b className={leaderMedian > 0 ? "green" : ""}>
              {leaderMedian > 0 ? f1(leaderMedian) : "—"}
            </b>
            {leaderMedian > 0 ? <span>тик/с</span> : null}
          </div>
          <div className="m-box">
            <small>Перевес</small>
            <b className={marginGreen ? "green" : ""}>{marginText}</b>
          </div>
          <div className="m-box">
            <small>Замерено схем</small>
            <b>{testedCount}</b>
            <span>из {s.schemes.length}</span>
          </div>
          <div className="m-box">
            <small>Прогонов</small>
            <b>{totalRuns}</b>
            <span>всего</span>
          </div>
        </div>
      </section>

      {alerts.length > 0 ? (
        <div className="alerts-strip">
          {alerts.map((a, i) => (
            <div className="alert-chip" key={i}>
              <b>{a.title}</b>
              {a.text}
            </div>
          ))}
        </div>
      ) : null}

      {leader && leader.phases.length > 0 ? (
        <section className="section-card">
          <div className="sc-bar">
            <h4 className="sc-title">Показатели лидера по фазам</h4>
            <span className="sc-hint">
              Сравнение корректно внутри одной фазы · ↓ — снижение частоты CPU
            </span>
          </div>
          <table className="phases-tbl">
            <thead>
              <tr>
                <th>Фаза нагрузки</th>
                <th>Медиана, тик/с</th>
                <th>P1 (мин. 1%), тик/с</th>
                <th>Стабильность</th>
                <th>Частота CPU</th>
              </tr>
            </thead>
            <tbody>
              {leader.phases.map((p, i) => (
                <tr key={p.name}>
                  {/* Номер фазы: в JSON хранится только подпись («Лёгкая»), а
                      в таблице без номера строки не с чем сравнивать. */}
                  <td>
                    {i + 1}. {p.name}
                  </td>
                  <td>
                    <b>{f1(p.median_throughput)}</b>
                  </td>
                  <td>{f1(p.p1_throughput)}</td>
                  <td>{f1(p.consistency_percent, 2)}%</td>
                  <td>
                    {p.frequency_mhz > 0
                      ? `${Math.round(p.frequency_mhz)} МГц${
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
        </section>
      ) : null}

      <section className="section-card">
        <div className="schemes-toolbar">
          <div className="st-left">
            <h4 className="sc-title">Сравнение схем питания</h4>
            {hasEmpty ? (
              <div className="mini-tabs" role="tablist" aria-label="Какие схемы показывать">
                <button
                  type="button"
                  role="tab"
                  aria-selected={tblFilter === "tested"}
                  className={`mtab${tblFilter === "tested" ? " active" : ""}`}
                  onClick={() => setTblFilter("tested")}
                >
                  С замерами ({testedCount})
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={tblFilter === "all"}
                  className={`mtab${tblFilter === "all" ? " active" : ""}`}
                  onClick={() => setTblFilter("all")}
                >
                  Все в сессии ({s.schemes.length})
                </button>
              </div>
            ) : null}
          </div>
          <div className="st-right">
            {hasEmpty ? (
              <label className="hide-empty" title="Схемы без прогонов при ранней остановке">
                <input
                  type="checkbox"
                  checked={tblFilter === "tested"}
                  onChange={(e) => setTblFilter(e.target.checked ? "tested" : "all")}
                />
                Скрыть без замеров
              </label>
            ) : null}
            <div className="mini-tabs">
              <button
                type="button"
                className={`mtab${tblSort === "speed" ? " active" : ""}`}
                title="Сортировка по результату"
                onClick={() => setTblSort("speed")}
              >
                По скорости
              </button>
              <button
                type="button"
                className={`mtab${tblSort === "alpha" ? " active" : ""}`}
                title="Сортировка по названию"
                onClick={() => setTblSort("alpha")}
              >
                По названию
              </button>
            </div>
            <input
              type="search"
              className="mini-search"
              value={schemeQuery}
              onChange={(e) => setSchemeQuery(e.target.value)}
              placeholder="Поиск схемы…"
              aria-label="Поиск схемы в таблице"
            />
          </div>
        </div>

        <div className="schemes-scroll">
          <table className="schemes-tbl">
            <thead>
              <tr>
                <th>Схема питания</th>
                <th>Медиана, тик/с</th>
                <th>ДИ 95%</th>
                <th>Прогонов</th>
                <th>Статус</th>
              </tr>
            </thead>
            <tbody>
              {shownSchemes.length === 0 ? (
                <tr>
                  <td colSpan={5} className="tbl-empty">
                    Ничего не найдено
                  </td>
                </tr>
              ) : (
                shownSchemes.map((sch, i) => {
                  const isLeader = leader != null && sch.scheme_id === leader.scheme_id;
                  const isTested = sch.runs > 0;
                  const pct =
                    isTested && leaderMedian > 0
                      ? Math.max(2, Math.min(100, Math.round((sch.median_throughput / leaderMedian) * 100)))
                      : 0;
                  const name =
                    (sch.name ?? "").trim() ||
                    schemeNames.get(sch.scheme_id.toLowerCase()) ||
                    `Схема ${sch.scheme_id.slice(0, 8)}…`;
                  return (
                    <tr
                      key={sch.scheme_id}
                      className={
                        isLeader ? "is-leader" : !isTested ? "is-untested" : undefined
                      }
                    >
                      <td>
                        <span className="rk-num">#{i + 1}</span>
                        {isLeader ? "★ " : ""}
                        {name}
                        <span className="guid-tail">{sch.scheme_id.slice(0, 8)}…</span>
                      </td>
                      <td>
                        {isTested ? (
                          <span className="score-cell">
                            <span className="score-bar">
                              <i style={{ width: `${pct}%` }} />
                            </span>
                            <b>{f1(sch.median_throughput)}</b>
                          </span>
                        ) : (
                          "—"
                        )}
                      </td>
                      <td className="mut">
                        {isTested && sch.ci_95[0] > 0
                          ? `[${f1(sch.ci_95[0])}; ${f1(sch.ci_95[1])}]`
                          : "—"}
                      </td>
                      <td>{sch.runs}</td>
                      <td>
                        {!isTested ? (
                          // Схема без прогонов: медиана неизвестна, а не ноль.
                          // Причину брака (если она есть) отдаём в подсказке —
                          // иначе строка молча теряет объяснение.
                          <span
                            className="st-badge skipped"
                            title={sch.rejection_reason ?? undefined}
                          >
                            пропущена
                          </span>
                        ) : sch.rejected ? (
                          // Причина брака — главное, ради чего строка и есть:
                          // «забракована» без объяснения ни о чём не говорит.
                          <span
                            className="st-badge rejected"
                            title={sch.rejection_reason ?? undefined}
                          >
                            забракована
                            {sch.rejection_reason ? `: ${sch.rejection_reason}` : ""}
                          </span>
                        ) : isLeader ? (
                          <span className="st-badge leader">лидер</span>
                        ) : (
                          <span className="st-badge">допущена</span>
                        )}
                      </td>
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>
        {rejectedCount > 0 ? (
          <div className="tbl-note">
            Забраковано схем: {rejectedCount}
            {tblFilter === "tested" ? " — они спрятаны вкладкой «Все в сессии»." : "."}
          </div>
        ) : null}
      </section>
    </div>
  );
}
