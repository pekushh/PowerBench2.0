// Верхняя панель безрамочного окна: бренд, сворачивание меню, окно.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronIcon, CloseIcon, MinusIcon, MotionIcon } from "./icons";

export default function TitleBar({
  collapsed,
  onToggleCollapse,
  motionOff,
  onToggleMotion,
}: {
  collapsed: boolean;
  onToggleCollapse: () => void;
  /** Моторика выключена ручным выбором (гасится системный — отдельно). */
  motionOff: boolean;
  onToggleMotion: () => void;
}) {
  function win() {
    try {
      return getCurrentWindow();
    } catch {
      return null;
    }
  }
  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar-brand" data-tauri-drag-region>
        <span className="mark" />
        <span>PowerBench</span>
      </div>
      <div className="titlebar-controls">
        <button
          type="button"
          className={`titlebar-btn motion${motionOff ? " off" : ""}`}
          title={motionOff ? "Включить анимации" : "Отключить анимации"}
          aria-label={motionOff ? "Включить анимации" : "Отключить анимации"}
          aria-pressed={motionOff}
          onClick={onToggleMotion}
        >
          <MotionIcon />
        </button>
        <span className="split" />
        <button
          type="button"
          className="titlebar-btn narrow"
          title={collapsed ? "Развернуть меню" : "Свернуть меню"}
          aria-label={collapsed ? "Развернуть меню" : "Свернуть меню"}
          onClick={onToggleCollapse}
        >
          <ChevronIcon flip={collapsed} />
        </button>
        <span className="split" />
        <button
          type="button"
          className="titlebar-btn"
          title="Свернуть"
          aria-label="Свернуть"
          onClick={() => win()?.minimize().catch(() => undefined)}
        >
          <MinusIcon />
        </button>
        <button
          type="button"
          className="titlebar-btn close"
          title="Закрыть"
          aria-label="Закрыть"
          onClick={() => win()?.close().catch(() => undefined)}
        >
          <CloseIcon />
        </button>
      </div>
    </div>
  );
}
