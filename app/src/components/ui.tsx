// Базовые элементы интерфейса (восстановлены из дизайна приложения).

import { useEffect, useRef, useState, type ReactNode } from "react";
import { usePill } from "./usePill";

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
export function useSpotlight<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const off = () =>
      document.documentElement.classList.contains("reduce-motion") ||
      document.documentElement.classList.contains("bench-running");
    let raf = 0;
    const onMove = (e: PointerEvent) => {
      if (off()) {
        el.classList.remove("spot-on");
        return;
      }
      cancelAnimationFrame(raf);
      const r = el.getBoundingClientRect();
      const x = e.clientX - r.left;
      const y = e.clientY - r.top;
      raf = requestAnimationFrame(() => {
        el.style.setProperty("--mx", `${x}px`);
        el.style.setProperty("--my", `${y}px`);
        el.classList.add("spot-on");
      });
    };
    const onLeave = () => {
      cancelAnimationFrame(raf);
      el.classList.remove("spot-on");
    };
    el.addEventListener("pointermove", onMove, { passive: true });
    el.addEventListener("pointerleave", onLeave);
    return () => {
      cancelAnimationFrame(raf);
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerleave", onLeave);
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
  ...rest
}: React.HTMLAttributes<HTMLDivElement>) {
  const { ref, top, bottom } = useScrollFade<HTMLDivElement>();
  const cls = [className, top ? "fade-top" : "", bottom ? "fade-bottom" : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <div ref={ref} className={cls} {...rest}>
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
