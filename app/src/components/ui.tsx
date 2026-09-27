// Базовые элементы интерфейса (восстановлены из дизайна приложения).

import { useEffect, useRef, useState, type ReactNode } from "react";

/** Затемнение краёв только там, где реально есть скрытый скролл. */
export function useScrollFade<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [fade, setFade] = useState({ top: false, bottom: false, right: false });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const vCan = el.scrollHeight - el.clientHeight > 4;
      const hCan = el.scrollWidth - el.clientWidth > 4;
      const next = {
        top: vCan && el.scrollTop > 4,
        bottom: vCan && el.scrollTop + el.clientHeight < el.scrollHeight - 4,
        right: hCan && el.scrollLeft + el.clientWidth < el.scrollWidth - 4,
      };
      // Без bail-out MutationObserver дёргал бы setState на каждое изменение DOM.
      setFade((prev) =>
        prev.top === next.top && prev.bottom === next.bottom && prev.right === next.right
          ? prev
          : next,
      );
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    const ro = new ResizeObserver(update);
    ro.observe(el);
    const mo = new MutationObserver(update);
    mo.observe(el, { childList: true, subtree: true, characterData: true });
    return () => {
      el.removeEventListener("scroll", update);
      ro.disconnect();
      mo.disconnect();
    };
  }, []);
  return { ref, ...fade };
}

/**
 * Мягкий свет за курсором (spotlight). Без ре-рендеров: координаты пишутся
 * напрямую в CSS-переменные через rAF, уважает reduce-motion (см. CSS).
 */
export function useSpotlight<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let raf = 0;
    const onMove = (e: PointerEvent) => {
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
export function FadeScroll({
  className = "",
  children,
  ...rest
}: React.HTMLAttributes<HTMLDivElement>) {
  const { ref, top, bottom, right } = useScrollFade<HTMLDivElement>();
  const cls = [className, top ? "fade-top" : "", bottom ? "fade-bottom" : "", right ? "fade-right" : ""]
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
    <button className={cls} {...rest}>
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
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
}) {
  return (
    <div className="seg">
      {options.map((o) => (
        <button key={o.value} className={o.value === value ? "on" : ""} onClick={() => onChange(o.value)}>
          <span>{o.label}</span>
        </button>
      ))}
    </div>
  );
}

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
    <div className="field">
      <label>{label}</label>
      {children}
      {hint ? <div className="field-hint">{hint}</div> : null}
    </div>
  );
}

/** Числовое поле с единицей измерения внутри рамки. */
export function NumInput({
  value,
  onChange,
  min,
  max,
  unit,
}: {
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  unit?: string;
}) {
  const shown = Number.isFinite(value) ? value : (min ?? 0);
  return (
    <span className="num-in">
      <input
        type="number"
        min={min}
        max={max}
        step={1}
        value={shown}
        onChange={(e) => {
          // Округляем и зажимаем сразу: иначе «9.5» или «-3» уходит в бэкенд
          // и возвращает пользователю сырую ошибку serde.
          const raw = Number(e.target.value);
          if (!Number.isFinite(raw)) {
            onChange(min ?? 0);
            return;
          }
          const v = Math.round(raw);
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
  children,
}: {
  open: boolean;
  title: ReactNode;
  onClose: () => void;
  footer?: ReactNode;
  wide?: boolean;
  children: ReactNode;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className={`modal${wide ? " wide" : ""}`} onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <div className="modal-title">{title}</div>
          <button className="modal-close" onClick={onClose} aria-label="Закрыть">
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
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open ]);
  const cur = options.find((o) => o.value === value);
  return (
    <div className="select" ref={ref} title={title}>
      <button
        type="button"
        className="dd-btn"
        aria-haspopup="listbox"
        aria-expanded={open}
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
