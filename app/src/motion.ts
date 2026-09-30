// Моторика «Graphite Fresh»: один выключатель на всё приложение.
//
// Три источника правды: системный `prefers-reduced-motion` живёт в CSS, ручной
// выключатель ставит `data-motion="off"` на `<html>` (его гасит правило
// `html[data-motion="off"] *`), а класс `reduce-motion` остаётся для уже
// написанных правил интерфейса. Источник истины — `appsettings.appearance.
// reduce_motion`: выбор синхронизируется с бэкендом и переживает перезапуск,
// поэтому он не дублируется в localStorage.

/** Применить выбор к документу. */
export function applyMotion(off: boolean): void {
  const root = document.documentElement;
  root.dataset.motion = off ? "off" : "on";
  root.classList.toggle("reduce-motion", off);
}

/** Выключены ли анимации по ручному выбору. */
export function motionOff(): boolean {
  return document.documentElement.dataset.motion === "off";
}
