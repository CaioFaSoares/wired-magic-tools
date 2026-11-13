import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { AlertTriangle, CheckCircle2, Info, X } from "lucide-react";

type ToastTone = "ok" | "error" | "info";

interface ToastInput {
  title: string;
  description?: string;
  tone?: ToastTone;
  durationMs?: number;
}

interface ToastItem extends Required<Pick<ToastInput, "title" | "tone">> {
  id: number;
  description?: string;
}

interface ToastContextValue {
  push: (toast: ToastInput) => void;
  dismiss: (id: number) => void;
}


const ToastContext = createContext<ToastContextValue | null>(null);

const toneClasses: Record<ToastTone, string> = {
  ok: "border-ok/30 bg-[#0d1b16] text-ink",
  error: "border-danger/40 bg-[#1c1012] text-ink",
  info: "border-line-strong bg-panel-2 text-ink",
};

const toneIconClasses: Record<ToastTone, string> = {
  ok: "text-ok",
  error: "text-danger",
  info: "text-info",
};

function ToneIcon({ tone }: { tone: ToastTone }) {
  if (tone === "ok") return <CheckCircle2 aria-hidden="true" className="size-4" />;
  if (tone === "error") return <AlertTriangle aria-hidden="true" className="size-4" />;
  return <Info aria-hidden="true" className="size-4" />;
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const nextId = useRef(1);
  const timers = useRef(new Map<number, number>());

  const dismiss = useCallback((id: number) => {
    const timer = timers.current.get(id);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      timers.current.delete(id);
    }
    setItems((current) => current.filter((item) => item.id !== id));
  }, []);

  const push = useCallback(
    (toast: ToastInput) => {
      const id = nextId.current++;
      const tone = toast.tone ?? "info";
      const item: ToastItem = {
        id,
        title: toast.title,
        description: toast.description,
        tone,
      };
      setItems((current) => [...current.slice(-3), item]);
      const timer = window.setTimeout(() => dismiss(id), toast.durationMs ?? (tone === "error" ? 7000 : 4500));
      timers.current.set(id, timer);
    },
    [dismiss],
  );

  useEffect(() => {
    const activeTimers = timers.current;
    return () => {
      for (const timer of activeTimers.values()) window.clearTimeout(timer);
      activeTimers.clear();
    };
  }, []);

  const value = useMemo<ToastContextValue>(() => ({ push, dismiss }), [push, dismiss]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div
        aria-live="polite"
        aria-atomic="false"
        className="pointer-events-none fixed inset-x-0 bottom-0 z-[70] flex flex-col items-end gap-2 p-4 sm:p-6"
      >
        {items.map((item) => (
          <div
            key={item.id}
            role={item.tone === "error" ? "alert" : "status"}
            className={`pointer-events-auto flex w-full max-w-sm items-start gap-3 rounded-lg border px-3.5 py-3 shadow-2xl shadow-black/40 ${toneClasses[item.tone]}`}
          >
            <span className={`mt-0.5 shrink-0 ${toneIconClasses[item.tone]}`}>
              <ToneIcon tone={item.tone} />
            </span>
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium leading-5">{item.title}</p>
              {item.description ? (
                <p className="mt-0.5 text-xs leading-5 text-muted">{item.description}</p>
              ) : null}
            </div>
            <button
              type="button"
              onClick={() => dismiss(item.id)}
              aria-label="Fechar notificação"
              className="mt-0.5 rounded p-1 text-faint transition-colors hover:bg-white/5 hover:text-ink"
            >
              <X aria-hidden="true" className="size-3.5" />
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): ToastContextValue {
  const context = useContext(ToastContext);
  if (!context) throw new Error("useToast precisa estar dentro de ToastProvider");
  return context;
}
