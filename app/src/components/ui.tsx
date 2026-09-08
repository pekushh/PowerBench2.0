// Мелкие переиспользуемые элементы интерфейса на дизайн-системе.

import type { ReactNode } from "react";

export function Glass({ className = "", children }: { className?: string; children: ReactNode }) {
  return <div className={`glass ${className}`.trim()}>{children}</div>;
}

export function Stat({ label, value, suffix = "" }: { label: string; value: string; suffix?: string }) {
  return (
    <div className="stat">
      <div className="label">{label}</div>
      <div className="value">
        {value}
        {suffix ? <span style={{ fontSize: 13, color: "var(--text-3)" }}> {suffix}</span> : null}
      </div>
    </div>
  );
}

export function Button({
  children,
  variant,
  big = false,
  className = "",
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "danger" | "ghost" | "";
  big?: boolean;
}) {
  const cls = ["btn", variant ?? "", big ? "big" : "", className].filter(Boolean).join(" ");
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
  kind?: "ok" | "warn" | "danger" | "plain";
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
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Field({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div className="field">
      <label>{label}</label>
      {children}
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