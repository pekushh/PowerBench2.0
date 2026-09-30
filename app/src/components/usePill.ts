// «Пилюля» под активной вкладкой: элемент `.pb-pill` внутри контейнера
// переезжает под активную кнопку, а сама кнопка теряет собственный фон.
//
// Позиция считается по `offsetLeft/offsetWidth`, поэтому контейнер должен быть
// `position: relative` (класс `.pb-pill-host`) и не иметь внутреннего скролла по
// горизонтали. `ResizeObserver` пересчитывает позу при изменении размеров: без
// него после смены темы, шрифта или числа счётчиков пилюля оставалась на
// старом месте.

import { useEffect, useRef, useState } from "react";

interface PillState {
  left: number;
  width: number;
  ready: boolean;
}

/**
 * Хук вешает пилюлю на активную кнопку внутри контейнера.
 *
 * @param activeValue значение активной кнопки (для `[data-value="…"]`)
 * @param deps список, меняющий раскладку: активная кнопка, счётчики, текст
 */
export function usePill(activeValue: string, deps: unknown[] = []): {
  ref: React.RefObject<HTMLDivElement>;
} {
  const ref = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<PillState>({ left: 0, width: 0, ready: false });

  useEffect(() => {
    const host = ref.current;
    if (!host) return;
    const place = () => {
      const active = host.querySelector<HTMLElement>(`[data-value="${CSS.escape(activeValue)}"]`);
      if (!active) {
        setState((s) => (s.ready ? { ...s, ready: false } : s));
        return;
      }
      setState({ left: active.offsetLeft, width: active.offsetWidth, ready: true });
    };
    place();
    const ro = new ResizeObserver(place);
    ro.observe(host);
    for (const child of Array.from(host.children)) ro.observe(child);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeValue, ...deps]);

  // Пилюля рисуется тем же элементом, что и хост, поэтому монтируем её
  // императивно: лишний рендер ради одного span не нужен.
  useEffect(() => {
    const host = ref.current;
    if (!host) return;
    let pill = host.querySelector<HTMLElement>(":scope > .pb-pill");
    if (!pill) {
      pill = document.createElement("span");
      pill.className = "pb-pill";
      pill.setAttribute("aria-hidden", "true");
      host.prepend(pill);
    }
    pill.style.transform = `translateX(${state.left}px)`;
    pill.style.width = `${state.width}px`;
    pill.style.opacity = state.ready ? "1" : "0";
  }, [state]);

  return { ref };
}
