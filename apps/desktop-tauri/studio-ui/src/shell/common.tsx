// Shared primitives for the desktop shell panels: a toast host (replacing the
// former imperative `toast()` helper) and a small status line.

import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";

export type StatusKind = "" | "ok" | "err";

export interface StatusState {
  text: string;
  kind: StatusKind;
}

export const emptyStatus: StatusState = { text: "", kind: "" };

/** Inline status line (muted / ok / err), matching the shell's `.status`. */
export function Status({ status }: { status: StatusState }) {
  return <span className={"status " + status.kind}>{status.text}</span>;
}

type ToastFn = (message: string, kind?: StatusKind) => void;

const ToastContext = createContext<ToastFn>(() => {});

/** Hook returning `toast(message, kind?)` bound to the app-level toast host. */
export const useToast = (): ToastFn => useContext(ToastContext);

/**
 * Provides a single bottom-center toast host shared by every shell panel. The
 * toast auto-hides after 3.2s, matching the former vanilla shell behaviour.
 */
export function ToastProvider({ children }: { children: ReactNode }) {
  const [toast, setToast] = useState<{ message: string; kind: StatusKind } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const show = useCallback<ToastFn>((message, kind = "") => {
    setToast({ message, kind });
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setToast(null), 3200);
  }, []);
  return (
    <ToastContext.Provider value={show}>
      {children}
      <div className={"toast" + (toast ? " show " + toast.kind : "")}>{toast?.message ?? ""}</div>
    </ToastContext.Provider>
  );
}
