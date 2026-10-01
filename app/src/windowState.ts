// Контракт окна: размер по умолчанию, минимальный размер и запоминание
// положения между запусками.
//
// Откуда эти числа. Контентной колонке нужно 1120 px, плюс 80 px отступов и
// 64 px меню — 1264 px. Старое окно 1200×800 было меньше этой суммы, и
// готовую вёрстку сжимало. Ширина 1440 даёт запас, а 1180×720 — нижняя
// граница, ниже которой сетка схем уже не держит три колонки.

import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Размер при первом запуске. */
export const WINDOW_DEFAULT = { w: 1440, h: 900 } as const;
/** Ниже этого размера раскладка не гарантируется. */
export const WINDOW_MIN = { w: 1180, h: 720 } as const;

const STORAGE_KEY = "pb.window";
/** Перетаскивание окна даёт события чаще, чем раз в 400 мс, а на каждое
 *  событие Tauri отдаёт размер и позицию — писать их в хранилище на каждом
 *  движении мыши незачем. */
const SAVE_DEBOUNCE_MS = 400;

export interface WindowBox {
  w: number;
  h: number;
  x: number;
  y: number;
}

/** Окно Tauri либо `null`, если код открыт в обычном браузере (`vite dev`). */
function currentWindow() {
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

function readSaved(): WindowBox | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as Partial<WindowBox>;
    if (typeof v.w !== "number" || typeof v.h !== "number") return null;
    if (!Number.isFinite(v.w) || !Number.isFinite(v.h)) return null;
    return {
      // Размер из прошлого запуска может быть меньше нынешнего минимума —
      // например, после смены требований к раскладке.
      w: Math.max(WINDOW_MIN.w, Math.round(v.w)),
      h: Math.max(WINDOW_MIN.h, Math.round(v.h)),
      x: typeof v.x === "number" && Number.isFinite(v.x) ? Math.round(v.x) : 0,
      y: typeof v.y === "number" && Number.isFinite(v.y) ? Math.round(v.y) : 0,
    };
  } catch {
    // Повреждённое значение — просто начинаем с умолчаний.
    return null;
  }
}

/**
 * Применяет контракт окна: минимальный размер, размер и позицию.
 *
 * Позиция восстанавливается только если она была сохранена: при первом
 * запуске окно центрируется конфигом (`center: true` в `tauri.conf.json`).
 */
export async function initWindow(): Promise<void> {
  const win = currentWindow();
  if (!win) return;
  try {
    await win.setMinSize(new LogicalSize(WINDOW_MIN.w, WINDOW_MIN.h));
  } catch {
    // Окно может быть недоступно (например, при выходе из приложения).
  }
  const saved = readSaved();
  const w = saved?.w ?? WINDOW_DEFAULT.w;
  const h = saved?.h ?? WINDOW_DEFAULT.h;
  try {
    await win.setSize(new LogicalSize(w, h));
    if (saved) await win.setPosition(new LogicalPosition(saved.x, saved.y));
  } catch {
    // Размер, заданный конфигом, останется в силе.
  }

  let timer = 0;
  const remember = () => {
    if (timer) window.clearTimeout(timer);
    timer = window.setTimeout(() => {
      timer = 0;
      void (async () => {
        try {
          const size = await win.outerSize();
          const pos = await win.outerPosition();
          // `scaleFactor` переводит физические пиксели в логические: на
          // мониторе с масштабом 150 % иначе запомнилось бы 2160×1350.
          const scale = await win.scaleFactor();
          const box: WindowBox = {
            w: Math.round(size.width / scale),
            h: Math.round(size.height / scale),
            x: Math.round(pos.x / scale),
            y: Math.round(pos.y / scale),
          };
          localStorage.setItem(STORAGE_KEY, JSON.stringify(box));
        } catch {
          // Не смогли прочитать геометрию — запоминать нечего.
        }
      })();
    }, SAVE_DEBOUNCE_MS);
  };
  try {
    // События окна, а не `resize` браузера: перетаскивание мышью не даёт
    // `resize`, а именно перетаскиванием меняют положение.
    await win.onResized(remember);
    await win.onMoved(remember);
  } catch {
    // Без запоминания приложение работает, просто не восстанавливает окно.
  }
}

/** Полный экран (F11 и кнопка окна). При выходе размер возвращается прежний. */
export async function toggleFullscreen(): Promise<boolean> {
  const win = currentWindow();
  if (!win) return false;
  try {
    // В Tauri v2 отдельного `toggleFullscreen` нет — состояние читается и
    // переключается явно.
    const next = !(await win.isFullscreen());
    await win.setFullscreen(next);
    return next;
  } catch {
    return false;
  }
}
