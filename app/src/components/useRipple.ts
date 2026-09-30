// Ripple от точки нажатия на кнопках с фоном.
//
// Один делегированный обработчик на документ вместо слушателя на каждой
// кнопке: кнопок в приложении сотни, и на странице схем их больше ста.
// Волна рисуется обычным элементом и снимает себя по `animationend` — ни
// таймеров, ни очистки состояния.

import { useEffect } from "react";

/** Классы кнопок, на которых рисуется волна. */
const RIPPLE_HOSTS = [
  ".btn",
  ".titlebar-btn",
  ".act-primary",
  ".act-secondary",
  ".btn-next",
  ".btn-launch-main",
  ".btn-open-report",
  ".btn-delete",
  ".btn-report",
  ".btn-back",
  ".adv-toggle",
  ".act-pill",
  ".sc-edit-link",
  ".btn-reset-params",
].join(", ");

let installed = false;

function install(): void {
  if (installed) return;
  installed = true;
  document.addEventListener(
    "pointerdown",
    (e) => {
      if (document.documentElement.dataset.motion === "off") return;
      const host = (e.target as HTMLElement | null)?.closest?.<HTMLElement>(RIPPLE_HOSTS);
      if (!host || host.hasAttribute("disabled")) return;
      const r = host.getBoundingClientRect();
      const size = Math.max(r.width, r.height);
      const span = document.createElement("span");
      span.className = "pb-ripple";
      span.style.width = `${size}px`;
      span.style.height = `${size}px`;
      span.style.left = `${e.clientX - r.left - size / 2}px`;
      span.style.top = `${e.clientY - r.top - size / 2}px`;
      span.addEventListener("animationend", () => span.remove(), { once: true });
      // Кнопка должна уметь обрезать волну по своим скруглённым краям.
      if (getComputedStyle(host).position === "static") host.style.position = "relative";
      host.appendChild(span);
    },
    { passive: true },
  );
}

/** Включить делегированный ripple один раз на приложение. */
export function useRipple(): void {
  useEffect(install, []);
}
