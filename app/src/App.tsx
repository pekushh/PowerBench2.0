// Корень приложения: безрамочное окно, иконка-сайдбар, страницы.

import { useCallback, useEffect, useState } from "react";
import { commands, onTestFinished, type SettingsDto } from "./api";
import { setRunning, useSession, useToasts, pushToast } from "./store";
import { useScrollFade } from "./components/ui";
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

function resolveMode(mode: string): string {
  if (mode === "Auto") {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "Light" : "Dark";
  }
  return mode;
}

function applyAppearance(s: SettingsDto) {
  const root = document.documentElement;
  root.setAttribute("data-theme", s.theme);
  root.setAttribute("data-mode", resolveMode(s.mode));
  root.classList.toggle("reduce-motion", s.reduce_motion);
}

export default function App() {
  const [page, setPage] = useState<PageId>("test");
  const [collapsed, setCollapsed] = useState(false);
  const [appearance, setAppearance] = useState<SettingsDto | null>(null);
  const toasts = useToasts();
  const { ref: mainRef } = useScrollFade<HTMLElement>();
  // Подписка на «идёт ли сессия» — единственный источник для отключения
  // анимаций ниже.
  const { running: sessionRunning } = useSession();

  useEffect(() => {
    commands
      .getSettings()
      .then((s) => {
        setAppearance(s);
        setCollapsed(s.sidebar_collapsed);
      })
      .catch(() => undefined);
    commands.testRunning().then(setRunning).catch(() => undefined);
    const unfor = onTestFinished(() => setRunning(false));
    return () => {
      unfor.then((f) => f());
    };
  }, []);

  // Архитектурное отключение анимаций на время замера.
  //
  // Пока идёт бенчмарк, интерфейс не должен ничего рисовать «для красоты»:
  // каждая анимация и transition — это работа главного потока WebView, а она
  // конкурирует с нагрузкой ядра, которую мы как раз измеряем. Класс на `<html>`
  // отключает всё разом, включая анимации, добавленные в будущем.
  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle("bench-running", sessionRunning);
    return () => root.classList.remove("bench-running");
  }, [sessionRunning]);

  useEffect(() => {
    if (!appearance) return;
    // Применяем новую тему сразу, анимацию настраиваем следом: если сначала
    // спросить `reduce-motion` у старого класса, решение всегда принималось бы
    // по предыдущему значению — включение анимировало, выключение нет.
    const d = document as Document & {
      startViewTransition?: (cb: () => void) => void;
    };
    const animate =
      !appearance.reduce_motion &&
      !sessionRunning &&
      typeof d.startViewTransition === "function";
    if (animate) {
      try {
        d.startViewTransition?.(() => applyAppearance(appearance));
      } catch {
        applyAppearance(appearance);
      }
    } else {
      applyAppearance(appearance);
    }
    if (appearance.mode !== "Auto") return;
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = () => appearance && applyAppearance(appearance);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [appearance, sessionRunning]);

  const toggleCollapse = useCallback(() => {
    // Не пишем IPC внутри апдейтера состояния: React вызывает его дважды
    // (StrictMode) и может вызвать во время чужого обновления. Прочитанное
    // значение настроек забираем явно, а запись делаем в отдельном потоке.
    const next = !collapsed;
    setCollapsed(next);
    commands
      .getSettings()
      .then((s) => commands.setSettings({ ...s, sidebar_collapsed: next }))
      .catch((e) => {
        pushToast("err", `Не удалось сохранить состояние панели: ${String(e)}`);
        setCollapsed(!next);
      });
  }, [collapsed]);

  const onAppearance = useCallback((s: SettingsDto) => setAppearance(s), []);

  return (
    <div className="shell">
      <TitleBar collapsed={collapsed} onToggleCollapse={toggleCollapse} />
      <div className="body">
        <aside className={`sidebar fade-in ${collapsed ? "collapsed" : ""}`}>
          <nav className="sidebar-nav">
            {NAV.map((n) => (
              <button
                key={n.id}
                type="button"
                className={`nav-item ${page === n.id ? "active" : ""}`}
                onClick={() => setPage(n.id)}
                // Навигация была на `<div onClick>`: с клавиатуры и со
                // скринридера до неё было не добраться вовсе.
                aria-current={page === n.id ? "page" : undefined}
                title={collapsed ? n.label : undefined}
              >
                <span className="nav-glyph">{n.icon}</span>
                <span className="nav-label">{n.label}</span>
              </button>
            ))}
          </nav>
          <div className="sidebar-foot">
            <button
              type="button"
              className={`nav-item ${page === "settings" ? "active" : ""}`}
              onClick={() => setPage("settings")}
              aria-current={page === "settings" ? "page" : undefined}
              title={collapsed ? "Настройки" : undefined}
            >
              <span className="nav-glyph">
                <GearIcon />
              </span>
              <span className="nav-label">Настройки</span>
            </button>
          </div>
        </aside>
        <main ref={mainRef} className="main scrolled-x">
          {/* BenchmarkPage всегда смонтирован: мастер хранит состояние сессии,
              и размонтирование теряло бы его при переходе на другие вкладки.
              Но показывается он ТОЛЬКО когда активен — иначе он накладывался
              на страницу «Схемы» (визуально две страницы в одном экране).
              Остальные страницы монтируются по требованию: раньше все пять
              висели в DOM, скрытые `display:none`, и каждая держала свои
              IPC-вызовы, подписки и таймеры всё время работы приложения. */}
          <div className={`page-host${page === "test" ? " on" : ""}`}>
            <BenchmarkPage />
          </div>
          {page === "schemes" ? (
            <div className="page-host on">
              <SchemesPage />
            </div>
          ) : null}
          {page === "results" ? (
            <div className="page-host on">
              <ResultsPage />
            </div>
          ) : null}
          {page === "log" ? (
            <div className="page-host on">
              <LogPage />
            </div>
          ) : null}
          {page === "settings" ? (
            <div className="page-host on">
              <SettingsPage onAppearance={onAppearance} />
            </div>
          ) : null}
        </main>
      </div>
      <div className="toasts" role="status" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast glass float ${t.kind}`}>
            {t.text}
          </div>
        ))}
      </div>
    </div>
  );
}
