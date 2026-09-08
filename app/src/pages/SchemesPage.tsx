// Страница «Схемы питания»: поиск, фильтр «Все / Избранные / Исключённые»,
// управление схемами (активация, дублирование, удаление, импорт .pow,
// восстановление стандартных) с подтверждениями.

import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { commands, type SchemeRow, type SettingsDto } from "../api";
import { Badge, Button, Field, Glass, Seg } from "../components/ui";
import { pushToast } from "../store";

type Filter = "all" | "fav" | "excluded";

function confirm(msg: string): boolean {
  return window.confirm(msg);
}

export default function SchemesPage() {
  const [schemes, setSchemes] = useState<SchemeRow[]>([]);
  const [settings, setSettings] = useState<SettingsDto | null>(null);
  const [adm, setAdm] = useState<boolean | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [busyAct, setBusyAct] = useState<string | null>(null);

  const refresh = () => {
    commands.listSchemes().then(setSchemes).catch((e) => pushToast("err", String(e)));
    commands.getSettings().then(setSettings).catch(() => undefined);
    commands.isAdmin().then(setAdm).catch(() => undefined);
  };

  useEffect(() => {
    refresh();
    const id = window.setInterval(refresh, 5000);
    return () => window.clearInterval(id);
  }, []);

  const favorites = settings?.favorite_schemes ?? [];
  const excluded = settings?.excluded_schemes ?? [];

  const isFav = (g: string) => favorites.some((x) => x.toLowerCase() === g.toLowerCase());
  const isExcl = (g: string) => excluded.some((x) => x.toLowerCase() === g.toLowerCase());

  const saveSettings = (next: SettingsDto) => {
    commands
      .setSettings(next)
      .then(() => setSettings(next))
      .catch((e) => pushToast("err", String(e)));
  };

  const toggleFav = (g: string) => {
    if (!settings) return;
    const list = isFav(g)
      ? favorites.filter((x) => x.toLowerCase() !== g.toLowerCase())
      : [...favorites, g];
    saveSettings({ ...settings, favorite_schemes: list });
  };

  const toggleExcl = (g: string) => {
    if (!settings) return;
    const list = isExcl(g)
      ? excluded.filter((x) => x.toLowerCase() !== g.toLowerCase())
      : [...excluded, g];
    saveSettings({ ...settings, excluded_schemes: list });
  };

  const act = async (action: string, guid?: string) => {
    setBusyAct(action + (guid ?? ""));
    try {
      const result = await commands.schemeAction(action, guid ?? null, null);
      pushToast("okk", action === "activate" ? "Схема активирована" : action === "duplicate" ? `Дубль: ${result}` : "Готово");
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setBusyAct(null);
    }
  };

  const doDelete = async (guid: string, name: string) => {
    if (!confirm(`Удалить схему «${name}»? Действие необратимо.`)) return;
    setBusyAct("delete" + guid);
    try {
      await commands.schemeAction("delete", guid, null);
      pushToast("okk", "Схема удалена");
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setBusyAct(null);
    }
  };

  const doImport = async () => {
    const path = await open({
      multiple: false,
      title: "Импорт схемы (.pow)",
      filters: [{ name: "Power scheme", extensions: ["pow"] }],
    });
    if (!path) return;
    setBusyAct("import");
    try {
      const guid = await commands.schemeAction("import", null, path);
      pushToast("okk", `Импортирована схема ${guid}`);
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setBusyAct(null);
    }
  };

  const doRestoreDefaults = async () => {
    if (!confirm("Восстановить стандартные схемы Windows? Активной станет системная по умолчанию.")) return;
    setBusyAct("restore");
    try {
      await commands.schemeAction("restore_defaults");
      pushToast("okk", "Стандартные схемы восстановлены");
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setBusyAct(null);
    }
  };

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return schemes.filter((s) => {
      if (filter === "fav" && !isFav(s.guid)) return false;
      if (filter === "excluded" && !isExcl(s.guid)) return false;
      if (!q) return true;
      return s.name.toLowerCase().includes(q) || s.guid.toLowerCase().includes(q);
    });
  }, [schemes, query, filter, favorites, excluded]);

  return (
    <div className="page">
      <div className="page-head">
        <h1>Схемы питания</h1>
        <div className="grow" />
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

      {adm === false ? (
        <Glass className="inset">
          <Badge kind="danger">Требуются права администратора</Badge> Изменение схем питания доступно
          только при запуске от имени администратора.
        </Glass>
      ) : null}

      <div className="row wrap">
        <div className="grow" style={{ minWidth: 220 }}>
          <Field label="Поиск по имени или GUID">
            <input
              id="schemes-search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="введите запрос…"
            />
          </Field>
        </div>
        <div className="row">
          <Button onClick={doImport} disabled={adm === false || busyAct !== null}>
            Импорт .pow
          </Button>
          <Button onClick={doRestoreDefaults} disabled={adm === false || busyAct !== null}>
            Вернуть стандартные
          </Button>
          <Button onClick={refresh}>Обновить (F5)</Button>
        </div>
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        {visible.map((s) => {
          const fav = isFav(s.guid);
          const ex = isExcl(s.guid);
          return (
            <Glass key={s.guid} className="scheme-item">
              <span className="icon-btn active-marker" title={s.active ? "Активная схема" : ""}>
                {s.active ? "●" : "○"}
              </span>
              <div className="grow">
                <div className="name">
                  {s.name} {fav ? <Badge kind="warn">★ избранная</Badge> : null} {ex ? <Badge kind="plain">исключена из теста</Badge> : null}
                </div>
                <div className="guid">{s.guid}</div>
              </div>
              <button className={`icon-btn fav${fav ? " on" : ""}`} title="Избранное" onClick={() => toggleFav(s.guid)}>
                {fav ? "★" : "☆"}
              </button>
              <button className="icon-btn" title="Исключить из теста" onClick={() => toggleExcl(s.guid)}>
                {ex ? "👁" : "🚫"}
              </button>
              <div className="row">
                <Button
                  disabled={s.active || adm === false || busyAct !== null}
                  onClick={() => act("activate", s.guid)}
                >
                  Активировать
                </Button>
                <Button disabled={adm === false || busyAct !== null} onClick={() => act("duplicate", s.guid)}>
                  Дублировать
                </Button>
                <Button variant="danger" disabled={adm === false || busyAct !== null} onClick={() => doDelete(s.guid, s.name)}>
                  Удалить
                </Button>
              </div>
            </Glass>
          );
        })}
        {visible.length === 0 ? (
          <Glass className="inset">
            <div className="sub" style={{ color: "var(--text-3)" }}>Ничего не найдено.</div>
          </Glass>
        ) : null}
      </div>
    </div>
  );
}