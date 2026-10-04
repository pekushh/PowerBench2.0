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
// и по фокусу, закрывается по Escape и при уходе указателя.
//
// Два решения, из-за которых подсказка стояла не там, где надо:
//
// 1. `position: fixed` не означает «от окна». Любой предок с `transform`,
//    `filter`, `will-change: transform` или `contain: layout` становится
//    содержащим блоком для фиксированного потомка. В приложении таких
//    предков два: `.modal` играет `pb-pop-in` с `fill-mode: both`, а
//    последний кадр анимации заканчивается на `transform: scale(1)` — это
//    не `none`, поэтому модалка остаётся containing block и после конца
//    анимации; и `.scheme-card` / `.pick-card` с `contain: layout`.
//    Координаты из `getBoundingClientRect()` относятся к окну, а применились
//    они к модалке — и подсказка уезжала на её смещение от верхнего левого
//    угла, то есть визуально «куда-то в угол». Поэтому подсказка рендерится
//    порталом в `document.body`: её предком становится сам `body`, и ни один
//    элемент страницы уже не может перехватить позиционирование.
//
// 2. Портал снимает и обрезание: скроллящийся предок с `overflow: hidden`
//    (`.section-card`, `.schemes-scroll`) подсказку больше не срезает.
//
// Положение считается после отрисовки (размер известен только тогда): сначала
// выбирается сторона по наличию места, затем ширина прижимается к краям окна.
// Подсказка никогда не перекрывает свой контрол.

import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

/** Сколько ждать наведения, прежде чем показать подсказку. */
const OPEN_DELAY_MS = 350;
/** Сколько ждать после ухода указателя, прежде чем скрыть. */
const CLOSE_DELAY_MS = 120;
/** Зазор между контролом и подсказкой. */
const GAP_PX = 8;
/** Отступ от края окна: подсказка не должна упираться в границу. */
const EDGE_PX = 8;
const MAX_W = 340;
const MIN_W = 160;

/** Куда можно показать подсказку: четыре стороны, выбирается по месту. */
type Side = "top" | "bottom";

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
  side?: Side;
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

  // Положение пересчитываем не только при открытии. Пока подсказка видна,
  // страницу можно прокрутить, а окно — изменить в размере (перетащили или
  // развернули): без подписей подсказка остаётся на старом месте и уезжает
  // от своего значка. Подписываемся в фазе захвата, потому что прокручиваются
  // вложенные прокручиваемые области, а не только окно.
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const a = anchor.current;
      const b = bubble.current;
      if (!a || !b) return;
      const r = a.getBoundingClientRect();
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      // ШиринаKnown только после отрисовки, и она же ограничивается окном:
      // на узком окне подсказка не должна быть шире его полезной части.
      const width = Math.min(MAX_W, Math.max(b.offsetWidth, MIN_W), vw - EDGE_PX * 2);
      const height = b.offsetHeight;

      // Сторона: сперва запрошенная, противоположная — если в ней больше
      // места. Затем прижимание к краям: подсказка показывается целиком
      // либо не показывается вовсе, но никогда не наезжает на контрол.
      let chosen: Side = side;
      if ((side === "bottom" ? vh - r.bottom : r.top) < height + GAP_PX + EDGE_PX) {
        const other: Side = side === "bottom" ? "top" : "bottom";
        if ((other === "bottom" ? vh - r.bottom : r.top) > (side === "bottom" ? vh - r.bottom : r.top)) {
          chosen = other;
        }
      }
      let top = chosen === "bottom" ? r.bottom + GAP_PX : r.top - height - GAP_PX;
      if (top + height > vh - EDGE_PX) top = vh - EDGE_PX - height;
      if (top < EDGE_PX) top = EDGE_PX;

      let left = r.left;
      if (left + width > vw - EDGE_PX) left = vw - EDGE_PX - width;
      if (left < EDGE_PX) left = EDGE_PX;
      setPos({ top, left });
    };

    place();
    window.addEventListener("scroll", place, true);
    window.addEventListener("resize", place);
    return () => {
      window.removeEventListener("scroll", place, true);
      window.removeEventListener("resize", place);
    };
  }, [open, side, text]);

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
      {open
        ? createPortal(
            <span
              className="tt-bubble glass float"
              ref={bubble}
              role="tooltip"
              id={id}
              // До первого расчёта подсказка невидима, но уже в потоке, иначе
              // при первом кадре она мелькнёт в верхнем левом углу окна.
              style={{
                top: pos?.top ?? 0,
                left: pos?.left ?? 0,
                visibility: pos ? "visible" : "hidden",
              }}
              // Указатель не должен наезжать на саму подсказку: наведение на
              // текст внутри неё иначе вызывает hide.
              onPointerEnter={clear}
              onPointerLeave={hide}
            >
              {text}
            </span>,
            document.body,
          )
        : null}
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