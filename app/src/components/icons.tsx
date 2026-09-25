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
      <path d="M12 3.5v8.5" />
      <path d="M7.2 6.6a7.25 7.25 0 1 0 9.6 0" />
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
      <path d="M5 12h14" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" fill="none" />
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
        strokeWidth="2.2"
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
        d="m15 5-7 7 7 7"
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
      <path d="M12 2.9l2.58 5.23 5.77.84-4.18 4.07.99 5.74L12 16.07l-5.16 2.71.99-5.74L3.65 8.97l5.77-.84L12 2.9Zm0 2.7 1.8 3.65 4.03.59-2.92 2.85.69 4.01L12 14.8l-3.6 1.9.69-4.01-2.92-2.85 4.03-.59L12 5.6Z" />
    </Base>
  );
}

/** Звезда заливка (в избранном). */
export function StarFilledIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <Base {...props}>
      <path d="M12 2.9l2.58 5.23 5.77.84-4.18 4.07.99 5.74L12 16.07l-5.16 2.71.99-5.74L3.65 8.97l5.77-.84L12 2.9Z" />
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

/** Корзина (удалить). */
export function TrashIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true" {...props}>
      <path d="M5.5 6h9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      <path d="M8 3.75h4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      <path
        d="M6.25 6 7 15.25a1.25 1.25 0 0 0 1.24 1.15h3.52A1.25 1.25 0 0 0 13 15.25L13.75 6"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path d="M8.75 8.75v4.25" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      <path d="M11.25 8.75v4.25" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

/** Лупа (поиск). */
export function SearchIcon(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 20 20" fill="none" aria-hidden="true" {...props}>
      <path
        d="M8.75 3.5a5.25 5.25 0 1 0 0 10.5a5.25 5.25 0 0 0 0-10.5Z"
        stroke="currentColor"
        strokeWidth="1.7"
      />
      <path d="m12.5 12.5 4 4" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" />
    </svg>
  );
}
