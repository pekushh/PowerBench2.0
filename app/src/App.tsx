// Корень приложения: тема из настроек, навигация (sidebar/пилюля <780px),
// хоткеи Ctrl+1..5, Ctrl+F, Ctrl+Enter, F5, Esc.

import { useEffect, useState } from "react";
import { commands, onTestFinished, type SettingsDto } from "./api";
import { Badge } from "./components/ui";
import { pushToast, setLastResult, useSession, useToasts } from "./store";
import LogPage from "./pages/LogPage";
import ResultsPage from "./pages/ResultsPage";
import SchemesPage from "./pages/SchemesPage";
import SettingsPage from "./pages/SettingsPage";
import TestPage from "./pages/TestPage";
import "./styles.css";

type PageId = "test" | "results" | "schemes" | "log" | "settings";

const ROUTES: { id: PageId; label: string; kbd: string; icon: string }[] = [
  { id: "test", label: "Тестирование", kbd: "Ctrl+1", icon: "▶" },
  { id: "results", label: "Результаты", kbd: "Ctrl+2", icon: "▤" },
  { id: "schemes", label: "Схемы питания", kbd: "Ctrl+3", icon: "⌁" },
  { id: "log", label: "Журнал", kbd: "Ctrl+4", icon: "≡" },
  { id: "settings", label: "Настройки", kbd: "Ctrl+5", icon: "⚙" },
];

function applyAppearance(s: SettingsDto) {
  const root = document.documentElement;
  root.setAttribute("data-theme", s.theme);
  root.setAttribute("data-mode", s.mode);
  root.classList.toggle("reduce-motion", s.reduce_motion);
}

export default function App() {
  const [page, setPage] = useState<PageId>("test");
  const [appearance, setAppearance] = useState<SettingsDto | null>(null);
  const [adm, setAdm] = useState<boolean | null>(null);
  const [ac, setAc] = useState<boolean | null>(null);
  const session = useSession();
  const toasts = useToasts();

  useEffect(() => {
    let alive = true;
    commands
      .getSettings()
      .then((s) => {
        if (!alive) return;
        setAppearance(s);
        setAppearanceForRender(s);
      })
      .catch(() => undefined);
    commands.isAdmin().then((v) => alive && setAdm(v)).catch(() => undefined);
    commands.acPowerOnline().then((v) => alive && setAc(v)).catch(() => undefined);
    const unfor = onTestFinished((m) => {
      setLastResult(m);
      setAcNow();
    });
    return () => {
      alive = false;
      unfor.then((f) => f());
    };
  }, []);

  function setAppearanceForRender(s: SettingsDto) {
    applyAppearance(s);
  }

  function setAcNow() {
    commands.acPowerOnline().then(setAc).catch(() => undefined);
  }

  useEffect(() => {
    if (page === "schemes") {
      // Ctrl+F уже нажался на этой странице — переключение фокуса поиска.
      const el = document.getElementById("schemes-search");
      if (el) el.focus();
    }
  }, [page]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const ctrl = e.ctrlKey || e.metaKey;
      if (ctrl && e.key >= "1" && e.key <= "5") {
        const r = ROUTES.find((r) => r.kbd.endsWith(e.key));
        if (r) {
          e.preventDefault();
          setPage(r.id);
        }
        return;
      }
      if (ctrl && (e.key === "f" || e.key === "F" || e.key === "а" || e.key === "А")) {
        e.preventDefault();
        setPage("schemes");
        requestAnimationFrame(() => {
          const el = document.getElementById("schemes-search");
          if (el) el.focus();
        });
        return;
      }
      if (ctrl && (e.key === "Enter")) {
        e.preventDefault();
        const el = document.getElementById("test-start");
        if (el) (el as HTMLButtonElement).click();
        return;
      }
      if (e.key === "F5") {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("pb-refresh"));
        return;
      }
      if (e.key === "Escape" && session.running) {
        e.preventDefault();
        commands.stopTest().then((was) => {
          if (was) pushToast("info", "запрос остановки сессии");
        }).catch((err) => pushToast("err", String(err)));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [page, session.running]);

  return (
    <div className="shell">
      <div className="ambient" />
      <aside className="sidebar fade-in">
        <div className="brand">
          <span className="mark" />
          <span>PowerBench</span>
        </div>
        {ROUTES.map((r) => (
          <div
            key={r.id}
            className={`nav-item ${page === r.id ? "active" : ""}`}
            onClick={() => setPage(r.id)}
          >
            <span>{r.icon}</span>
            <span>{r.label}</span>
            <span className="kbd">{r.kbd}</span>
          </div>
        ))}
        <div className="nav-legend">
          <div>
            {adm === null ? "…" : adm ? <Badge kind="ok">администратор</Badge> : <Badge kind="danger">не админ</Badge>}
          </div>
          <div>
            {ac === null ? "…" : ac ? <Badge kind="ok">сеть</Badge> : <Badge kind="warn">батарея</Badge>}
          </div>
          {appearance ? (
            <div>
              тема {appearance.theme} · {appearance.mode.toLowerCase()}
            </div>
          ) : null}
        </div>
      </aside>

      <main className="main">
        {page === "test" ? <TestPage /> : null}
        {page === "results" ? <ResultsPage /> : null}
        {page === "schemes" ? <SchemesPage /> : null}
        {page === "log" ? <LogPage /> : null}
        {page === "settings" ? <SettingsPage onAppearance={setAppearanceForRender} /> : null}
      </main>

      <div className="pill float" role="navigation" aria-label="Навигация">
        {ROUTES.map((r) => (
          <button key={r.id} className={page === r.id ? "on" : ""} onClick={() => setPage(r.id)} title={`${r.label} (${r.kbd})`}>
            {r.icon}
          </button>
        ))}
      </div>

      {toasts.map((t) => (
        <div key={t.id} className={`toast float ${t.kind}`}>
          {t.text}
        </div>
      ))}
    </div>
  );
}