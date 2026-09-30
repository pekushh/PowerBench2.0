// Плитки схем питания: поиск, фильтры, избранное, исключения,
// активация, удаление, экспорт, дублирование.

import { useMemo, useRef, useState } from "react";
import {
  QUARANTINE_LABELS,
  commands,
  type QuarantineEntry,
  type SchemeRow,
  type SettingsDto,
} from "../api";
import {
  ExportIcon,
  GridIcon,
  ListViewIcon,
  MinusCircleIcon,
  RestoreIcon,
  SearchIcon,
  StarFilledIcon,
  StarOutlineIcon,
  TrashIcon,
} from "./icons";
import { Button, Modal, Spot } from "./ui";
import { pushToast } from "../store";
import { usePill } from "./usePill";
import { usePopOnFilter } from "./useCascade";

type Filter = "all" | "fav" | "excluded" | "dup";

// Ветка `restore` удалена как мёртвая: восстановление стандартных схем живёт
// на странице «Схемы» (SchemesPage), а отсюда `setConfirm` вызывался только с
// `kind: "delete"`.
type Confirm = { kind: "delete"; guid: string; name: string } | null;

/**
 * Порядок показа: строго по алфавиту (русская локаль), при равенстве имён —
 * по GUID, чтобы порядок не зависел от ответа `powercfg`.
 *
 * Раньше активная схема поднималась наверх. Из-за этого список менялся
 * целиком в момент, когда пользователь переключал схему, а страница «Схемы»
 * и выбор в бенчмарке выглядели по-разному. Теперь «активная» — это метка на
 * плитке, а не её позиция: и обе страницы, и два окна показывают один и тот
 * же список в одном порядке.
 *
 * Порядок *измерения* при этом остаётся прежним: план строит
 * `config::canonical_scheme_order`, который первым ставит активную схему —
 * это нужно для восстановления состояния после прерывания сессии.
 */
function schemeOrder(a: SchemeRow, b: SchemeRow): number {
  const byName = (a.name || "").localeCompare(b.name || "", "ru", { sensitivity: "base" });
  if (byName !== 0) return byName;
  return a.guid.localeCompare(b.guid);
}

/** Отсортированная копия списка схем. Компаратор был продублирован в трёх
 *  местах, из-за чего страницы показывали схемы в разном порядке. */
export function sortSchemes(list: SchemeRow[]): SchemeRow[] {
  return [...list].sort(schemeOrder);
}

/**
 * Схемы, доступные для бенчмарка: исключённые пользователем и карантинные
 * выпадают. Сравнение по GUID регистронезависимое — GUID в Windows бывают
 * в разном регистре.
 */
export function filterEligible(
  list: SchemeRow[],
  excludedGuids: readonly string[],
  quarantine: readonly QuarantineEntry[],
): SchemeRow[] {
  const excluded = new Set(excludedGuids.map((g) => g.toLowerCase()));
  const quarantined = new Set(quarantine.map((q) => q.scheme_id.toLowerCase()));
  return list.filter((x) => {
    const k = x.guid.toLowerCase();
    return !excluded.has(k) && !quarantined.has(k);
  });
}

/**
 * Выбор схем для бенчмарка: та же плитка, что на странице «Схемы питания»
 * (поиск, избранное, исключение, карантин), но клик по плитке включает
 * схему в сравнение, а не выбирает её для экспорта.
 */
export function SchemePicker({
  schemes,
  selected,
  settings,
  quarantine,
  onToggle,
  onChanged,
  onToggleMany,
  estimate,
  oneScheme,
  detailed,
}: {
  schemes: SchemeRow[];
  selected: Set<string>;
  settings: SettingsDto | null;
  quarantine: QuarantineEntry[];
  onToggle: (guid: string, active: boolean) => void;
  onChanged: () => void;
  /** Заменить весь набор выбранных схем (быстрые действия над списком). */
  onToggleMany: (guids: string[]) => void;
  /** Оценка времени всей сессии для полосы-сводки. */
  estimate?: string;
  /** Оценка времени одной схемы: нужна для подсказки про режим «Быстро». */
  oneScheme?: string;
  /** Выбран детальный режим — подсказка про >20 схем показывается в нём. */
  detailed?: boolean;
}) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");

  const favs = useMemo(
    () => new Set((settings?.favorite_schemes ?? []).map((g) => g.toLowerCase())),
    [settings],
  );
  const excluded = useMemo(
    () => new Set((settings?.excluded_schemes ?? []).map((g) => g.toLowerCase())),
    [settings],
  );
  const quarantined = useMemo(() => {
    const m = new Map<string, QuarantineEntry>();
    for (const q of quarantine) m.set(q.scheme_id.toLowerCase(), q);
    return m;
  }, [quarantine]);
  const pickerFilterPill = usePill(filter, [schemes.length, favs.size, excluded.size]);
  const pickerPopRef = usePopOnFilter<HTMLDivElement>(query + filter);

  const copies = useMemo(() => copyMark(schemes), [schemes]);

  // Активная схема идёт первой: она стартует первой же, а при афавитной
  // сортировке её приходилось искать в списке из сотни строк.
  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return [...schemes]
      .filter((s) => {
        if (q && !s.name.toLowerCase().includes(q) && !s.guid.toLowerCase().includes(q)) return false;
        if (filter === "fav" && !favs.has(s.guid.toLowerCase())) return false;
        if (filter === "excluded" && !excluded.has(s.guid.toLowerCase())) return false;
        return true;
      })
      .sort((a, b) => Number(b.active) - Number(a.active) || schemeOrder(a, b));
  }, [schemes, query, filter, favs, excluded]);

  // Наборы для быстрых действий над списком.
  const eligibleGuids = useMemo(
    () => filterEligible(schemes, settings?.excluded_schemes ?? [], quarantine).map((s) => s.guid),
    [schemes, settings, quarantine],
  );
  // «Без дубликатов»: из группы одноимённых остаётся первая, вторая и
  // дальше снимаются — иначе одна и та же схема меряется дважды.
  const firstCopyGuids = useMemo(
    () => schemes.filter((s) => (copies.get(s.guid)?.copy ?? 1) === 1).map((s) => s.guid),
    [schemes, copies],
  );
  const activeGuids = useMemo(
    () => schemes.filter((s) => s.active).map((s) => s.guid),
    [schemes],
  );

  // Очередь записи: два быстрых щелчка читали один и тот же снимок `settings`,
  // и вторая запись затирала первую. Состояние читается в момент записи.
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const patch = (p: Partial<SettingsDto>) => {
    queue.current = queue.current
      .then(async () => commands.setSettings({ ...(await commands.getSettings()), ...p }))
      .then(onChanged)
      .catch((e: unknown) => {
        pushToast("err", String(e));
      });
  };

  /** Переключить GUID в списке, не трогая порядок и регистр остальных. */
  function flip(list: string[] | undefined, guid: string): string[] {
    const cur = list ?? [];
    const has = cur.some((g) => g.toLowerCase() === guid.toLowerCase());
    return has ? cur.filter((g) => g.toLowerCase() !== guid.toLowerCase()) : [...cur, guid];
  }

  const toggleFav = (guid: string) => {
    patch({ favorite_schemes: flip(settings?.favorite_schemes, guid) });
  };

  const toggleExcluded = (guid: string) => {
    patch({ excluded_schemes: flip(settings?.excluded_schemes, guid) });
  };

  const restore = (guid: string) => {
    commands
      .quarantineClear(guid)
      .then((was) => {
        pushToast("okk", was ? "Схема возвращена в бенчмарк" : "Схема уже вне карантина");
        onChanged();
      })
      .catch((e) => pushToast("err", String(e)));
  };

  // Карточка схемы: клик по всей площади переключает выбор, поэтому клик
  // мимо кнопок должен честно сказать, что схема в карантине.
  const pick = (s: SchemeRow) => {
    const q = quarantined.get(s.guid.toLowerCase());
    if (q) {
      pushToast(
        "err",
        `Схема в карантине (${QUARANTINE_LABELS[q.kind] ?? q.kind}): ${q.reason}. Верните её кнопкой на карточке.`,
      );
      return;
    }
    onToggle(s.guid, s.active);
  };

  return (
    <div className="scheme-picker">
      <div className="step2-bar">
        <div className="s2-left">
          <div className="search-box">
            <SearchIcon />
            <input
              className="search"
              placeholder="Поиск схемы по имени или GUID…"
              aria-label="Поиск схемы питания"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            {query ? (
              <button
                type="button"
                className="search-clear"
                title="Очистить поиск"
                onClick={() => setQuery("")}
              >
                ×
              </button>
            ) : null}
          </div>
          <div
            className="filter-tabs pb-pill-host"
            role="tablist"
            aria-label="Фильтр схем"
            ref={pickerFilterPill.ref}
          >
            {(
              [
                { value: "all", label: "Все", count: schemes.length },
                { value: "fav", label: "Избранные", count: favs.size },
                { value: "excluded", label: "Исключённые", count: excluded.size },
              ] as { value: Filter; label: string; count: number }[]
            ).map((t) => (
              <button
                key={t.value}
                type="button"
                role="tab"
                aria-selected={filter === t.value}
                className={`ftab${filter === t.value ? " active" : ""}`}
                data-value={t.value}
                onClick={() => setFilter(t.value)}
              >
                <span>{t.label}</span>
                <span className="cnt">{t.count}</span>
              </button>
            ))}
          </div>
        </div>
        <div className="s2-actions">
          <button
            type="button"
            className="act-pill"
            onClick={() => onToggleMany(eligibleGuids)}
          >
            Выбрать все
          </button>
          <button
            type="button"
            className="act-pill"
            title="Снять выбор со вторых копий одноимённых схем"
            onClick={() => onToggleMany(firstCopyGuids)}
          >
            Без дубликатов
          </button>
          <button
            type="button"
            className="act-pill"
            title="Оставить только активную схему"
            onClick={() => onToggleMany(activeGuids)}
          >
            Только активная
          </button>
          <button type="button" className="act-pill" onClick={() => onToggleMany([])}>
            Снять все
          </button>
        </div>
      </div>

      <div className="sel-summary-strip">
        <span>
          Выбрано схем: <b>{selected.size}</b> из {schemes.length}
        </span>
        <div className="sel-summary-right">
          <span>
            Оценка времени: <span className="est">{estimate ?? "—"}</span>
          </span>
          {detailed && selected.size > 20 ? (
            <span className="time-tip">
              Совет: для &gt;20 схем начните с режима «Быстро» ({oneScheme ?? "—"} на схему)
            </span>
          ) : null}
        </div>
      </div>

      {visible.length === 0 ? (
        <div className="glass inset">
          <div className="muted">
            {schemes.length === 0
              ? "Схемы питания не найдены. Проверьте, что PowerBench запущен от имени администратора."
              : query.trim()
                ? "По этому запросу ничего не найдено."
                : "В этой вкладке нет схем."}
          </div>
        </div>
      ) : (
        <div className="pick-grid" ref={pickerPopRef}>
          {visible.map((s) => {
            const key = s.guid.toLowerCase();
            const isFav = favs.has(key);
            const isEx = excluded.has(key);
            const q = quarantined.get(key);
            const on = selected.has(s.guid);
            const copy = copies.get(s.guid);
            return (
              <Spot
                key={s.guid}
                className={`pick-card${on ? " checked" : ""}${s.active ? " is-active-scheme" : ""}`}
                role="checkbox"
                aria-checked={on}
                tabIndex={0}
                aria-label={`${s.name || "Схема без названия"}${on ? " — выбрана" : ""}`}
                title={q ? `Карантин (${QUARANTINE_LABELS[q.kind] ?? q.kind}): ${q.reason}` : undefined}
                onClick={() => pick(s)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    pick(s);
                  }
                }}
              >
                <div className="pc-left">
                  <span className="chk-box">✓</span>
                  <div className="pc-info">
                    <div className="pc-name-row">
                      <span className="pc-name">{s.name || "Без названия"}</span>
                      {s.active ? <span className="badge-active">Активна</span> : null}
                    </div>
                    <div className="pc-meta">
                      {s.guid.slice(0, 8)}…
                      {s.active ? " · стартует первой" : ""}
                      {copy && copy.of > 1 ? ` · копия #${copy.copy} из ${copy.of}` : ""}
                      {q ? ` · карантин: ${QUARANTINE_LABELS[q.kind] ?? q.kind}` : ""}
                    </div>
                  </div>
                </div>
                <div className="pc-quick">
                  <button
                    type="button"
                    className={`mini-ic${isFav ? " on" : ""}`}
                    title={isFav ? "Убрать из избранного" : "В избранное"}
                    aria-label={
                      isFav
                        ? `Убрать «${s.name || s.guid}» из избранного`
                        : `Добавить «${s.name || s.guid}» в избранное`
                    }
                    aria-pressed={isFav}
                    onClick={(e) => {
                      e.stopPropagation();
                      toggleFav(s.guid);
                    }}
                  >
                    {isFav ? <StarFilledIcon /> : <StarOutlineIcon />}
                  </button>
                  {q ? (
                    <button
                      type="button"
                      className="mini-ic warn"
                      title="Вернуть из карантина"
                      aria-label={`Вернуть «${s.name || s.guid}» из карантина`}
                      onClick={(e) => {
                        e.stopPropagation();
                        restore(s.guid);
                      }}
                    >
                      <RestoreIcon />
                    </button>
                  ) : (
                    <button
                      type="button"
                      className={`mini-ic${isEx ? " on ex" : ""}`}
                      title={isEx ? "Включить в бенчмарк" : "Исключить из бенчмарка"}
                      aria-label={
                        isEx
                          ? `Вернуть «${s.name || s.guid}» в бенчмарк`
                          : `Исключить «${s.name || s.guid}» из бенчмарка`
                      }
                      aria-pressed={isEx}
                      onClick={(e) => {
                        e.stopPropagation();
                        toggleExcluded(s.guid);
                      }}
                    >
                      <MinusCircleIcon />
                    </button>
                  )}
                </div>
              </Spot>
            );
          })}
        </div>
      )}
    </div>
  );
}

/**
 * Три встроенные схемы Windows. Сверяем по GUID, а не по названию: названия
 * локализованы («Сбалансированная», «Balanced», «Equilibrato»), а GUID у них
 * неизменны и служат для этого официально.
 */
const WINDOWS_BUILTIN_GUIDS = [
  "381b4222-f694-41f0-9685-ff5bb260df2e", // Сбалансированная
  "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c", // Высокая производительность
  "a1841308-3541-4fab-bc81-f71556f20b4a", // Экономия энергии
];

/**
 * Номер копии внутри группы одноимённых схем и размер группы.
 *
 * Копии — это разные люди, назвавшие схему одинаково. Схемы с одинаковым
 * именем измеряются отдельно, но выбирать придётся одну, и без метки нельзя
 * понять, какая именно. Первую в группе считаем оригиналом и не помечаем,
 * остальным показываем «Копия #k/m».
 */
function copyMark(list: SchemeRow[]): Map<string, { copy: number; of: number }> {
  const groups = new Map<string, SchemeRow[]>();
  for (const s of list) {
    const key = (s.name || "").trim().toLowerCase();
    if (!key) continue;
    const g = groups.get(key);
    if (g) g.push(s);
    else groups.set(key, [s]);
  }
  const out = new Map<string, { copy: number; of: number }>();
  for (const g of groups.values()) {
    if (g.length < 2) continue;
    g.forEach((s, i) => out.set(s.guid, { copy: i + 1, of: g.length }));
  }
  return out;
}

export default function SchemeTiles({
  schemes,
  settings,
  isAdmin,
  running,
  exportTarget,
  onSelectExport,
  onChanged,
}: {
  schemes: SchemeRow[];
  settings: SettingsDto | null;
  isAdmin: boolean;
  running: boolean;
  exportTarget?: string | null;
  onSelectExport?: (guid: string | null) => void;
  onChanged: () => void;
}) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [view, setView] = useState<"grid" | "list">("grid");
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [copied, setCopied] = useState<string | null>(null);

  const favs = useMemo(
    () => new Set((settings?.favorite_schemes ?? []).map((g) => g.toLowerCase())),
    [settings],
  );
  const excluded = useMemo(
    () => new Set((settings?.excluded_schemes ?? []).map((g) => g.toLowerCase())),
    [settings],
  );
  const pageFilterPill = usePill(filter, [schemes.length, favs.size, excluded.size]);
  const viewPill = usePill(view);
  const pagePopRef = usePopOnFilter<HTMLDivElement>(query + filter);
  const copies = useMemo(() => copyMark(schemes), [schemes]);

  const counts = useMemo(
    () => ({
      all: schemes.length,
      fav: schemes.filter((s) => favs.has(s.guid.toLowerCase())).length,
      excluded: schemes.filter((s) => excluded.has(s.guid.toLowerCase())).length,
      dup: copies.size,
    }),
    [schemes, favs, excluded, copies],
  );

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return schemes
      .filter((s) => {
        if (q && !s.name.toLowerCase().includes(q) && !s.guid.toLowerCase().includes(q))
          return false;
        if (filter === "fav" && !favs.has(s.guid.toLowerCase())) return false;
        if (filter === "excluded" && !excluded.has(s.guid.toLowerCase())) return false;
        if (filter === "dup" && !copies.has(s.guid)) return false;
        return true;
      })
      // Активная схема идёт первой: её ищут чаще всего, а при афавибной
      // сортировке она оказывалась в середине списка из сотни строк.
      .sort((a, b) => Number(b.active) - Number(a.active) || schemeOrder(a, b));
  }, [schemes, query, filter, favs, excluded, copies]);

  // Записи настроек выстраиваются в очередь: два быстрых щелчка раньше читали
  // один и тот же снимок `settings`, и вторая запись затирала первую.
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  function patchSettings(patch: Partial<SettingsDto>) {
    queue.current = queue.current
      .then(async () => {
        // Свежее состояние читаем в момент записи, а не из замыкания.
        const cur = await commands.getSettings();
        await commands.setSettings({ ...cur, ...patch });
      })
      .then(onChanged)
      .catch((e: unknown) => {
        pushToast("err", String(e));
      });
  }

  /** Переключить GUID в списке, сохраняя порядок и регистр остальных. */
  function flip(list: string[] | undefined, guid: string): string[] {
    const cur = list ?? [];
    const has = cur.some((g) => g.toLowerCase() === guid.toLowerCase());
    return has ? cur.filter((g) => g.toLowerCase() !== guid.toLowerCase()) : [...cur, guid];
  }

  const toggleFav = (guid: string) => {
    patchSettings({ favorite_schemes: flip(settings?.favorite_schemes, guid) });
  };

  const toggleExcluded = (guid: string) => {
    patchSettings({ excluded_schemes: flip(settings?.excluded_schemes, guid) });
  };

  const activate = async (s: SchemeRow) => {
    if (s.active) return;
    try {
      await commands.schemeAction("activate", s.guid, null);
      pushToast("okk", `Схема «${s.name || s.guid}» активирована`);
      onChanged();
    } catch (e) {
      pushToast("err", String(e));
    }
  };

  // Полный GUID в буфер обмена. В карточке он показан коротким: 36 символов
  // занимали строку целиком, а для `powercfg` нужен именно полный.
  const copyGuid = async (guid: string) => {
    try {
      await navigator.clipboard.writeText(guid);
      setCopied(guid);
      window.setTimeout(() => setCopied((cur) => (cur === guid ? null : cur)), 1000);
    } catch (e) {
      pushToast("err", String(e));
    }
  };

  const doDelete = async (guid: string) => {
    try {
      await commands.schemeAction("delete", guid, null);
      pushToast("okk", "Схема удалена");
      if (exportTarget === guid) onSelectExport?.(null);
      onChanged();
    } catch (e) {
      pushToast("err", String(e));
    }
  };

  const tabs: { value: Filter; label: string; count: number }[] = [
    { value: "all", label: "Все", count: counts.all },
    { value: "fav", label: "Избранные", count: counts.fav },
    { value: "excluded", label: "Исключённые", count: counts.excluded },
    { value: "dup", label: "Дубликаты", count: counts.dup },
  ];

  return (
      <div
        className={`schemes-block${schemes.length === 0 ? " is-loading" : ""}`}
      >
      <div className="schemes-toolbar">
        <div className="toolbar-left">
          <div className="search-box sch-search">
            <SearchIcon />
            <input
              id="schemes-search"
              className="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Поиск по названию или GUID…"
              aria-label="Поиск схемы питания по названию или GUID"
            />
          </div>
          <div
            className="filter-tabs pb-pill-host"
            role="tablist"
            aria-label="Фильтр схем"
            ref={pageFilterPill.ref}
          >
            {tabs.map((t) => (
              <button
                key={t.value}
                type="button"
                role="tab"
                aria-selected={filter === t.value}
                className={`ftab${filter === t.value ? " active" : ""}`}
                data-value={t.value}
                onClick={() => setFilter(t.value)}
              >
                <span>{t.label}</span>
                <span className="cnt">{t.count}</span>
              </button>
            ))}
          </div>
        </div>
        <div
          className="view-switch pb-pill-host"
          role="group"
          aria-label="Вид списка"
          ref={viewPill.ref}
        >
          <button
            type="button"
            className={`vbtn${view === "grid" ? " active" : ""}`}
            data-value="grid"
            title="Сетка карточек"
            aria-pressed={view === "grid"}
            onClick={() => setView("grid")}
          >
            <GridIcon />
          </button>
          <button
            type="button"
            className={`vbtn${view === "list" ? " active" : ""}`}
            data-value="list"
            title="Компактный список"
            aria-pressed={view === "list"}
            onClick={() => setView("list")}
          >
            <ListViewIcon />
          </button>
        </div>
      </div>

      {visible.length === 0 ? (
        <div className="empty-card">
          {query.trim()
            ? `По запросу «${query.trim()}» ничего не найдено.`
            : filter === "fav"
              ? "Нет избранных схем. Отметьте звездой на карточке."
              : filter === "excluded"
                ? "Нет исключённых схем."
                : filter === "dup"
                  ? "Схем с одинаковыми названиями нет."
                  : "Нет схем для отображения."}
        </div>
      ) : (
        <div className={`schemes-grid${view === "list" ? " list-mode" : ""}`} ref={pagePopRef}>
          {visible.map((s) => {
            const isFav = favs.has(s.guid.toLowerCase());
            const isEx = excluded.has(s.guid.toLowerCase());
            const isSel = exportTarget === s.guid;
            const copy = copies.get(s.guid);
            const isWin = WINDOWS_BUILTIN_GUIDS.includes(s.guid.toLowerCase());
            const canAct = isAdmin && !running;
            return (
              <article
                key={s.guid}
                className={`scheme-card${s.active ? " is-active" : ""}${
                  isEx ? " is-excluded" : ""
                }${isSel ? " is-sel" : ""}`}
              >
                <div className="sc-head">
                  <span className="sc-name" title={s.name || s.guid}>
                    {s.name || "Без названия"}
                  </span>
                  <span className="sc-head-tags">
                    {copy ? (
                      <span
                        // Первая схема группы — нейтральный бейдж, остальные
                        // подсвечены: без «#1» пара выглядела как одна копия
                        // с потерянным оригиналом.
                        className={`sc-badge ${copy.copy > 1 ? "dup" : "orig"}`}
                        title={`Схем с таким именем: ${copy.of}`}
                      >
                        Копия #{copy.copy}/{copy.of}
                      </span>
                    ) : null}
                    {isWin ? (
                      <span className="sc-badge sys" title="Встроенная схема Windows">
                        Windows
                      </span>
                    ) : null}
                    {isEx ? <span className="sc-badge ex">исключена</span> : null}
                  </span>
                </div>

                <div className="sc-foot">
                  <div className="sc-left-act">
                    <button
                      type="button"
                      className="btn-power"
                      disabled={s.active || !canAct}
                      title={
                        s.active
                          ? "Схема уже активна"
                          : !isAdmin
                            ? "Требуются права администратора"
                            : running
                              ? "Во время замера схемы менять нельзя"
                              : "Сделать активной"
                      }
                      onClick={() => void activate(s)}
                    >
                      {s.active ? "✓ Активна" : "Включить"}
                    </button>
                    <button
                      type="button"
                      className="guid-chip"
                      title={`Копировать GUID: ${s.guid}`}
                      onClick={() => void copyGuid(s.guid)}
                    >
                      {copied === s.guid ? "Скопировано" : `${s.guid.slice(0, 8)}…`}
                    </button>
                    {onSelectExport ? (
                      <button
                        type="button"
                        className={`sc-export${isSel ? " on" : ""}`}
                        title={isSel ? "Выбрана для экспорта" : "Выбрать для экспорта и дублирования"}
                        aria-pressed={isSel}
                        onClick={() => onSelectExport(isSel ? null : s.guid)}
                      >
                        <ExportIcon />
                      </button>
                    ) : null}
                  </div>
                  <div className="sc-icons">
                    <button
                      type="button"
                      className={`ic-btn fav${isFav ? " on" : ""}`}
                      title={isFav ? "Убрать из избранного" : "В избранное"}
                      aria-label={
                        isFav
                          ? `Убрать «${s.name || s.guid}» из избранного`
                          : `Добавить «${s.name || s.guid}» в избранное`
                      }
                      aria-pressed={isFav}
                      onClick={() => toggleFav(s.guid)}
                    >
                      {isFav ? <StarFilledIcon /> : <StarOutlineIcon />}
                    </button>
                    <button
                      type="button"
                      className={`ic-btn excl${isEx ? " on" : ""}`}
                      title={isEx ? "Включить в бенчмарк" : "Исключить из бенчмарка"}
                      aria-label={
                        isEx
                          ? `Вернуть «${s.name || s.guid}» в бенчмарк`
                          : `Исключить «${s.name || s.guid}» из бенчмарка`
                      }
                      aria-pressed={isEx}
                      onClick={() => toggleExcluded(s.guid)}
                    >
                      <MinusCircleIcon />
                    </button>
                    <button
                      type="button"
                      className="ic-btn del"
                      title="Удалить схему"
                      aria-label={`Удалить схему «${s.name || s.guid}»`}
                      disabled={!canAct}
                      onClick={() => setConfirm({ kind: "delete", guid: s.guid, name: s.name })}
                    >
                      <TrashIcon />
                    </button>
                  </div>
                </div>
              </article>
            );
          })}
        </div>
      )}

      <Modal
        open={confirm !== null}
        title="Удалить схему?"
        onClose={() => setConfirm(null)}
        footer={
          confirm ? (
            <>
              <Button variant="ghost" onClick={() => setConfirm(null)}>
                Отмена
              </Button>
              <Button
                variant="danger"
                disabled={running}
                onClick={() => {
                  const c = confirm;
                  setConfirm(null);
                  void doDelete(c.guid);
                }}
              >
                Удалить
              </Button>
            </>
          ) : null
        }
      >
        {confirm ? (
          <p className="hint">
            Удалить схему <span className="strong">«{confirm.name}»</span>? Действие необратимо.
          </p>
        ) : null}
      </Modal>
    </div>
  );
}
