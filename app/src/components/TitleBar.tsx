// Кастомный заголовок безрамочного окна: drag-зона, кнопки свернуть/развернуть/закрыть.
// Системные декорации отключены (decorations:false в tauri.conf.json).

import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

export default function TitleBar() {
  const [maxed, setMaxed] = useState(false);

  useEffect(() => {
    let alive = true;
    const sync = () => {
      try {
        getCurrentWindow()
          .isMaximized()
          .then((v) => alive && setMaxed(v))
          .catch(() => undefined);
      } catch {
        /* не под Tauri (браузерный предпросмотр) — игнорируем */
      }
    };
    sync();
    let unlisten: Promise<() => void> | null = null;
    try {
      unlisten = getCurrentWindow().onResized(sync);
    } catch {
      unlisten = null;
    }
    return () => {
      alive = false;
      unlisten?.then((f) => f()).catch(() => undefined);
    };
  }, []);

  async function act(fn: (w: ReturnType<typeof getCurrentWindow>) => Promise<void>) {
    try {
      await fn(getCurrentWindow());
    } catch {
      /* не под Tauri — игнорируем */
    }
    try {
      setMaxed(await getCurrentWindow().isMaximized());
    } catch {
      /* игнорируем */
    }
  }

  return (
    <header className="titlebar" onDoubleClick={() => act((w) => w.toggleMaximize())}>
      <div className="tb-drag" data-tauri-drag-region>
        <span className="tb-mark" />
        <span className="tb-title">PowerBench</span>
      </div>
      <div className="tb-btns">
        <button
          className="tb-btn"
          title="Свернуть"
          aria-label="Свернуть"
          onClick={() => act((w) => w.minimize())}
        >
          <svg width="12" height="12" viewBox="0 0 12 12"><path d="M1 6h10" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" /></svg>
        </button>
        <button
          className="tb-btn"
          title={maxed ? "Восстановить" : "Развернуть"}
          aria-label={maxed ? "Восстановить" : "Развернуть"}
          onClick={() => act((w) => w.toggleMaximize())}
        >
          {maxed ? (
            <svg width="12" height="12" viewBox="0 0 12 12"><path d="M4 1h7v7M3 5H1v6h6V9" stroke="currentColor" strokeWidth="1.3" fill="none" strokeLinejoin="round" /></svg>
          ) : (
            <svg width="12" height="12" viewBox="0 0 12 12"><rect x="1.5" y="1.5" width="9" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.3" fill="none" /></svg>
          )}
        </button>
        <button
          className="tb-btn close"
          title="Закрыть"
          aria-label="Закрыть"
          onClick={() => act((w) => w.close())}
        >
          <svg width="12" height="12" viewBox="0 0 12 12"><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" /></svg>
        </button>
      </div>
    </header>
  );
}
