// Кросс-страничное состояние: идёт ли сессия, последний результат,
// незакрытое уведомление. Простой observable без внешних зависимостей.

import { useEffect, useState } from "react";
import type { FinishedPayload } from "./api";

export interface SessionState {
  running: boolean;
  last: FinishedPayload | null;
}

let state: SessionState = { running: false, last: null };
const listeners = new Set<(s: SessionState) => void>();

function notify() {
  for (const l of listeners) l(state);
}

export function setRunning(running: boolean) {
  if (state.running !== running) {
    state = { ...state, running };
    notify();
  }
}

export function setLastResult(r: FinishedPayload | null) {
  state = { ...state, last: r };
  notify();
}

export function getSession(): SessionState {
  return state;
}

export function useSession(): SessionState {
  const [s, setS] = useState<SessionState>(state);
  useEffect(() => {
    listeners.add(setS);
    return () => {
      listeners.delete(setS);
    };
  }, []);
  return s;
}

// ---------- Уведомления ----------

export interface Toast {
  id: number;
  kind: "err" | "okk" | "info";
  text: string;
}

let toasts: Toast[] = [];
let nextId = 1;
const toastListeners = new Set<(t: Toast[]) => void>();

function toastNotify() {
  for (const l of toastListeners) l(toasts);
}

export function pushToast(kind: Toast["kind"], text: string) {
  const t: Toast = { id: nextId++, kind, text };
  toasts = [...toasts, t];
  toastNotify();
  setTimeout(() => {
    toasts = toasts.filter((x) => x.id !== t.id);
    toastNotify();
  }, 6500);
}

export function useToasts(): Toast[] {
  const [ts, setTs] = useState<Toast[]>(toasts);
  useEffect(() => {
    toastListeners.add(setTs);
    return () => {
      toastListeners.delete(setTs);
    };
  }, []);
  return ts;
}