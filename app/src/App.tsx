// Корень приложения: безрамочное окно, иконка-сайдбар, страницы.

import { useCallback, useEffect, useState } from "react";
import { commands, onTestFinished, type SettingsDto } from "./api";
import { setRunning, useToasts } from "./store";
import TitleBar from "./components/TitleBar";
import { CubeIcon, GearIcon, HomeIcon, ListIcon, PowerIcon } from "./components/icons";
import BenchmarkPage from "./pages/BenchmarkPage";
import LogPage from "./pages/LogPage";
import ResultsPage from "./pages/ResultsPage";
import SchemesPage from "./pages/SchemesPage";
import SettingsPage from "./pages/SettingsPage";
import "./styles.css";

export type PageId = "test" | "schemes" | "results" | "log" | "settings";

const NAV: { id: PageId; label: string; icon: React.ReactNode }[] = [
  { id: "test", label: "Бенчмарк", icon: <HomeIcon /> },
  { id: "schemes", label: "Схемы", icon: <PowerIcon /> },
  { id: "results", label: "Результаты", icon: <CubeIcon /> },
  { id: "log", label: "Логи", icon: <ListIcon /> },
];

function applyAppearance(s: SettingsDto) {
  const root = document.documentElement;
  root.setAttribute("data-theme", s.theme);
  root.setAttribute("data-mode", s.mode);
  root.classList.toggle("reduce-motion", s.reduce_motion);
}

export default function App() {
  const [page, setPage] = useState<PageId>("test");
  const [collapsed, setCollapsed] = useState(false);
  const toasts = useToasts();

  useEffect(() => {
    commands
      .getSettings()
      .then((s) => {
        applyAppearance(s);
        setCollapsed(s.sidebar_collapsed);
      })
      .catch(() => undefined);
    commands.testRunning().then(setRunning).catch(() => undefined);
    const unfor = onTestFinished(() => setRunning(false));
    return () => {
      unfor.then((f) => f());
    };
  }, []);

  const toggleCollapse = useCallback(() => {
    setCollapsed((c) => {
      const next = !c;
      commands
        .getSettings()
        .then((s) => commands.setSettings({ ...s, sidebar_collapsed: next }).catch(() => undefined))
        .catch(() => undefined);
      return next;
    });
  }, []);

  const onAppearance = useCallback((s: SettingsDto) => applyAppearance(s), []);

  return (
    <div className="shell">
      <TitleBar collapsed={collapsed} onToggleCollapse={toggleCollapse} />
      <div className="body">
        <aside className={`sidebar fade-in ${collapsed ? "collapsed" : ""}`}>
          <nav className="sidebar-nav">
            {NAV.map((n) => (
              <div
                key={n.id}
                className={`nav-item ${page === n.id ? "active" : ""}`}
                onClick={() => setPage(n.id)}
                title={collapsed ? n.label : undefined}
              >
                <span className="nav-glyph">{n.icon}</span>
                <span className="nav-label">{n.label}</span>
              </div>
            ))}
          </nav>
          <div className="sidebar-foot">
            <div
              className={`nav-item ${page === "settings" ? "active" : ""}`}
              onClick={() => setPage("settings")}
              title={collapsed ? "Настройки" : undefined}
            >
              <span className="nav-glyph">
                <GearIcon />
              </span>
              <span className="nav-label">Настройки</span>
            </div>
          </div>
        </aside>
        <main className="main">
          <div className={`page-host ${page === "test" ? "on" : ""}`}>
            <BenchmarkPage />
          </div>
          <div className={`page-host ${page === "schemes" ? "on" : ""}`}>
            <SchemesPage />
          </div>
          <div className={`page-host ${page === "results" ? "on" : ""}`}>
            <ResultsPage active={page === "results"} />
          </div>
          <div className={`page-host ${page === "log" ? "on" : ""}`}>
            <LogPage />
          </div>
          <div className={`page-host ${page === "settings" ? "on" : ""}`}>
            <SettingsPage onAppearance={onAppearance} />
          </div>
        </main>
      </div>
      <div className="toasts">
        {toasts.map((t) => (
          <div key={t.id} className={`toast glass float ${t.kind}`}>
            {t.text}
          </div>
        ))}
      </div>
    </div>
  );
}
