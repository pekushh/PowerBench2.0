// Подсказка по наведению и фокусу: короткая справка к контролу.
//
// Зачем отдельный компонент, а не `title`:
//  — `title` браузер рисует сам: серый прямоугольник без скругления, поверх
//    всего, с задержкой около секунды и без возможности убрать его по Escape;
//  — он не читается с клавиатуры;
//  — подсказка на 100–130 символов (объяснение, почему нужно 5 повторов, а не
//    один) в `title` нечитаема: она вылезает за край окна и обрезается.
//
// Здесь подсказка — обычный узел с `role="tooltip"`, показывается по наведению
// и по фокусу, закрывается по Escape и при уходе указателя. Положение считается
// после отрисовки: сначала сверху или снизу (по наличию места), затем по ширине
// с прижатием к краю окна. Подсказка никогда не перекрывает сам контрол.

import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";

/** Сколько ждать наведения, прежде чем показать подсказку. */
const OPEN_DELAY_MS = 350;
/** Сколько ждать после ухода указателя, прежде чем скрыть. */
const CLOSE_DELAY_MS = 120;
/** Зазор между контролом и подсказкой. */
const GAP_PX = 8;
/** Отступ от края окна: подсказка не должна упираться в границу. */
const EDGE_PX = 8;
const MAX_W = 340;

export function Tooltip({
  children,
  text,
  side = "bottom",
}: {
  /** Контрол, к которому относится подсказка. */
  children: ReactNode;
  text: ReactNode;
  /** Куда раскрывать по умолчанию. Если там не хватает места — в другую
   *  сторону: обрезанная подсказка хуже, чем не с той стороны. */
  side?: "top" | "bottom";
}) {
  const id = useId();
  const anchor = useRef<HTMLSpanElement>(null);
  const bubble = useRef<HTMLSpanElement>(null);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const timer = useRef(0);

  const clear = useCallback(() => {
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = 0;
  }, []);

  useEffect(() => clear, [clear]);

  // Положение считаем после того, как подсказка появилась в DOM: её размер
  // известен только тогда, а без него нельзя ни выбрать сторону, ни прижать
  // к краю окна.
  useLayoutEffect(() => {
    if (!open) return;
    const a = anchor.current;
    const b = bubble.current;
    if (!a || !b) return;
    const r = a.getBoundingClientRect();
    const bw = b.offsetWidth;
    const bh = b.offsetHeight;
    const room = side === "bottom" ? window.innerHeight - r.bottom : r.top;
    const flip = room < bh + GAP_PX + EDGE_PX;
    const top = (flip ? r.top - bh : r.bottom) + (flip ? -GAP_PX : GAP_PX);
    const width = Math.min(MAX_W, Math.max(bw, 160), window.innerWidth - EDGE_PX * 2);
    let left = r.left;
    if (left + width > window.innerWidth - EDGE_PX) left = window.innerWidth - EDGE_PX - width;
    if (left < EDGE_PX) left = EDGE_PX;
    setPos({ top, left });
  }, [open, side]);

  const show = useCallback(() => {
    clear();
    timer.current = window.setTimeout(() => setOpen(true), OPEN_DELAY_MS);
  }, [clear]);

  const hide = useCallback(() => {
    clear();
    timer.current = window.setTimeout(() => setOpen(false), CLOSE_DELAY_MS);
  }, [clear]);

  return (
    <span
      className="tt"
      ref={anchor}
      onPointerEnter={show}
      onPointerLeave={hide}
      onFocusCapture={() => {
        clear();
        setOpen(true);
      }}
      onBlurCapture={() => {
        clear();
        setOpen(false);
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
        }
      }}
    >
      {children}
      {open ? (
        <span
          className="tt-bubble glass float"
          ref={bubble}
          role="tooltip"
          id={id}
          style={pos ? { top: pos.top, left: pos.left } : { visibility: "hidden" }}
          // Указатель не должен наезжать на саму подсказку: наведение на текст
          // внутри неё иначе вызывает hide.
          onPointerEnter={clear}
          onPointerLeave={hide}
        >
          {text}
        </span>
      ) : null}
    </span>
  );
}

/**
 * Значок «i» с подсказкой — для длинных пояснений, которые не должны быть
 * видны постоянно (объяснение параметра, условие применимости, откуда взялся
 * порог).
 */
export function InfoTip({ text }: { text: ReactNode }) {
  return (
    <Tooltip text={text}>
      <span className="tt-icon" tabIndex={0} role="img" aria-label="Подробнее">
        i
      </span>
    </Tooltip>
  );
}