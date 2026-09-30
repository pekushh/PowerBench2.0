// Каскад появления элементов.
//
// Проблема, которую это решает. Все шесть экранов смонтированы одновременно и
// переключаются классом `.on`, поэтому анимация `animation … backwards` на
// детях страницы проигрывалась ровно один раз — при первом открытии. При
// возврате на вкладку элементы те же самые, узлы те же, анимация уже доиграна:
// со второй-третьей попытки экраны выходили «мёртвыми», и хуже всего —
// переход назад выглядел иначе, чем вперёд (вперёд анимация ещё не успела
// доиграть к моменту ухода).
//
// Решение: анимация живёт на классе `.pb-cascade`, а хук снимает и через кадр
// возвращает этот класс. Смена `animation-name` перезапускает анимацию без
// перемонтирования: данные не перезапрашиваются, скролл-позиция сохраняется.

import { useEffect, useRef } from "react";
/**
 * Класс каскада на корне экрана. `on` — когда экран видим.
 *
 * @param on экран сейчас показан
 * @param deps значения, при которых каскад нужно перезапустить (обычно `[]`
 *             и пустой перезапуск на каждое открытие)
 */
export function useCascade<T extends HTMLElement>(
  on: boolean,
  deps: unknown[] = [],
): React.RefObject<T> {
  const ref = useRef<T>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (!on) {
      el.classList.remove("pb-cascade");
      return;
    }
    // Кадр без класса и потом с классом: иначе браузер посчитает, что
    // `animation-name` не менялся, и не перезапустит анимацию.
    el.classList.remove("pb-cascade");
    const raf = requestAnimationFrame(() => {
      el.classList.add("pb-cascade");
    });
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [on, ...deps]);
  return ref;
}

/**
 * «Оживление» списка после фильтрации.
 *
 * На каждый символ в поиске перерисовывать анимацию нельзя: список мигает и
 * перерисовывается на каждый введённый символ. Поэтому ждём паузу в наборе и
 * только после неё один раз проигрываем появление строк.
 */
export function usePopOnFilter<T extends HTMLElement>(
  dep: unknown,
  delayMs = 180,
): React.RefObject<T> {
  const ref = useRef<T>(null);
  const first = useRef(true);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // Первое появление отдаём каскаду экрана: дважды проигрывать одно и то же
    // не нужно.
    if (first.current) {
      first.current = false;
      return;
    }
    el.classList.remove("pb-pop-list");
    const id = window.setTimeout(() => el.classList.add("pb-pop-list"), delayMs);
    return () => window.clearTimeout(id);
  }, [dep, delayMs]);
  return ref;
}