// Корень приложения: безрамочное окно, иконка-сайдбар, страницы.

import { useCallback, useEffect, useRef, useState } from "react";
import { commands, onTestFinished, type SettingsDto } from "./api";
import { setRunning, useSession, useToasts, pushToast } from "./store";
import { useScrollFade } from "./components/ui";
import { useNavIndicator } from "./components/useNavIndicator";
import { useRipple } from "./components/useRipple";
import TitleBar from "./components/TitleBar";
import { BarsIcon, GaugeIcon, GearIcon, LogLinesIcon, PlugIcon } from "./components/icons";
import BenchmarkPage from "./pages/BenchmarkPage";
import LogPage from "./pages/LogPage";
import ResultsPage from "./pages/ResultsPage";
import SchemesPage from "./pages/SchemesPage";
import SettingsPage from "./pages/SettingsPage";
import { applyMotion, applyDensity } from "./motion";
import { toggleFullscreen } from "./windowState";
import "./styles.css";

export type PageId = "test" | "schemes" | "results" | "log" | "settings";

const NAV: { id: PageId; label: string; icon: React.ReactNode }[] = [
  { id: "test", label: "Бенчмарк", icon: <GaugeIcon /> },
  { id: "schemes", label: "Схемы питания", icon: <PlugIcon /> },
  { id: "results", label: "Результаты", icon: <BarsIcon /> },
  { id: "log", label: "Логи", icon: <LogLinesIcon /> },
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
  // Один выключатель моторики на всё приложение: он же гасит анимации по
  // системному правилу `html[data-motion="off"]`.
  applyMotion(s.reduce_motion);
  // Плотность и масштаб текста — два независимых множителя в CSS (`--k` и
  // `--kt`); интерфейс только ставит атрибуты.
  applyDensity(s.density, s.text_scale);
}

export default function App() {
  const [page, setPage] = useState<PageId>("test");
  // Что на самом деле нарисовано: вкладка меняется с задержкой в 150 мс,
  // чтобы уходящая успела уехать вниз и погаснуть. Без задержки размонтирование
  // мгновенное и уход не виден вовсе.
  const [shown, setShown] = useState<PageId>(page);
  const [leaving, setLeaving] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  const [appearance, setAppearance] = useState<SettingsDto | null>(null);
  const toasts = useToasts();
  const { ref: mainRef, top: mainTop, bottom: mainBottom } = useScrollFade<HTMLElement>();
  // Один индикатор активного пункта на всё меню, внутри сайдбара.
  const sidebarRef = useRef<HTMLElement>(null);
  useNavIndicator(page, collapsed, sidebarRef);
  useRipple();
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
      unfor.then((f) => f()).catch(() => undefined);
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

  // F11 — полный экран: на время замера и для показа. При выходе из полного
  // экрана окно возвращается к прежнему размеру само.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "F11" || e.repeat) return;
      // В поле ввода F11 всё равно не должен перехватываться: проверяем, что
      // пользователь не правит текст.
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      e.preventDefault();
      void toggleFullscreen();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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

  // Смена вкладки: область возвращается к началу раздела плавно, но при
  // уменьшенном движении — мгновенно, иначе «плавно» означало бы задержку на
  // целое поведение.
  useEffect(() => {
    const el = mainRef.current;
    if (!el) return;
    el.scrollTo({
      top: 0,
      behavior: appearance?.reduce_motion ? "auto" : "smooth",
    });
  }, [page, appearance?.reduce_motion, mainRef]);

  // Уход вкладки: 150 мс на анимацию, затем размонтирование.
  useEffect(() => {
    if (page === shown) return;
    setLeaving(true);
    const id = window.setTimeout(() => {
      setShown(page);
      setLeaving(false);
    }, 150);
    return () => window.clearTimeout(id);
  }, [page, shown]);

  // Шапка экрана: та же схема, что и у каскадов, — класс снимается и
  // возвращается через кадр, иначе анимация страницы играет один раз при
  // монтировании: «Бенчмарк» не размонтируется при переходе на другие
  // вкладки, и возвращаться на него приходилось бы в «мёртвый» экран.
  useEffect(() => {
    const host = document.querySelector<HTMLElement>(".page-host.on");
    if (!host) return;
    host.classList.remove("pb-host-in");
    const raf = requestAnimationFrame(() => host.classList.add("pb-host-in"));
    return () => cancelAnimationFrame(raf);
  }, [shown]);

  const toggleCollapse = useCallback(() => {
    // Не пишем IPC внутри апдейтера состояния: React вызывает его дважды
    // (StrictMode) и может вызвать во время чужого обновления. Прочитанное
    // значение настроек забираем явно, а запись делаем в отдельном потоке.
    //
    // Подтверждение тостом убрано: плашка «Меню свёрнуто/развёрнуто»
    // появлялась в правом нижнем углу поверх всего и дублировала состояние,
    // которое и так видно по меню. Состояние и раньше переживало перезапуск —
    // оно в `sidebar_collapsed`.
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

  // В шапке — состояние, а не название раздела: название уже написано
  // крупно на странице. Уточнение публикует активная страница через стор,
  // чтобы шапка не делала собственный запрос и не показывала другое число.
  const { sectionDetail } = useSession();
  const [hwChip, setHwChip] = useState<string | undefined>(undefined);
  const [hwFull, setHwFull] = useState<string | undefined>(undefined);
  useEffect(() => {
    let alive = true;
    commands
      .identityInfo()
      .then((id) => {
        if (!alive) return;
        const full = (id.cpu_brand || id.cpu_identifier || "").trim();
        if (!full) return;
        const mem = id.memory_gib > 0 ? ` · ${id.memory_gib.toFixed(0)} ГБ` : "";
        setHwFull(`${full}${mem}`);
        // Короткое имя без хвостов вида «6-Core Processor»: в чипе режется
        // многоточием, а полное остаётся в подсказке.
        const short = full
          .replace(/\b(AMD|Intel)\b/gi, "")
          .replace(/\s*\(?\s*(with\s+)?Radeon.*$/i, "")
          .replace(/\s*-?\s*\d+-Core\s+Processor\s*$/i, "")
          .replace(/\s*\(.*?\)\s*$/, "")
          .replace(/\s+/g, " ")
          .trim();
        setHwChip(`${short.slice(0, 30)}${short.length > 30 ? "…" : ""}${mem}`);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  // Выключатель моторики в топбаре удалён вместе с кнопкой: значение пишется
  // те же настройки, что и тумблер в «Настройках», и два места с одним
  // переключателем расходились при рассинхронизации IPC.
  const onAppearance = useCallback((s: SettingsDto) => setAppearance(s), []);

  return (
    <div className={`shell ${collapsed ? "rail-collapsed" : ""}`}>
      <TitleBar
        collapsed={collapsed}
        onToggleCollapse={toggleCollapse}
        status={sectionDetail}
        hardware={hwChip}
        hardwareFull={hwFull}
      />
      <div className="body">
        <aside ref={sidebarRef} className={`sidebar fade-in ${collapsed ? "collapsed" : ""}`}>
          <nav className="sidebar-nav">
            {NAV.map((n) => (
              <button
                key={n.id}
                type="button"
                className={`nav-item ${page === n.id ? "active" : ""}`}
                data-value={n.id}
                // Подпись в свёрнутом виде схлопнута, и пункт становится
                // безымянным — имя переносится в подсказку на CSS.
                data-tip={n.label}
                onClick={() => setPage(n.id)}
                // Навигация была на `<div onClick>`: с клавиатуры и со
                // скринридера до неё было не добраться вовсе.
                aria-current={page === n.id ? "page" : undefined}
                title={collapsed ? undefined : n.label}
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
              data-value="settings"
              data-tip="Настройки"
              onClick={() => setPage("settings")}
              aria-current={page === "settings" ? "page" : undefined}
              title={collapsed ? undefined : "Настройки"}
            >
              <span className="nav-glyph">
                <GearIcon />
              </span>
              <span className="nav-label">Настройки</span>
            </button>
          </div>
        </aside>
        <main ref={mainRef} className={`main${mainTop ? " fade-top" : ""}${mainBottom ? " fade-bottom" : ""}`}>
          {/* BenchmarkPage всегда смонтирован: мастер хранит состояние сессии,
              и размонтирование теряло бы его при переходе на другие вкладки.
              Но показывается он ТОЛЬКО когда активен — иначе он накладывался
              на страницу «Схемы» (визуально две страницы в одном экране).
              Остальные страницы монтируются по требованию: раньше все пять
              висели в DOM, скрытые `display:none`, и каждая держала свои
              IPC-вызовы, подписки и таймеры всё время работы приложения. */}
          <div
            className={`page-host${shown === "test" ? " on" : ""}${
              leaving ? " leaving" : ""
            }`}
          >
            <BenchmarkPage active={shown === "test" && !leaving} />
          </div>
          {shown === "schemes" ? (
            <div className={`page-host on${leaving ? " leaving" : ""}`}>
              <SchemesPage active={!leaving} />
            </div>
          ) : null}
          {shown === "results" ? (
            <div className={`page-host on${leaving ? " leaving" : ""}`}>
              <ResultsPage active={!leaving} />
            </div>
          ) : null}
          {shown === "log" ? (
            <div className={`page-host on${leaving ? " leaving" : ""}`}>
              <LogPage active={!leaving} />
            </div>
          ) : null}
          {shown === "settings" ? (
            <div className={`page-host on${leaving ? " leaving" : ""}`}>
              <SettingsPage onAppearance={onAppearance} active={!leaving} />
            </div>
          ) : null}
        </main>
      </div>
      <div className="toasts" role="status" aria-live="polite">
        {/* Во время замера тосты схлопываются в один (ТЗ XIII.5): стопка
            сообщений поверх идущего замера и запись в журнал сама по себе
            нагружает интерфейс. Показываем последний — самый свежий. */}
        {(sessionRunning ? toasts.slice(-1) : toasts).map((t) => (
          <div key={t.id} className={`toast glass float ${t.kind}`}>
            {t.text}
          </div>
        ))}
      </div>
    </div>
  );
}
