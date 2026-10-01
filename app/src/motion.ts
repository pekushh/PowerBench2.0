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

/* --- Плотность и масштаб текста (ТЗ II.4) ---
   Те же два множителя живут в CSS как `--k` и `--kt`, а интерфейс только
   ставит атрибуты на `<html>`. Источник истины — конфиг
   `appsettings.appearance.density` / `.text_scale`, поэтому выбор
   переживает перезапуск и не дублируется в localStorage. */

/** Плотность: «compact» · «normal» · «roomy». */
export type Density = "compact" | "normal" | "roomy";
/** Масштаб текста: «s» · «m» · «l». */
export type TextScale = "s" | "m" | "l";

const DENSITIES: readonly Density[] = ["compact", "normal", "roomy"];
const TEXT_SCALES: readonly TextScale[] = ["s", "m", "l"];

/**
 * Применить плотность и масштаб текста к документу.
 *
 * Неизвестное значение (например, из конфига другой версии) берётся как
 * «обычно» и средний масштаб — иначе интерфейс остался бы без токенов.
 */
export function applyDensity(density: string, textScale: string): void {
  const root = document.documentElement;
  const d = (DENSITIES as readonly string[]).includes(density) ? density : "normal";
  const t = (TEXT_SCALES as readonly string[]).includes(textScale) ? textScale : "m";
  root.dataset.density = d;
  root.dataset.text = t;
}
