// Плитки схем питания: поиск, фильтры, избранное, исключения,
// активация, удаление, экспорт, дублирование.

import { useMemo, useState } from "react";
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

type Confirm = { kind: "delete"; guid: string; name: string } | { kind: "restore" } | null;

/** Сравнение по алфавиту (русская локаль), безымянные — в конец. */
function byName(a: SchemeRow, b: SchemeRow): number {
  const an = (a.name || "").trim();
  const bn = (b.name || "").trim();
  if (!an && !bn) return a.guid.localeCompare(b.guid);
  if (!an) return 1;
  if (!bn) return -1;
  return an.localeCompare(bn, "ru", { sensitivity: "base" });
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
      .sort(byName);
  }, [schemes, query, filter, favs, excluded]);

  const patch = async (p: Partial<SettingsDto>) => {
    try {
      const cur = settings ?? (await commands.getSettings());
      await commands.setSettings({ ...cur, ...p });
      onChanged();
    } catch (e) {
      pushToast("err", String(e));
    }
  };

  const toggleFav = (guid: string) => {
    const cur = settings?.favorite_schemes ?? [];
    const has = cur.some((g) => g.toLowerCase() === guid.toLowerCase());
    void patch({
      favorite_schemes: has
        ? cur.filter((g) => g.toLowerCase() !== guid.toLowerCase())
        : [...cur, guid],
    });
  };

  const toggleExcluded = (guid: string) => {
    const cur = settings?.excluded_schemes ?? [];
    const has = cur.some((g) => g.toLowerCase() === guid.toLowerCase());
    void patch({
      excluded_schemes: has
        ? cur.filter((g) => g.toLowerCase() !== guid.toLowerCase())
        : [...cur, guid],
    });
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
                title={q ? `Карантин (${QUARANTINE_LABELS[q.kind] ?? q.kind}): ${q.reason}` : undefined}
                onClick={() => activate(s)}
                onKeyDown={(e) => e.key === "Enter" && activate(s)}
              >
                <div className="pick-name">{s.name || "Без названия"}</div>
                <div className="pick-badges">
                  {s.active ? <Badge kind="ok">АКТИВНА</Badge> : null}
                  {q ? (
                    <Badge kind="danger" title={q.reason}>
                      карантин · {QUARANTINE_LABELS[q.kind] ?? q.kind}
                    </Badge>
                  ) : isEx ? (
                    <Badge kind="plain">исключена</Badge>
                  ) : null}
                </div>
                <div className="scheme-id" title="ID схемы">
                  {s.guid}
                </div>
                {q ? <div className="pick-why">{q.reason}</div> : null}
                <div className="pick-foot">
                  <span className="pick-state">
                    <span className="tick">{on && !q ? "✓" : ""}</span>
                    {q ? "в карантине" : on ? "Выбрано" : "Выбрать"}
                  </span>
                  <span className="ts-grow" />
                  {q ? (
                    <button
                      className="icon-btn"
                      title="Вернуть из карантина"
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
                        className={`icon-btn${isFav ? " on-fav" : ""}`}
                        title={isFav ? "Убрать из избранного" : "В избранное"}
                        onClick={(e) => {
                          e.stopPropagation();
                          toggleFav(s.guid);
                        }}
                      >
                        {isFav ? <StarFilledIcon /> : <StarOutlineIcon />}
                      </button>
                      <button
                        className={`icon-btn${isEx ? " on-ex" : ""}`}
                        title={isEx ? "Включить в бенчмарк" : "Исключить из бенчмарка"}
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

  const visible = schemes.filter((s) => {
    const q = query.trim().toLowerCase();
    if (q && !s.name.toLowerCase().includes(q) && !s.guid.toLowerCase().includes(q)) return false;
    if (filter === "fav" && !favs.has(s.guid.toLowerCase())) return false;
    if (filter === "excluded" && !excluded.has(s.guid.toLowerCase())) return false;
    return true;
  });

  async function patchSettings(patch: Partial<SettingsDto>) {
    try {
      const cur = settings ?? (await commands.getSettings());
      await commands.setSettings({ ...cur, ...patch });
      onChanged();
    } catch (e) {
      pushToast("err", String(e));
    }
  }

  const toggleFav = (guid: string) => {
    const cur = settings?.favorite_schemes ?? [];
    const has = cur.some((g) => g.toLowerCase() === guid.toLowerCase());
    void patchSettings({
      favorite_schemes: has
        ? cur.filter((g) => g.toLowerCase() !== guid.toLowerCase())
        : [...cur, guid],
    });
  };

  const toggleExcluded = (guid: string) => {
    const cur = settings?.excluded_schemes ?? [];
    const has = cur.some((g) => g.toLowerCase() === guid.toLowerCase());
    void patchSettings({
      excluded_schemes: has
        ? cur.filter((g) => g.toLowerCase() !== guid.toLowerCase())
        : [...cur, guid],
    });
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
            placeholder="введите запрос…"
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
              className={`tile-scheme${s.active ? " active" : ""}${isSel ? " sel" : ""}`}
              onClick={() => onSelectExport?.(s.guid)}
              title="Клик — выбрать для экспорта"
            >
              <div className="ts-head">
                <button
                  className="icon-btn ts-act"
                  title={s.active ? "Уже активна" : "Сделать активной"}
                  disabled={s.active || !isAdmin || running}
                  onClick={(e) => {
                    e.stopPropagation();
                    void runAction("activate", s.guid);
                  }}
                >
                  <PowerIcon />
                </button>
                <div className="ts-title">
                  <div className="ts-name">{s.name || "Без названия"}</div>
                  <div className="ts-guid">{s.guid}</div>
                </div>
                {s.active ? <Badge kind="ok">АКТИВНА</Badge> : null}
              </div>
              <div className="ts-foot">
                <button
                  className={`icon-btn${isFav ? " on-fav" : ""}`}
                  title={isFav ? "Убрать из избранного" : "В избранное"}
                  onClick={(e) => {
                    e.stopPropagation();
                    toggleFav(s.guid);
                  }}
                >
                  {isFav ? <StarFilledIcon /> : <StarOutlineIcon />}
                </button>
                <button
                  className={`icon-btn${isEx ? " on-ex" : ""}`}
                  title={isEx ? "Включить в бенчмарк" : "Исключить из бенчмарка"}
                  onClick={(e) => {
                    e.stopPropagation();
                    toggleExcluded(s.guid);
                  }}
                >
                  <MinusCircleIcon />
                </button>
                <span className="ts-grow" />
                <button
                  className="icon-btn on-del"
                  title="Удалить схему"
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
          <div className="muted">Ничего не найдено.</div>
        </div>
      ) : null}
      <Modal
        open={confirm !== null}
        title={confirm?.kind === "restore" ? "Вернуть стандартные схемы" : "Удалить схему?"}
        onClose={() => setConfirm(null)}
        footer={
          confirm ? (
            <>
              <Button variant="ghost" onClick={() => setConfirm(null)}>
                Отмена
              </Button>
              <Button
                variant={confirm.kind === "delete" ? "danger" : "primary"}
                disabled={running}
                onClick={() => {
                  const c = confirm;
                  setConfirm(null);
                  if (c.kind === "delete") void doDelete(c.guid);
                  else void runAction("restore_defaults");
                }}
              >
                {confirm.kind === "restore" ? "Восстановить" : "Удалить"}
              </Button>
            </>
          ) : null
        }
      >
        {confirm?.kind === "restore" ? (
          <p className="hint">Восстановить стандартные схемы Windows? Активной станет системная по умолчанию.</p>
        ) : confirm ? (
          <p className="hint">
            Удалить схему{" "}
            <span className="strong">
              «{confirm.name}»
            </span>
            ? Действие необратимо.
          </p>
        ) : null}
      </Modal>
    </div>
  );
}
