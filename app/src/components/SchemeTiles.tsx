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
import { Badge, Button, Modal, Seg, Spot } from "./ui";
import {
  MinusCircleIcon,
  MinusIcon,
  PowerIcon,
  SearchIcon,
  StarFilledIcon,
  StarOutlineIcon,
  TrashIcon,
} from "./icons";
import { pushToast } from "../store";

type Filter = "all" | "fav" | "excluded";

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
}: {
  schemes: SchemeRow[];
  selected: Set<string>;
  settings: SettingsDto | null;
  quarantine: QuarantineEntry[];
  onToggle: (guid: string, active: boolean) => void;
  onChanged: () => void;
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

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return [...schemes]
      .filter((s) => {
        if (q && !s.name.toLowerCase().includes(q) && !s.guid.toLowerCase().includes(q)) return false;
        if (filter === "fav" && !favs.has(s.guid.toLowerCase())) return false;
        if (filter === "excluded" && !excluded.has(s.guid.toLowerCase())) return false;
        return true;
      })
      .sort(schemeOrder);
  }, [schemes, query, filter, favs, excluded]);

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

  const activate = (s: SchemeRow) => {
    const q = quarantined.get(s.guid.toLowerCase());
    if (q) {
      pushToast(
        "err",
        `Схема в карантине (${QUARANTINE_LABELS[q.kind] ?? q.kind}): ${q.reason}. Верните её кнопкой на плитке.`,
      );
      return;
    }
    onToggle(s.guid, s.active);
  };

  return (
    <div className="scheme-picker">
      <div className="pick-search">
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
            <button className="search-clear" title="Очистить поиск" onClick={() => setQuery("")}>
              ×
            </button>
          ) : null}
        </div>
        <Seg
          options={[
            { value: "all", label: "Все" },
            { value: "fav", label: "Избранные" },
            { value: "excluded", label: "Исключённые" },
          ]}
          value={filter}
          onChange={setFilter}
        />
        <span className="pick-count">
          {visible.length} из {schemes.length} · выбрано {selected.size}
        </span>
      </div>

      {visible.length === 0 ? (
        <div className="glass inset">
          <div className="muted">
            {query.trim()
              ? `По запросу «${query.trim()}» ничего не найдено.`
              : "Нет схем для отображения."}
          </div>
        </div>
      ) : (
        <div className="pick-grid">
          {visible.map((s) => {
            const key = s.guid.toLowerCase();
            const isFav = favs.has(key);
            const isEx = excluded.has(key);
            const q = quarantined.get(key);
            const on = selected.has(s.guid);
            const cls = [
              "pick-tile",
              on ? "on" : "",
              q ? "quarantined" : "",
              !q && isEx ? "excluded" : "",
            ]
              .filter(Boolean)
              .join(" ");
            return (
              <Spot
                key={s.guid}
                className={cls}
                role="button"
                tabIndex={0}
                // Плитка работает и на Space, иначе с клавиатуры её не выбрать.
                // `aria-pressed` сообщает скринридеру, что это переключатель.
                aria-pressed={on && !q}
                aria-label={`${s.name || "Схема без названия"}${on ? " — выбрана" : ""}`}
                title={q ? `Карантин (${QUARANTINE_LABELS[q.kind] ?? q.kind}): ${q.reason}` : undefined}
                onClick={() => activate(s)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    activate(s);
                  }
                }}
              >
                <div className="scheme-card-name">{s.name || "Без названия"}</div>
                <div className="scheme-card-badges">
                  {s.active ? <Badge kind="ok">АКТИВНА</Badge> : null}
                  {q ? (
                    <Badge kind="danger" title={q.reason}>
                      карантин · {QUARANTINE_LABELS[q.kind] ?? q.kind}
                    </Badge>
                  ) : isEx ? (
                    <Badge kind="plain">исключена</Badge>
                  ) : null}
                </div>
                <div className="scheme-card-id" title="ID схемы">
                  {s.guid}
                </div>
                {q ? <div className="pick-why">{q.reason}</div> : null}
                <div className="scheme-card-foot">
                  <span className="scheme-card-state">
                    <span className="tick">{on && !q ? "✓" : ""}</span>
                    {q ? "в карантине" : on ? "Выбрано" : "Выбрать"}
                  </span>
                  <span className="ts-grow" />
                  {q ? (
                    <button
                      type="button"
                      className="icon-btn"
                      title="Вернуть из карантина"
                      aria-label={`Вернуть «${s.name || s.guid}» из карантина`}
                      onClick={(e) => {
                        e.stopPropagation();
                        restore(s.guid);
                      }}
                    >
                      <MinusIcon />
                    </button>
                  ) : (
                    <>
                      <button
                        type="button"
                        className={`icon-btn${isFav ? " on-fav" : ""}`}
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
                      <button
                        type="button"
                        className={`icon-btn${isEx ? " on-ex" : ""}`}
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
                    </>
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
  const [confirm, setConfirm] = useState<Confirm>(null);

  const favs = useMemo(
    () => new Set((settings?.favorite_schemes ?? []).map((g) => g.toLowerCase())),
    [settings],
  );
  const excluded = useMemo(
    () => new Set((settings?.excluded_schemes ?? []).map((g) => g.toLowerCase())),
    [settings],
  );

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return schemes
      .filter((s) => {
        if (q && !s.name.toLowerCase().includes(q) && !s.guid.toLowerCase().includes(q))
          return false;
        if (filter === "fav" && !favs.has(s.guid.toLowerCase())) return false;
        if (filter === "excluded" && !excluded.has(s.guid.toLowerCase())) return false;
        return true;
      })
      .sort(schemeOrder);
  }, [schemes, query, filter, favs, excluded]);

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

  const runAction = async (kind: string, guid?: string | null, path?: string | null) => {
    try {
      const res = await commands.schemeAction(kind, guid ?? null, path ?? null);
      pushToast(
        "okk",
        kind === "activate" ? "Схема активирована" : kind === "duplicate" ? `Дубль: ${res}` : "Готово",
      );
      onChanged();
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

  return (
    <div>
      <div className="wizard-toolbar" style={{ marginBottom: 10 }}>
        <div className="search-box">
          <SearchIcon />
          <input
            id="schemes-search"
            className="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Поиск схемы по имени или GUID…"
            aria-label="Поиск схемы питания по имени или GUID"
          />
        </div>
        <Seg
          options={[
            { value: "all", label: "Все" },
            { value: "fav", label: "Избранные" },
            { value: "excluded", label: "Исключённые" },
          ]}
          value={filter}
          onChange={setFilter}
        />
      </div>
      <div className="scheme-tiles">
        {visible.map((s) => {
          const isFav = favs.has(s.guid.toLowerCase());
          const isEx = excluded.has(s.guid.toLowerCase());
          const isSel = exportTarget === s.guid;
          return (
            <div
              key={s.guid}
              className={`tile-scheme${s.active ? " active" : ""}${isSel ? " sel" : ""}${isEx ? " excluded" : ""}`}
              // Выбор для экспорта — тоже интерактивный элемент: раньше это
              // был `<div>` без роли и клавиатуры.
              role={onSelectExport ? "button" : undefined}
              tabIndex={onSelectExport ? 0 : undefined}
              aria-pressed={isSel}
              onClick={() => onSelectExport?.(s.guid)}
              onKeyDown={(e) => {
                if (!onSelectExport) return;
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onSelectExport(s.guid);
                }
              }}
              title="Клик — выбрать для экспорта"
            >
              <div className="scheme-card-name">{s.name || "Без названия"}</div>
              {/* Порядок блоков и сами стили — те же, что у плитки в мастере
                  бенчмарка: окно выбора схем и страница «Схемы» должны
                  выглядеть одинаково, иначе их невозможно сопоставлять. */}
              <div className="scheme-card-badges">
                {s.active ? <Badge kind="ok">АКТИВНА</Badge> : null}
                {isEx ? <Badge kind="plain">исключена</Badge> : null}
              </div>
              <div className="scheme-card-id" title="ID схемы">
                {s.guid}
              </div>
              <div className="scheme-card-foot">
                <button
                  type="button"
                  className="icon-btn ts-act"
                  title={s.active ? "Уже активна" : "Сделать активной"}
                  aria-label={`Сделать схемой по умолчанию: ${s.name || s.guid}`}
                  disabled={s.active || !isAdmin || running}
                  onClick={(e) => {
                    e.stopPropagation();
                    void runAction("activate", s.guid);
                  }}
                >
                  <PowerIcon />
                </button>
                <button
                  type="button"
                  className={`icon-btn${isFav ? " on-fav" : ""}`}
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
                <button
                  type="button"
                  className={`icon-btn${isEx ? " on-ex" : ""}`}
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
                <span className="ts-grow" />
                <button
                  type="button"
                  className="icon-btn on-del"
                  title="Удалить схему"
                  aria-label={`Удалить схему «${s.name || s.guid}»`}
                  disabled={!isAdmin || running}
                  onClick={(e) => {
                    e.stopPropagation();
                    setConfirm({ kind: "delete", guid: s.guid, name: s.name });
                  }}
                >
                  <TrashIcon />
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {visible.length === 0 ? (
        <div className="glass inset">
          {/* Разные причины пустого списка требуют разных действий, поэтому
              сообщаем именно ту, что сработала. */}
          <div className="muted">
            {query.trim()
              ? `По запросу «${query.trim()}» ничего не найдено.`
              : filter === "fav"
                ? "Нет избранных схем. Отметьте звездой на плитке."
                : filter === "excluded"
                  ? "Нет исключённых схем."
                  : "Нет схем для отображения."}
          </div>
        </div>
      ) : null}
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
