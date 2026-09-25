// Плитки схем питания: поиск, фильтры, избранное, исключения,
// активация, удаление, экспорт, дублирование.

import { useMemo, useState } from "react";
import { commands, type SchemeRow, type SettingsDto } from "../api";
import { Badge, Button, Modal, Seg } from "./ui";
import {
  MinusCircleIcon,
  PowerIcon,
  SearchIcon,
  StarFilledIcon,
  StarOutlineIcon,
  TrashIcon,
} from "./icons";
import { pushToast } from "../store";

type Filter = "all" | "fav" | "excluded";

type Confirm = { kind: "delete"; guid: string; name: string } | { kind: "restore" } | null;

/** Простые карточки выбора для визарда (scheme-grid). */
export function SchemeCards({
  schemes,
  selected,
  excluded,
  onToggle,
}: {
  schemes: SchemeRow[];
  selected: Set<string>;
  excluded: Set<string>;
  onToggle: (guid: string, active: boolean) => void;
}) {
  return (
    <div className="scheme-grid">
      {schemes.map((s) => {
        const on = selected.has(s.guid);
        const isEx = excluded.has(s.guid.toLowerCase());
        return (
          <div
            key={s.guid}
            className={`scheme-card ${on ? "on" : ""}`}
            role="button"
            tabIndex={0}
            onClick={() => onToggle(s.guid, s.active)}
            onKeyDown={(e) => e.key === "Enter" && onToggle(s.guid, s.active)}
          >
            <div className="scheme-name">{s.name || "Без названия"}</div>
            <div className="scheme-meta">
              {s.active ? <Badge kind="ok">АКТИВНА</Badge> : null}
              {isEx ? <Badge kind="plain">исключена</Badge> : null}
            </div>
            <div className="scheme-id" title="ID схемы">
              {s.guid}
            </div>
            <div className="check">
              <span className="tick">{on ? "✓" : ""}</span>
              {on ? "Выбрано" : "Выбрать"}
            </div>
          </div>
        );
      })}
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
