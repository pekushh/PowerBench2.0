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

/** Анимации сейчас не идут: волна не отрисовалась бы, а её узел остался бы
 *  висеть белым пятном до перерисовки кнопки. */
function motionOff(): boolean {
  const el = document.documentElement;
  return (
    el.dataset.motion === "off" ||
    el.classList.contains("bench-running") ||
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

function install(): void {
  if (installed) return;
  installed = true;
  document.addEventListener(
    "pointerdown",
    (e) => {
      if (motionOff()) return;
      const host = (e.target as HTMLElement | null)?.closest?.<HTMLElement>(RIPPLE_HOSTS);
      if (!host || host.hasAttribute("disabled")) return;
      // Скрытым кнопкам волна не нужна: в свёрнутом меню и в блоках с
      // нулевой высотой она осталась бы незамеченной, но в разметке висла.
      if (host.hidden || host.offsetParent === null) return;
      const r = host.getBoundingClientRect();
      const size = Math.max(r.width, r.height);
      const span = document.createElement("span");
      span.className = "pb-ripple";
      span.style.width = `${size}px`;
      span.style.height = `${size}px`;
      span.style.left = `${e.clientX - r.left - size / 2}px`;
      span.style.top = `${e.clientY - r.top - size / 2}px`;
      const drop = () => span.remove();
      // Убираем и по событию анимации, и по таймеру: если моторику выключат
      // прямо во время волны, `animationend` не придёт и узел останется.
      span.addEventListener("animationend", drop, { once: true });
      window.setTimeout(drop, 900);
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
