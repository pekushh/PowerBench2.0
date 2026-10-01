// Верхняя панель безрамочного окна: бренд, сворачивание меню, окно.

import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CrossIcon, MenuIcon, MinusIcon, MotionIcon, SquaresIcon, SquareIcon } from "./icons";

export default function TitleBar({
  collapsed,
  onToggleCollapse,
  motionOff,
  onToggleMotion,
  status,
  hardware,
  hardwareFull,
}: {
  collapsed: boolean;
  onToggleCollapse: () => void;
  /** Моторика выключена ручным выбором (гасится системный — отдельно). */
  motionOff: boolean;
  onToggleMotion: () => void;
  /** Состояние приложения для крошки: «Идёт замер · раунд 2», «111 схем»,
   *  «5000 записей». Название раздела в шапке не дублируется — оно уже
   *  написано крупно под ней. */
  status?: string;
  /** Короткое имя железа для чипа: `Ryzen 5 7500F · 32 ГБ`. */
  hardware?: string;
  /** Полное название для подсказки. */
  hardwareFull?: string;
}) {
  // Смена состояния: ключ по тексту пересоздаёт узел, и кросс-фейд
  // (уход вверх, приход снизу) проигрывается снова. Без ключа текст
  // подменялся бы мгновенно.
  const [statusShown, setStatusShown] = useState(status);
  useEffect(() => {
    setStatusShown(status);
  }, [status]);

  // Состояние окна нужно для иконки разворачивания: у развёрнутого окна она
  // должна быть «два квадрата», иначе кнопка врёт о том, что сделает.
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    const w = win();
    if (!w) return;
    const sync = () => w.isMaximized().then(setMaximized).catch(() => undefined);
    void sync();
    const un = w.onResized(() => void sync());
    return () => {
      un.then((f) => f()).catch(() => undefined);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const toggleMaximize = async () => {
    const w = win();
    if (!w) return;
    try {
      await w.toggleMaximize();
      setMaximized(await w.isMaximized());
    } catch {
      // Окно может быть недоступно (например, при выходе): кнопка просто
      // ничего не делает, ошибку в тост не выводим.
    }
  };
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
        void toggleMaximize();
      }}
    >
      <div className="titlebar-brand" data-tauri-drag-region>
        {/* Переключатель меню — слева, рядом со знаком: так он стоит в
            VS Code и Figma, и на него смотрят по умолчанию. */}
        <button
          type="button"
          className={`titlebar-btn menu${collapsed ? " off" : ""}`}
          title={collapsed ? "Развернуть меню (Ctrl+B)" : "Свернуть меню (Ctrl+B)"}
          aria-label={collapsed ? "Развернуть меню" : "Свернуть меню"}
          aria-pressed={collapsed}
          onClick={onToggleCollapse}
        >
          <MenuIcon />
        </button>
        {/* Знак приложения: плитка с молнией вместо серого кружка, который
            ничего не сообщал. */}
        <span className="mark">
          <MotionIcon width={12} height={12} />
        </span>
        <span>PowerBench</span>
      </div>
      {statusShown ? (
        <span className="split" />
      ) : null}
      {/* В шапке — состояние, а не название раздела: название уже написано
          крупно под шапкой, а полезно то, чего на экране нет. */}
      {statusShown ? (
        <span
          className="titlebar-section is-in"
          key={statusShown}
          data-tauri-drag-region
        >
          {statusShown}
        </span>
      ) : null}
      <div className="titlebar-chips" data-tauri-drag-region>
        {/* Чип железа — самый длинный, поэтому у него `data-prio="1"`:
            при узком окне он прячется первым. */}
        {hardware ? (
          <span className="titlebar-chip trunc" data-prio="1" title={hardwareFull ?? hardware}>
            {hardware}
          </span>
        ) : null}
      </div>
      <div className="titlebar-controls">
        {/* Кнопка с подписью: одна иконка без подписи читалась как
            «питание / энергосбережение». */}
        <button
          type="button"
          className={`titlebar-toggle${motionOff ? " off" : ""}`}
          data-prio="2"
          onClick={onToggleMotion}
        >
          <MotionIcon width={12} height={12} />
          Анимации
        </button>
        {/* Кнопки окна — своим блоком за разделителем. */}
        <span className="window-group">
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
            className="titlebar-btn"
            title={maximized ? "Вернуть в окно" : "Развернуть"}
            aria-label={maximized ? "Вернуть в окно" : "Развернуть"}
            onClick={() => void toggleMaximize()}
          >
            {maximized ? <SquaresIcon /> : <SquareIcon />}
          </button>
          <button
            type="button"
            className="titlebar-btn close"
            title="Закрыть"
            aria-label="Закрыть"
            onClick={() => win()?.close().catch(() => undefined)}
          >
            <CrossIcon />
          </button>
        </span>
      </div>
    </div>
  );
}
