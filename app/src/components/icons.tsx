// SVG-иконки интерфейса (восстановлены из дизайна приложения).

import type { SVGProps } from "react";

function Base({ children, ...rest }: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" {...rest}>
      {children}
    </svg>
  );
}

/** Дом (Бенчмарк). */
export function HomeIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base {...props}>
      <path d="M2.75 10.2 12 2.7l9.25 7.5a1.15 1.15 0 0 1-1.45 1.78l-.55-.45v7.92A1.8 1.8 0 0 1 17.45 21H6.55a1.8 1.8 0 0 1-1.8-1.8v-7.67l-.55.45a1.15 1.15 0 1 1-1.45-1.78Z" />
    </Base>
  );
}

/** Питание (Схемы). */
export function PowerIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" {...props}>
      <path d="M12 4.11v8.5" />
      <path d="M7.2 7.21a7.25 7.25 0 1 0 9.6 0" />
    </Base>
  );
}

/** Куб (Результаты). */
export function CubeIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base {...props}>
      <path
        fillRule="evenodd"
        clipRule="evenodd"
        d="M12 2.4 20.6 6.9v10.2L12 21.6 3.4 17.1V6.9L12 2.4Zm0 2.7 5.9 3.3-5.9 3.4-5.9-3.4L12 5.1Zm-6.3 5.1 5.2 3v6.9L5.7 17V10.2Zm7.4 9.9v-6.9l5.2-3V17l-5.2 3.1Z"
      />
    </Base>
  );
}

/** Список (Логи). */
export function ListIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base {...props}>
      <path
        fillRule="evenodd"
        clipRule="evenodd"
        d="M6 2.5h12A2.5 2.5 0 0 1 20.5 5v14a2.5 2.5 0 0 1-2.5 2.5H6A2.5 2.5 0 0 1 3.5 19V5A2.5 2.5 0 0 1 6 2.5Zm2 4a1 1 0 0 0 0 2h8a1 1 0 1 0 0-2H8Zm-1 5a1 1 0 0 1 1-1h8a1 1 0 1 1 0 2H8a1 1 0 0 1-1-1Zm1 3a1 1 0 0 0 0 2h6a1 1 0 1 0 0-2H8Z"
      />
    </Base>
  );
}

/** Шестерня (Настройки). */
export function GearIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base {...props}>
      <path
        fillRule="evenodd"
        clipRule="evenodd"
        d="m10.45 2.2-.55 2.05c-.55.2-1.07.5-1.55.9L6.3 4.6 4.45 6.45 5 8.5c-.4.48-.7 1-.9 1.55l-2.05.55v2.8l2.05.55c.2.55.5 1.07.9 1.55l-.55 2.05L6.3 19.4l2.05-.55c.48.4 1 .7 1.55.9l.55 2.05h3.1l.55-2.05c.55-.2 1.07-.5 1.55-.9l2.05.55 1.85-1.85L19 15.5c.4-.48.7-1 .9-1.55l2.05-.55v-2.8l-2.05-.55c-.2-.55-.5-1.07-.9-1.55l.55-2.05L17.7 4.6l-2.05.55c-.48-.4-1-.7-1.55-.9l-.55-2.05h-3.1ZM12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Z"
      />
    </Base>
  );
}

/** Минус (свернуть). */
export function MinusIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" {...props}>
          <path d="M5 12h14" stroke="currentColor" strokeWidth="2" strokeLinecap="round" fill="none" />
    </svg>
  );
}

/** Крестик (закрыть). */
export function CloseIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" {...props}>
      <path
        d="M7 7l10 10M17 7L7 17"
        stroke="currentColor"
            strokeWidth="2"
        strokeLinecap="round"
        fill="none"
      />
    </svg>
  );
}

/** Шеврон (свернуть/развернуть меню). */
export function ChevronIcon({ flip = false, ...rest }: SVGProps<SVGSVGElement> & { flip?: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden="true"
      style={flip ? { transform: "rotate(180deg)" } : undefined}
      {...rest}
    >
      <path
        d="m15.5 5-7 7 7 7"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        fill="none"
      />
    </svg>
  );
}

/** Звезда контур (в избранное). */
export function StarOutlineIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base {...props}>
      <path d="M12 4.06l2.58 5.23 5.77.84-4.18 4.07.99 5.74L12 17.23l-5.16 2.71.99-5.74L3.65 10.13l5.77-.84L12 4.06Zm0 2.7 1.8 3.65 4.03.59-2.92 2.85.69 4.01L12 15.96l-3.6 1.9.69-4.01-2.92-2.85 4.03-.59L12 6.76Z" />
    </Base>
  );
}

/** Звезда заливка (в избранном). */
export function StarFilledIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base {...props}>
      <path d="M12 4.06l2.58 5.23 5.77.84-4.18 4.07.99 5.74L12 17.23l-5.16 2.71.99-5.74L3.65 10.13l5.77-.84L12 4.06Z" />
    </Base>
  );
}

/** Минус в круге (исключить). */
export function MinusCircleIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" {...props}>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M7.4 12h9.2" />
    </Base>
  );
}

/** Папка (открыть каталог с отчётами или настройками). */
export function FolderIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} {...props}>
      <path d="M21 19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4.5l2 3H19a2 2 0 0 1 2 2Z" />
    </Base>
  );
}

/** Солнце (тема оформления). */
export function ThemeIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" {...props}>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2.6v1.9M12 19.5v1.9M4.9 4.9l1.35 1.35M17.75 17.75l1.35 1.35M2.6 12h1.9M19.5 12h1.9M6.25 17.75l-1.35 1.35M19.1 4.9l-1.35 1.35" />
    </Base>
  );
}

/** Молния (уменьшить движение). */
export function MotionIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinejoin="round" {...props}>
      <path d="M13.5 2.5 4 14h7l-.5 7.5L20 10h-7Z" />
    </Base>
  );
}

/* --- Иконки разделов по разбору оболочки ---
   Смысл прежней была сбита: «Бенчмарк» домиком читался как «главная»,
   «Результаты» кубом — как «пакет». Теперь каждая иконка называет раздел. */

/** Спидометр — «Бенчмарк», единственный раздел, который запускает замер. */
export function GaugeIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" {...props}>
      <path d="M4.6 16.87a8 8 0 1 1 14.8 0" />
      <path d="M12 16.87l4.2-4.8" />
      <circle cx="12" cy="16.87" r="1.3" />
    </Base>
  );
}

/** Вилка — «Схемы питания»: речь о схемах, а не о выключении. Чаша шире
 *  прежней, иначе глиф был заметно уже соседей по оптической массе. */
export function PlugIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" {...props}>
      <path d="M8.5 3v5.5M15.5 3v5.5" />
      <path d="M5.5 8.5h13v2.2a6.5 6.5 0 0 1-13 0z" />
      <path d="M12 17.2V21" />
    </Base>
  );
}

/** Столбцы — «Результаты»: внутри таблицы, медианы и сравнение. */
export function BarsIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" {...props}>
      <path d="M5 19.5V13M10 19.5V7M15 19.5v-9M20 19.5V4.5" />
      <path d="M2.5 19.5h19" />
    </Base>
  );
}

/** Строки журнала — «Логи». Размах строк подобран под остальные иконки: у
 *  трёх коротких линий оптическая высота получалась заметно меньше, и пункт
 *  выглядел легче остальных. */
export function LogLinesIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" {...props}>
      <path d="M4 6.5h16M4 12h16M4 17.5h11" />
    </Base>
  );
}

/** Меню (свернуть/развернуть боковую панель). */
export function MenuIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" {...props}>
      <path d="M4 6h16M4 12h10M4 18h16" />
    </Base>
  );
}

/** Квадрат окна (развернуть). */
export function SquareIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.3} {...props}>
      <rect x="3" y="3" width="18" height="18" rx="2.4" />
    </Base>
  );
}

/** Два наложенных квадрата (вернуть в окно). */
export function SquaresIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.3} strokeLinecap="round" {...props}>
      <rect x="3" y="7.5" width="13" height="13" rx="2.2" />
      <path d="M8.5 7.5V5.9A2.4 2.4 0 0 1 10.9 3.5H18.6A2.4 2.4 0 0 1 21 5.9v7.7A2.4 2.4 0 0 1 18.6 16H17" />
    </Base>
  );
}

/** Закрыть. */
export function CrossIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.3} strokeLinecap="round" {...props}>
      <path d="M5 5l14 14M19 5 5 19" />
    </Base>
  );
}

/** Корзина (удалить). */
export function TrashIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...props}>
      <path d="M6.6 7.2h10.8" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
      <path d="M9.6 4.5h4.8" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
      <path
        d="M7.5 7.2 8.4 18.3a1.5 1.5 0 0 0 1.49 1.38h4.22A1.5 1.5 0 0 0 15.6 18.3L16.5 7.2"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path d="M10.5 10.5v5.1" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
      <path d="M13.5 10.5v5.1" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  );
}

/** Щит (права администратора). */
export function ShieldIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinejoin="round" {...props}>
      <path d="M12 2.5 19 5.5v6c0 4.8-3.2 7.9-7 9.5-3.8-1.6-7-4.7-7-9.5v-6l7-3Z" />
      <path d="m9.2 11.8 2 2 3.6-3.8" strokeLinecap="round" />
    </Base>
  );
}

/** Розетка/вилка (питание от сети). */
export function SocketIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" {...props}>
      <path d="M9 2.5v5.5M15 2.5v5.5M6.5 8h11v3.5a5.5 5.5 0 0 1-11 0V8Z" />
      <path d="M12 17v4.5" />
    </Base>
  );
}

/** Экспорт (стрелка вверх из лотка). */
export function ExportIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" {...props}>
      <path d="M12 15V3.5" />
      <path d="M7 8.5 12 3.5l5 5" />
      <path d="M5 15.5V19a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-3.5" />
    </Base>
  );
}

/** Лупа (поиск). */
export function SearchIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" {...props}>
      <path
        d="M10.5 4.2a6.3 6.3 0 1 0 0 12.6a6.3 6.3 0 0 0 0-12.6Z"
        stroke="currentColor"
        strokeWidth="1.8"
      />
      <path d="m15 15 4.8 4.8" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  );
}

/** Плюс (импорт схемы из файла). */
export function PlusIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" {...props}>
      <path d="M12 5.5v13M5.5 12h13" />
    </Base>
  );
}

/** Круговая стрелка (вернуть стандартные схемы, обновить список). */
export function RestoreIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" {...props}>
      <path d="M2.7 5V11.5h6.5" />
      <path d="M4.3 12.7A8.5 8.5 0 1 0 6.7 6.1L2.7 10" />
    </Base>
  );
}

/** Сетка (вид списка схем плитками). */
export function GridIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={2} {...props}>
      <rect x="3.5" y="3.5" width="7" height="7" rx="1.5" />
      <rect x="13.5" y="3.5" width="7" height="7" rx="1.5" />
      <rect x="13.5" y="13.5" width="7" height="7" rx="1.5" />
      <rect x="3.5" y="13.5" width="7" height="7" rx="1.5" />
    </Base>
  );
}

/** Список (компактный вид схем). */
export function ListViewIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" {...props}>
      <path d="M8.5 6.5H20M8.5 12H20M8.5 17.5H20" />
      <path d="M4 6.5h.01M4 12h.01M4 17.5h.01" />
    </Base>
  );
}

/** Документ (HTML-отчёт). */
export function ReportIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" {...props}>
      <path d="M14 2.5H6.5a1.5 1.5 0 0 0-1.5 1.5v16a1.5 1.5 0 0 0 1.5 1.5h11a1.5 1.5 0 0 0 1.5-1.5V7.5Z" />
      <path d="M14 2.5V7.5h5" />
      <path d="M8.5 13h7M8.5 16.5h7" />
    </Base>
  );
}
