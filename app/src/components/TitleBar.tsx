// Верхняя панель безрамочного окна: бренд, сворачивание меню, окно.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronIcon, CloseIcon, MinusIcon } from "./icons";

export default function TitleBar({
  collapsed,
  onToggleCollapse,
}: {
  collapsed: boolean;
  onToggleCollapse: () => void;
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
