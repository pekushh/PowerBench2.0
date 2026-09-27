// Страница «Схемы»: управление схемами питания.

import { useEffect, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { commands, type SchemeRow, type SettingsDto } from "../api";
import { Badge, Button, Modal } from "../components/ui";
import { ExportIcon } from "../components/icons";
import SchemeTiles from "../components/SchemeTiles";
import { pushToast } from "../store";

export default function SchemesPage() {
  const [schemes, setSchemes] = useState<SchemeRow[]>([]);
  const [settings, setSettings] = useState<SettingsDto | null>(null);
  const [isAdmin, setIsAdmin] = useState(false);
  const [exportTarget, setExportTarget] = useState<string | null>(null);
  const [askRestore, setAskRestore] = useState(false);
  const [busy, setBusy] = useState(false);

  const refresh = () => {
    commands.listSchemes().then(setSchemes).catch((e) => pushToast("err", String(e)));
    commands.getSettings().then(setSettings).catch(() => undefined);
    commands.isAdmin().then(setIsAdmin).catch(() => undefined);
  };

  useEffect(refresh, []);

  const exportSel = async () => {
    if (!exportTarget) return;
    const sch = schemes.find((s) => s.guid === exportTarget);
    try {
      const path = await save({
        title: "Экспорт схемы (.pow)",
        defaultPath: `${sch?.name ?? "scheme"}.pow`,
        filters: [{ name: "Power scheme", extensions: ["pow"] }],
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
        filters: [{ name: "Power scheme", extensions: ["pow"] }],
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

  return (
    <div className="page">
      <div className="page-head">
        <h1>Схемы питания</h1>
        <span className="sub">схем: {schemes.length}</span>
        <div className="actions">
          <Button variant="ghost" disabled={busy || !isAdmin} title={isAdmin ? undefined : "Требуются права администратора"} onClick={() => void importScheme()}>
            Импорт .pow
          </Button>
          <Button variant="ghost" disabled={busy || !isAdmin} title={isAdmin ? undefined : "Требуются права администратора"} onClick={() => setAskRestore(true)}>
            Вернуть стандартные схемы
          </Button>
        </div>
      </div>
      {!isAdmin ? (
        <div className="hint" style={{ marginBottom: 4 }}>
          <Badge kind="warn">Изменение схем питания доступно только при запуске от имени администратора.</Badge>
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
        isAdmin={isAdmin}
        running={false}
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
