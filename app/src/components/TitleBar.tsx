// Верхняя панель безрамочного окна: бренд, сворачивание меню, окно.

import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronIcon, CloseIcon, MinusIcon, MotionIcon } from "./icons";

export default function TitleBar({
  collapsed,
  onToggleCollapse,
  motionOff,
  onToggleMotion,
  section,
  sectionDetail,
  hardware,
  quietOn,
}: {
  collapsed: boolean;
  onToggleCollapse: () => void;
  /** Моторика выключена ручным выбором (гасится системный — отдельно). */
  motionOff: boolean;
  onToggleMotion: () => void;
  /** Название текущего раздела и уточнение к нему, например
   *  `Схемы питания · 111 схем`. */
  section?: string;
  sectionDetail?: string;
  /** Чип железа, например `Ryzen 5 7500F · 32 ГБ`. */
  hardware?: string;
  quietOn?: boolean;
}) {
  // Смена названия раздела: ключ по названию пересоздаёт узел, и кросс-фейд
  // (уход вверх, приход снизу) проигрывается снова. Без ключа текст
  // подменялся бы мгновенно.
  const [sectionShown, setSectionShown] = useState(section);
  useEffect(() => {
    setSectionShown(section);
  }, [section]);
  function win() {
    try {
      return getCurrentWindow();
    } catch {
      return null;
    }
  }
  return (
    <div
      className="titlebar"
      data-tauri-drag-region
      // Двойной клик по пустому месту шапки разворачивает окно: привычное
      // поведение безрамочного окна, кнопки при этом не затрагиваются.
      onDoubleClick={(e) => {
        if ((e.target as HTMLElement).closest("button")) return;
        void win()?.toggleMaximize().catch(() => undefined);
      }}
    >
      <div className="titlebar-brand" data-tauri-drag-region>
        <span className="mark" />
        <span>PowerBench</span>
      </div>
      {sectionShown ? (
        <span className="split" />
      ) : null}
      {sectionShown ? (
        <span
          className="titlebar-section is-in"
          key={sectionShown}
          data-tauri-drag-region
        >
          <span className="titlebar-section-name">{sectionShown}</span>
          {sectionDetail ? (
            <span className="titlebar-section-detail">{sectionDetail}</span>
          ) : null}
        </span>
      ) : null}
      <div className="titlebar-chips" data-tauri-drag-region>
        {quietOn ? (
          <span className="titlebar-chip on" title="Тихий режим включён">
            <span className="dot-live" />
            Тихий режим
          </span>
        ) : null}
        {hardware ? (
          <span className="titlebar-chip" title={hardware}>
            {hardware}
          </span>
        ) : null}
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
