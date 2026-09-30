// Индикатор активного пункта меню — ровно один на всё меню.
//
// Раньше он создавался на каждую группу (список пунктов и подвал), и у той
// группы, где активного пункта нет, оставался кусок полоски прошлой высоты:
// её прятали только прозрачностью, а любая перерисовка возвращала. Теперь
// индикатор один, лежит прямо в сайдбаре и умеет либо ехать, либо пропадать
// целиком (`height: 0` + `visibility: hidden`).
//
// Позиция пересчитывается при всех событиях, меняющих раскладку: смена
// вкладки (плавно), сворачивание меню (мгновенно, иначе полоска пролетает
// через весь список и оставляет след), изменение размера окна и сайдбара.

import { useEffect, useRef } from "react";

export function useNavIndicator(
  page: string,
  collapsed: boolean,
  sidebarRef: React.RefObject<HTMLElement>,
): void {
  // Подавление анимации на один кадр: после смены раскладки полоска должна
  // встать на место сразу, а не ехать из старой позиции.
  const instantRef = useRef(false);
  const pageRef = useRef(page);

  useEffect(() => {
    const sidebar = sidebarRef.current;
    if (!sidebar) return;
    let bar = sidebar.querySelector<HTMLElement>(":scope > .pb-nav-ind");
    if (!bar) {
      bar = document.createElement("span");
      bar.className = "pb-nav-ind";
      bar.setAttribute("aria-hidden", "true");
      sidebar.prepend(bar);
    }
    let frame = 0;

    const place = () => {
      frame = 0;
      const item = sidebar.querySelector<HTMLElement>(".nav-item.active");
      if (!item) {
        bar.classList.remove("is-on");
        bar.style.height = "0px";
        return;
      }
      const s = sidebar.getBoundingClientRect();
      const i = item.getBoundingClientRect();
      if (instantRef.current) bar.classList.add("is-instant");
      bar.style.height = `${i.height}px`;
      bar.style.transform = `translateY(${i.top - s.top + sidebar.scrollTop}px)`;
      bar.classList.add("is-on");
      if (instantRef.current) {
        instantRef.current = false;
        // Два кадра: первый гасит переход, второй возвращает его, иначе
        // браузер успевает применить `transition: none` к новой позиции.
        requestAnimationFrame(() =>
          requestAnimationFrame(() => bar?.classList.remove("is-instant")),
        );
      }
    };

    const schedule = (instant: boolean) => {
      if (instant) instantRef.current = true;
      if (frame !== 0) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(place);
    };

    // Смена вкладки — плавно, сворачивание — мгновенно.
    if (page !== pageRef.current) {
      pageRef.current = page;
      schedule(false);
    } else {
      schedule(true);
    }

    const onResize = () => schedule(true);
    window.addEventListener("resize", onResize);
    // Сайдбар меняет ширину анимацией: пока она идёт, размеры пункта едут.
    const onWidthEnd = (e: TransitionEvent) => {
      if (e.propertyName === "flex-basis" || e.propertyName === "width") schedule(true);
    };
    sidebar.addEventListener("transitionend", onWidthEnd);
    const ro = new ResizeObserver(() => schedule(true));
    ro.observe(sidebar);
    return () => {
      if (frame !== 0) cancelAnimationFrame(frame);
      window.removeEventListener("resize", onResize);
      sidebar.removeEventListener("transitionend", onWidthEnd);
      ro.disconnect();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, collapsed]);
}
