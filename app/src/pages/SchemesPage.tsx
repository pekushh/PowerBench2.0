// Страница «Схемы»: управление схемами питания.

import { useEffect, useMemo, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { commands, type SchemeRow, type SettingsDto } from "../api";
import { Badge, Button, Modal } from "../components/ui";
import { ExportIcon, PlusIcon, RestoreIcon } from "../components/icons";
import SchemeTiles from "../components/SchemeTiles";
import { useCascade } from "../components/useCascade";
import { pushToast, useSession } from "../store";

export default function SchemesPage({ active = true }: { active?: boolean }) {
  const [schemes, setSchemes] = useState<SchemeRow[]>([]);
  const [settings, setSettings] = useState<SettingsDto | null>(null);
  // `null` — права ещё не проверены. Пока так, плашку не рисуем: иначе на
  // миллисекунду вспыхивало «требуются права администратора», а потом список
  // схем появлялся, и экран дёргался дважды.
  const [isAdmin, setIsAdmin] = useState<boolean | null>(null);
  const [exportTarget, setExportTarget] = useState<string | null>(null);
  const [askRestore, setAskRestore] = useState(false);
  const [busy, setBusy] = useState(false);
  const { running } = useSession();
  const rootRef = useCascade<HTMLDivElement>(active);
  const admin = isAdmin === true;

  const refresh = () => {
    commands.listSchemes().then(setSchemes).catch((e) => pushToast("err", String(e)));
    commands.getSettings().then(setSettings).catch(() => undefined);
    commands.isAdmin().then(setIsAdmin).catch(() => undefined);
  };

  useEffect(refresh, []);
  // После сессии список схем мог измениться (схема восстановлена, карантин
  // обновился) — перечитываем, чтобы страница не показывала устаревшее.
  useEffect(refresh, [running]);

  const exportSel = async () => {
    if (!exportTarget) return;
    const sch = schemes.find((s) => s.guid === exportTarget);
    try {
      const path = await save({
        title: "Экспорт схемы (.pow)",
        defaultPath: `${sch?.name ?? "scheme"}.pow`,
        filters: [{ name: "Схема питания Windows (.pow)", extensions: ["pow"] }],
      });
      if (!path) return;
      setBusy(true);
      await commands.schemeAction("export", exportTarget, path as string);
      pushToast("okk", "Схема экспортирована");
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setBusy(false);
    }
  };

  const duplicateSel = async () => {
    if (!exportTarget) return;
    try {
      setBusy(true);
      const res = await commands.schemeAction("duplicate", exportTarget, null);
      pushToast("okk", `Дубль: ${res}`);
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setBusy(false);
    }
  };

  const importScheme = async () => {
    try {
      const path = await open({
        multiple: false,
        title: "Импорт схемы (.pow)",
        filters: [{ name: "Схема питания Windows (.pow)", extensions: ["pow"] }],
      });
      if (!path) return;
      setBusy(true);
      const res = await commands.schemeAction("import", null, path as string);
      pushToast("okk", `Импортирована схема ${res}`);
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setBusy(false);
    }
  };

  const doRestore = async () => {
    try {
      setBusy(true);
      await commands.schemeAction("restore_defaults", null, null);
      pushToast("okk", "Стандартные схемы восстановлены");
      refresh();
    } catch (e) {
      pushToast("err", String(e));
    } finally {
      setBusy(false);
      setAskRestore(false);
    }
  };

  const selName = schemes.find((s) => s.guid === exportTarget)?.name ?? "";
  const dupCount = useMemo(() => {
    const names = new Map<string, number>();
    for (const s of schemes) {
      const key = (s.name || "").trim().toLowerCase();
      if (!key) continue;
      names.set(key, (names.get(key) ?? 0) + 1);
    }
    let extra = 0;
    for (const n of names.values()) if (n > 1) extra += n - 1;
    return extra;
  }, [schemes]);

  return (
      <div className="page schemes-page" ref={rootRef}>
      <div className="page-head">
        <h1>Схемы питания</h1>
        {/* Сводка в строке заголовка: отдельной строкой она отодвигала поиск
            на лишний отступ сверху. */}
        <span className="sub">
          <b>{schemes.length}</b>{" "}
          {schemes.length % 10 === 1 && schemes.length % 100 !== 11
            ? "схема"
            : schemes.length % 10 >= 2 &&
                schemes.length % 10 <= 4 &&
                !(schemes.length % 100 >= 12 && schemes.length % 100 <= 14)
              ? "схемы"
              : "схем"}{" "}
          · дубликатов по имени: <b>{dupCount}</b>
        </span>
        <div className="actions">
          <button
            type="button"
            className="act-primary"
            disabled={busy || !admin}
            title={admin ? "Импортировать схему питания из файла .pow" : "Требуются права администратора"}
            onClick={() => void importScheme()}
          >
            <PlusIcon />
            Импорт .pow
          </button>
          <button
            type="button"
            className="act-secondary"
            disabled={busy || !admin}
            title={admin ? "Восстановить стандартные схемы Windows" : "Требуются права администратора"}
            onClick={() => setAskRestore(true)}
          >
            <RestoreIcon />
            Вернуть стандартные схемы
          </button>
        </div>
      </div>
      {isAdmin === false ? (
        <div className="hint" style={{ marginBottom: 4 }}>
          {/* Без точки: это короткая метка-подсказка, а не предложение. */}
          <Badge kind="warn">Требуется запуск от имени администратора</Badge>
        </div>
      ) : null}
      {exportTarget ? (
        <div className="wizard-toolbar" style={{ marginBottom: 10 }}>
          <span className="ttl">Выбрана: {selName || exportTarget}</span>
          <div className="spacer" />
          <Button variant="ghost" disabled={busy} onClick={() => void exportSel()}>
            <ExportIcon width={15} height={15} />
            Экспорт схемы (.pow)
          </Button>
          <Button variant="ghost" disabled={busy} onClick={() => void duplicateSel()}>
            Дублировать
          </Button>
          <Button variant="ghost" onClick={() => setExportTarget(null)}>
            Отмена
          </Button>
        </div>
      ) : null}
      <SchemeTiles
        schemes={schemes}
        settings={settings}
        isAdmin={admin}
        // Во время замера переключение и удаление схем запрещены: смена схемы
        // посреди сессии портит результат. Раньше здесь стояло `false`, и
        // плитки оставались активными даже во время работающего бенчмарка.
        running={running}
        exportTarget={exportTarget}
        onSelectExport={setExportTarget}
        onChanged={refresh}
      />
      <Modal
        open={askRestore}
        title="Вернуть стандартные схемы"
        onClose={() => setAskRestore(false)}
        footer={
          <>
            <Button variant="ghost" onClick={() => setAskRestore(false)}>
              Отмена
            </Button>
            <Button variant="primary" disabled={busy} onClick={() => void doRestore()}>
              Восстановить
            </Button>
          </>
        }
      >
        <p className="hint">Восстановить стандартные схемы Windows? Активной станет системная по умолчанию.</p>
      </Modal>
    </div>
  );
}
