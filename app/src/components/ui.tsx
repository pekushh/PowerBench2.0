// Базовые элементы интерфейса (восстановлены из дизайна приложения).

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { usePill } from "./usePill";

/**
 * Позиционирование меню, вынесенного порталом в `document.body`.
 *
 * Зачем портал, а не `position: absolute` рядом с кнопкой:
 *
 *  — у страницы `.page.fill{overflow:hidden}`, и это держит раскладку (на нём
 *    стоит `check:adaptive`). Меню внутри страницы обрезается этим `overflow`:
 *    на «Результатах» выпадающий список форматов экспорта уезжал под панель
 *    фильтров, и половины пунктов не было видно;
 *  — `z-index` не помогает: у `.split-act` есть `isolation: isolate`, то есть
 *    собственный контекст наложения, и `z-index` меню остаётся внутри него.
 *    Поднять слой выше `--z-modal` сквозь `overflow: hidden` предка невозможно
 *    в принципе;
 *  — в портале предком меню становится `body`, где нет ни `transform`, ни
 *    `contain`, а значит нет и containing block, перехватывающего
 *    позиционирование. Тот же приём уже применён к подсказке (`Tooltip.tsx`),
 *    и по той же причине.
 *
 * Позиция считается в координатах окна (`position: fixed`) и пересчитывается на
 * прокрутке (в фазе захвата — прокручиваются вложенные области) и на изменении
 * размера окна. Если внизу места не хватает, меню раскрывается вверх; по
 * горизонтали прижимается к краям окна. До первого расчёта меню невидимо, но
 * уже в потоке — иначе оно мелькнуло бы в левом верхнем углу окна.
 *
 * Возвращает `style` для портированного узла меню.
 */
export function useFloatingMenu(
  open: boolean,
  anchor: React.RefObject<HTMLElement | null>,
  menu: React.RefObject<HTMLElement | null>,
  onClose: () => void,
) {
  const [style, setStyle] = useState<CSSProperties | null>(null);

  useLayoutEffect(() => {
    if (!open) {
      setStyle(null);
      return;
    }
    const GAP = 6;
    const EDGE = 8;
    const place = () => {
      const a = anchor.current;
      const m = menu.current;
      if (!a || !m) return;
      const r = a.getBoundingClientRect();
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const w = m.offsetWidth;
      const h = m.offsetHeight;

      const roomBelow = vh - r.bottom;
      // Раскрываемся вниз, пока там есть место; вверх — только если вниз не
      // помещается, а вверх помещается.
      const below = roomBelow >= h + GAP + EDGE || roomBelow >= r.top;
      let top = below ? r.bottom + GAP : r.top - h - GAP;
      if (top + h > vh - EDGE) top = vh - EDGE - h;
      if (top < EDGE) top = EDGE;

      let left = r.left;
      if (left + w > vw - EDGE) left = vw - EDGE - w;
      if (left < EDGE) left = EDGE;

      setStyle({ position: "fixed", top, left });
    };
    place();
    window.addEventListener("scroll", place, true);
    window.addEventListener("resize", place);
    return () => {
      window.removeEventListener("scroll", place, true);
      window.removeEventListener("resize", place);
    };
  }, [open, anchor, menu]);

  // Закрытие по клику вне и по Escape. Подписка на документ, а не на кнопку:
  // меню в портале, и клик по нему не всплывает до кнопки.
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!anchor.current?.contains(t) && !menu.current?.contains(t)) onClose();
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open, anchor, menu, onClose]);

  return style;
}

/**
 * Меню в портале `document.body`.
 *
 * `className` переносится на портированный узел, поэтому вид (фон, скругление,
 * тень) задаётся теми же правилами, что и раньше, а `z-index` берётся из
 * шкалы слоёв и теперь действительно работает: в портале нет контекста
 * наложения, который его бы ограничил.
 */
export function FloatingMenu({
  open,
  anchor,
  onClose,
  className,
  role = "menu",
  children,
}: {
  open: boolean;
  anchor: React.RefObject<HTMLElement | null>;
  onClose: () => void;
  className: string;
  role?: string;
  children: ReactNode;
}) {
  const menu = useRef<HTMLDivElement | null>(null);
  const style = useFloatingMenu(open, anchor, menu, onClose);
  if (!open) return null;
  return createPortal(
    <div
      className={className}
      role={role}
      ref={menu}
      style={style ?? { position: "fixed", top: 0, left: 0, visibility: "hidden" }}
    >
      {children}
    </div>,
    document.body,
  );
}



/**
 * Ref на прокручиваемый контейнер + признаки «есть что прокрутить» сверху и
 * снизу (классы `fade-top` / `fade-bottom`).
 *
 * Подписка только на `scroll` и `ResizeObserver`, обновление не чаще одного
 * кадра. `MutationObserver` здесь раньше стоял, но он читал `scrollHeight`
 * на каждое изменение DOM — синхронный layout, то есть лишняя работа ровно
 * во время замера. Изменение размеров и так ловится `ResizeObserver`.
 */
export function useScrollFade<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [fade, setFade] = useState({ top: false, bottom: false });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let frame = 0;
    const update = () => {
      frame = 0;
      const canScroll = el.scrollHeight - el.clientHeight > 4;
      const next = {
        top: canScroll && el.scrollTop > 4,
        bottom: canScroll && el.scrollTop + el.clientHeight < el.scrollHeight - 4,
      };
      setFade((prev) =>
        prev.top === next.top && prev.bottom === next.bottom ? prev : next,
      );
    };
    const schedule = () => {
      if (frame === 0) frame = requestAnimationFrame(update);
    };
    update();
    el.addEventListener("scroll", schedule, { passive: true });
    const ro = new ResizeObserver(schedule);
    ro.observe(el);
    return () => {
      el.removeEventListener("scroll", schedule);
      ro.disconnect();
      if (frame !== 0) cancelAnimationFrame(frame);
    };
  }, []);
  return { ref, ...fade };
}

/**
 * Мягкий свет за курсором (spotlight). Без ре-рендеров: координаты пишутся
 * напрямую в CSS-переменные через rAF.
 *
 * Во время замера подсветка выключена (класс `bench-running` на `<html>`), и
 * обработчик движения мыши не делает ничего: иначе каждое движение мыши во
 * время бенчмарка планировало бы кадр с записью в DOM. Проверка классов идёт
 * внутри обработчика, а не один раз при монтировании — иначе подписка
 * осталась бы активной на всю сессию.
 */
/**
 * Мягкий свет за курсором (spotlight). Без ре-рендеров: координаты пишутся
 * напрямую в CSS-переменные через rAF.
 *
 * Один обработчик на весь документ, а не по одному на каждую карточку.
 * Раньше `pointermove` висел на каждом `.spot`: на экране «Схемы» их больше
 * ста, и одно движение мыши запускало сто вызовов `getBoundingClientRect()`
 * (принудительный layout) и сто записей в стиль — отсюда лаг при движении
 * указателя. Теперь событие делегируется: на каждое движение обновляется
 * ровно одна карточка под курсором.
 *
 * Во время замера подсветка выключена (класс `bench-running` на `<html>`), и
 * обработчик движения мыши не делает ничего: иначе каждое движение мыши во
 * время бенчмарка планировало бы кадр с записью в DOM. Проверка классов идёт
 * внутри обработчика, а не один раз при монтировании — иначе подписка
 * осталась бы активной на всю сессию.
 */
let spotlightInstalled = false;

/** Карточка, подсвеченная в прошлом кадре: с неё снимаем свет. */
let spotlightLit: HTMLElement | null = null;

function spotlightOff(): void {
  if (spotlightLit) {
    spotlightLit.classList.remove("spot-on");
    spotlightLit = null;
  }
}

function installSpotlight(): void {
  if (spotlightInstalled) return;
  spotlightInstalled = true;
  let raf = 0;
  const off = () =>
    document.documentElement.classList.contains("reduce-motion") ||
    document.documentElement.classList.contains("bench-running");
  document.addEventListener(
    "pointermove",
    (e) => {
      const el = (e.target as HTMLElement | null)?.closest?.(".spot");
      if (el !== spotlightLit) spotlightOff();
      if (!el) return;
      if (off()) return;
      spotlightLit = el as HTMLElement;
      if (raf !== 0) return;
      const x = e.clientX;
      const y = e.clientY;
      raf = requestAnimationFrame(() => {
        raf = 0;
        const node = spotlightLit;
        if (!node || !node.isConnected) return;
        const r = node.getBoundingClientRect();
        node.style.setProperty("--mx", `${x - r.left}px`);
        node.style.setProperty("--my", `${y - r.top}px`);
        node.classList.add("spot-on");
      });
    },
    { passive: true },
  );
  // Уход указателя с окна и потеря фокуса тоже гасят свет: иначе он «залипал»
  // на последней карточке.
  document.addEventListener("pointerleave", spotlightOff);
  window.addEventListener("blur", spotlightOff);
}

export function useSpotlight<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  useEffect(() => {
    installSpotlight();
    // Если карточка ушла с экрана, а на ней горел свет — снимаем.
    return () => {
      if (ref.current && spotlightLit === ref.current) spotlightOff();
    };
  }, []);
  return ref;
}

/** Обёртка со spotlight-подсветкой за курсором. */
export function Spot({
  className = "",
  children,
  ...rest
}: React.HTMLAttributes<HTMLDivElement>) {
  const ref = useSpotlight<HTMLDivElement>();
  return (
    <div ref={ref} className={`spot ${className}`.trim()} {...rest}>
      {children}
    </div>
  );
}

/** Прокручиваемый блок с умными фейдами по краям. */
/**
 * Прокручиваемый блок с индикацией «есть что прокрутить» сверху и снизу.
 *
 * Только вертикальные края: боковые тени рисовались поверх строк и портили
 * текст в журнале и в длинных списках.
 */
export function FadeScroll({
  className = "",
  children,
  innerRef,
  ...rest
}: React.HTMLAttributes<HTMLDivElement> & {
  /** Ref на сам прокручиваемый блок: нужен хукам, которые вешают классы
   *  анимации прямо на контейнер (например, «оживление» после фильтра). */
  innerRef?: React.RefObject<HTMLDivElement>;
}) {
  const { ref, top, bottom } = useScrollFade<HTMLDivElement>();
  const cls = [className, top ? "fade-top" : "", bottom ? "fade-bottom" : ""]
    .filter(Boolean)
    .join(" ");
  // Свой ref и переданный должны указывать на один узел, поэтому внешний
  // подменяет внутренний.
  const setRef = (node: HTMLDivElement | null) => {
    (ref as { current: HTMLDivElement | null }).current = node;
    if (innerRef) (innerRef as { current: HTMLDivElement | null }).current = node;
  };
  return (
    <div ref={setRef} className={cls} {...rest}>
      {children}
    </div>
  );
}

export function Glass({ className = "", children }: { className?: string; children: ReactNode }) {
  return <div className={`glass ${className}`.trim()}>{children}</div>;
}

export function Panel({
  title,
  hint,
  className = "",
  children,
}: {
  title?: ReactNode;
  hint?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <Glass className={className}>
      {(title ?? hint) ? (
        <div className="card-title">
          {title}
          {hint ? <div className="hint">{hint}</div> : null}
        </div>
      ) : null}
      {children}
    </Glass>
  );
}

export function Stat({ label, value, suffix = "" }: { label: string; value: string; suffix?: string }) {
  return (
    <div className="stat">
      <div className="label">{label}</div>
      <div className="value">
        {value}
        {suffix ? <span className="stat-suffix"> {suffix}</span> : null}
      </div>
    </div>
  );
}

export function Button({
  children,
  variant,
  big = false,
  sm = false,
  className = "",
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "danger" | "ghost" | "";
  big?: boolean;
  sm?: boolean;
}) {
  const cls = ["btn", variant ?? "", big ? "big" : "", sm ? "sm" : "", className]
    .filter(Boolean)
    .join(" ");
  return (
    // `type="button"` по умолчанию: без него кнопка внутри формы отправляет её.
    // `big` шириной в 100%, поэтому в flex-строке он должен уступить место
    // соседям — иначе «Старт» раздвигал бы соседние кнопки.
    <button
      type="button"
      className={cls}
      data-big={big ? "1" : undefined}
      {...rest}
    >
      {children}
    </button>
  );
}

export function Badge({
  kind = "plain",
  children,
  big = false,
  title,
}: {
  kind?: "ok" | "warn" | "danger" | "plain" | "info" | "accent";
  children: ReactNode;
  big?: boolean;
  title?: string;
}) {
  return (
    <span className={`badge ${kind} ${big ? "big" : ""}`.trim()} title={title}>
      {children}
    </span>
  );
}

export function Seg<T extends string>({
  options,
  value,
  onChange,
  label,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
  /** Доступное имя группы; `aria-pressed` на кнопках безымянной группы
   *  скринридер читает как «переключатель» без пояснения, что переключают. */
  label?: string;
}) {
  // «Пилюля» под активной кнопкой: она сама едет между сегментами, поэтому у
  // кнопок свой фон не нужен и лёгких ��ерестроений при переключении нет.
  const { ref } = usePill(value, [options.length]);
  return (
    <div className="seg pb-pill-host" role="group" aria-label={label} ref={ref}>
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          data-value={o.value}
          className={o.value === value ? "on active" : ""}
          aria-pressed={o.value === value}
          onClick={() => onChange(o.value)}
        >
          <span>{o.label}</span>
        </button>
      ))}
    </div>
  );
}

/**
 * Поле с подписью. Подпись оборачивает сам контрол (`<label>` вокруг), иначе
 * она ни с чем не связана: ни мышь, ни скринридер не свяжут «Длительность»
 * с полем ввода.
 */
export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
      {hint ? <span className="field-hint">{hint}</span> : null}
    </label>
  );
}

/** Числовое поле с единицей измерения внутри рамки. */
export function NumInput({
  value,
  onChange,
  min,
  max,
  step,
  unit,
}: {
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  /** Шаг ползунка/стрелок. По умолчанию 1 — все текущие поля целочисленные. */
  step?: number;
  unit?: string;
}) {
  const shown = Number.isFinite(value) ? value : (min ?? 0);
  // Шаг меньше единицы — дробные значения, их округлять нельзя.
  const fractional = step != null && step < 1;
  return (
    <span className="num-in">
      <input
        type="number"
        min={min}
        max={max}
        step={step ?? 1}
        value={shown}
        onChange={(e) => {
          // Округляем и зажимаем сразу: иначе «9.5» или «-3» уходит в бэкенд
          // и возвращает пользователю сырую ошибку serde.
          const raw = Number(e.target.value);
          if (!Number.isFinite(raw)) {
            onChange(min ?? 0);
            return;
          }
          const v = fractional ? Math.round(raw / step) * step : Math.round(raw);
          const lo = min ?? v;
          const hi = max ?? v;
          onChange(Math.min(Math.max(v, lo), hi));
        }}
      />
      {unit ? <i>{unit}</i> : null}
    </span>
  );
}

export function Switch({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: ReactNode;
}) {
  return (
    <label className="switch">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span className="track" />
      {label ? <span className="label">{label}</span> : null}
    </label>
  );
}

export function Modal({
  open,
  title,
  onClose,
  footer,
  wide = false,
  className = "",
  children,
}: {
  open: boolean;
  title: ReactNode;
  onClose: () => void;
  footer?: ReactNode;
  wide?: boolean;
  /** Дополнительный класс окна: у крупных окон своя раскладка шапки и
   *  подвала, и без него их нечем было отличить от обычного диалога. */
  className?: string;
  children: ReactNode;
}) {
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    // Кто открыл — тому вернём фокус при закрытии, иначе после Escape
    // фокус падает на `<body>` и следующий Tab начинает с начала окна.
    const opener = document.activeElement as HTMLElement | null;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== "Tab" || !panel.current) return;
      // Ловушка фокуса: Tab не должен уводить за пределы диалога.
      const items = panel.current.querySelectorAll<HTMLElement>(
        'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
      );
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || !panel.current.contains(active))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (active === last || !panel.current.contains(active))) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    // Фокус внутрь диалога: иначе он остаётся на фоне под оверлеем.
    const focusFirst = window.setTimeout(() => {
      const items = panel.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
      );
      (items && items.length > 0 ? items[0] : panel.current)?.focus();
    }, 0);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.clearTimeout(focusFirst);
      opener?.focus?.();
    };
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className={`modal${wide ? " wide" : ""}${className ? ` ${className}` : ""}`}
        onClick={(e) => e.stopPropagation()}
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-label={typeof title === "string" ? title : undefined}
      >
        <div className="modal-head">
          <div className="modal-title">{title}</div>
          <button type="button" className="modal-close" onClick={onClose} aria-label="Закрыть">
            ×
          </button>
        </div>
        <div className="modal-body">{children}</div>
        {footer ? <div className="modal-foot">{footer}</div> : null}
      </div>
    </div>
  );
}

/** Выпадающий список в стиле приложения (без системного попапа). */
export function Dropdown<T extends string>({
  options,
  value,
  onChange,
  title,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
  title?: string;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      // Escape возвращает фокус на кнопку: иначе он уходит на `<body>`.
      if (e.key === "Escape") {
        setOpen(false);
        trigger.current?.focus();
      }
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);
  const cur = options.find((o) => o.value === value);
  return (
    <div className="select" ref={ref} title={title}>
      <button
        type="button"
        ref={trigger}
        className="dd-btn"
        aria-haspopup="listbox"
        aria-expanded={open}
        // Без `aria-label` у кнопки нет доступного имени: `title` на обёртке
        // скринридер не читает, а текст — только текущее значение.
        aria-label={title ? `${title}: ${cur?.label ?? value}` : cur?.label ?? value}
        onClick={() => setOpen((o) => !o)}
      >
        <span>{cur?.label ?? value}</span>
        <span className="caret">▾</span>
      </button>
      {open ? (
        <div className="dd-menu glass float" role="listbox">
          {options.map((o) => (
            <button
              key={o.value}
              type="button"
              role="option"
              aria-selected={o.value === value}
              className={`dd-item${o.value === value ? " on" : ""}`}
              onClick={() => {
                onChange(o.value);
                setOpen(false);
              }}
            >
              {o.label}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

export function Progress({ value }: { value: number }) {
  const v = Math.max(0, Math.min(100, value));
  return (
    <div className="progress">
      <i style={{ width: `${v}%` }} />
    </div>
  );
}
