// Кросс-страничное состояние: идёт ли сессия. Простой observable
// без внешних зависимостей.

import { useSyncExternalStore } from "react";

export interface SessionState {
  running: boolean;
  /** Уточнение к названию раздела в шапке: «111 схем», «2 сессии»,
   *  «идёт замер». Публикует активная страница, читает шапка. */
  sectionDetail: string;
}

let state: SessionState = { running: false, sectionDetail: "" };
const listeners = new Set<() => void>();

function notify() {
  for (const l of listeners) l();
}

export function setRunning(running: boolean) {
  if (state.running !== running) {
    state = { ...state, running };
    notify();
  }
}

/** Опубликовать уточнение для шапки. */
export function setSectionDetail(detail: string) {
  if (state.sectionDetail !== detail) {
    state = { ...state, sectionDetail: detail };
    notify();
  }
}

/**
 * `useSyncExternalStore`, а не `useState` + подписка в эффекте.
 *
 * Прежняя схема читала модульное состояние при первом рендере и подписывалась
 * в эффекте: изменение между рендером и эффектом (например, `setRunning` из
 * обработчика события Tauri) терялось, и компонент оставался с устаревшим
 * значением до следующего обновления. Снапшот здесь всегда свежий.
 */
export function useSession(): SessionState {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => state,
  );
}

// ---------- Уведомления ----------

export interface Toast {
  id: number;
  kind: "err" | "okk" | "info";
  text: string;
}

let toasts: Toast[] = [];
let nextId = 1;
const toastListeners = new Set<() => void>();

function toastNotify() {
  for (const l of toastListeners) l();
}

export function pushToast(kind: Toast["kind"], text: string, lifeMs = 6500) {
  const t: Toast = { id: nextId++, kind, text };
  toasts = [...toasts, t];
  toastNotify();
  // Живёт недолго, но всё равно через `setTimeout`: состояние живёт вне
  // React, и `useSyncExternalStore` без такого хука обновляет подписчиков
  // только при следующем изменении списка.
  setTimeout(() => {
    toasts = toasts.filter((x) => x.id !== t.id);
    toastNotify();
  }, lifeMs);
}

export function useToasts(): Toast[] {
  return useSyncExternalStore(
    (cb) => {
      toastListeners.add(cb);
      return () => toastListeners.delete(cb);
    },
    () => toasts,
  );
}
