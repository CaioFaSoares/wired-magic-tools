import { useEffect, useRef, useState, type ButtonHTMLAttributes, type ComponentPropsWithoutRef, type ReactNode } from "react";
import { Link, type LinkProps } from "react-router-dom";
import {
  AlertTriangle,
  Check,
  Copy,
  Inbox,
  Loader2,
  RefreshCw,
} from "lucide-react";
import { copyToClipboard } from "../lib/clipboard";
import { errorMessage } from "../lib/errors";
import { useToast } from "./Toast";

export function cn(...classes: Array<string | false | null | undefined>): string {
  return classes.filter(Boolean).join(" ");
}

// ---------------------------------------------------------------------------
// Botões
// ---------------------------------------------------------------------------

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "subtle";
type ButtonSize = "sm" | "md";

const buttonVariants: Record<ButtonVariant, string> = {
  primary:
    "border-accent bg-accent text-[#04211d] hover:border-accent-strong hover:bg-accent-strong active:translate-y-px font-semibold",
  secondary:
    "border-line-strong bg-panel-2 text-ink hover:border-line-strong hover:bg-white/[0.06]",
  ghost: "border-transparent bg-transparent text-muted hover:bg-white/[0.05] hover:text-ink",
  danger:
    "border-danger/40 bg-danger/10 text-danger hover:border-danger/60 hover:bg-danger/20",
  subtle: "border-line bg-transparent text-muted hover:bg-white/[0.04] hover:text-ink",
};

const buttonSizes: Record<ButtonSize, string> = {
  sm: "h-8 gap-1.5 rounded-md px-2.5 text-xs",
  md: "h-9 gap-2 rounded-md px-3.5 text-sm",
};

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
}

export function Button({
  variant = "secondary",
  size = "md",
  loading = false,
  className,
  children,
  disabled,
  type = "button",
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      className={cn(
        "inline-flex shrink-0 items-center justify-center border font-medium transition-colors disabled:opacity-45",
        buttonVariants[variant],
        buttonSizes[size],
        className,
      )}
      {...props}
    >
      {loading ? <Loader2 aria-hidden="true" className="size-3.5 animate-spin" /> : null}
      {children}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Cartões e cabeçalhos
// ---------------------------------------------------------------------------

export function Card({
  className,
  children,
  ...props
}: ComponentPropsWithoutRef<"section">) {
  return (
    <section className={cn("panel", className)} {...props}>
      {children}
    </section>
  );
}

interface CardHeaderProps {
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  icon?: ReactNode;
  className?: string;
}

export function CardHeader({ title, description, actions, icon, className }: CardHeaderProps) {
  return (
    <div className={cn("flex flex-wrap items-start justify-between gap-3 border-b border-line px-4 py-3", className)}>
      <div className="min-w-0">
        <h2 className="flex items-center gap-2 text-sm font-semibold text-ink">
          {icon ? <span className="text-accent">{icon}</span> : null}
          {title}
        </h2>
        {description ? <p className="mt-0.5 text-xs leading-5 text-muted">{description}</p> : null}
      </div>
      {actions ? <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div> : null}
    </div>
  );
}

interface PageHeaderProps {
  title: string;
  description?: ReactNode;
  eyebrow?: string;
  actions?: ReactNode;
}

export function PageHeader({ title, description, eyebrow, actions }: PageHeaderProps) {
  return (
    <div className="mb-5 flex flex-wrap items-end justify-between gap-3">
      <div className="min-w-0">
        {eyebrow ? (
          <p className="mb-1 font-mono text-[10px] uppercase tracking-[0.22em] text-accent/80">{eyebrow}</p>
        ) : null}
        <h1 className="text-xl font-semibold tracking-tight text-ink sm:text-2xl">{title}</h1>
        {description ? <p className="mt-1 max-w-3xl text-sm leading-6 text-muted">{description}</p> : null}
      </div>
      {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Badges, status, indicadores
// ---------------------------------------------------------------------------

export type BadgeTone = "neutral" | "ok" | "warn" | "danger" | "info" | "accent";

const badgeTones: Record<BadgeTone, string> = {
  neutral: "border-line-strong bg-white/[0.04] text-muted",
  ok: "border-ok/30 bg-ok/10 text-ok",
  warn: "border-warn/30 bg-warn/10 text-warn",
  danger: "border-danger/30 bg-danger/10 text-danger",
  info: "border-info/30 bg-info/10 text-info",
  accent: "border-accent/30 bg-accent/10 text-accent",
};

const dotTones: Record<BadgeTone, string> = {
  neutral: "bg-faint",
  ok: "bg-ok",
  warn: "bg-warn",
  danger: "bg-danger",
  info: "bg-info",
  accent: "bg-accent",
};

export function Badge({
  tone = "neutral",
  dot = false,
  children,
  className,
  title,
}: {
  tone?: BadgeTone;
  dot?: boolean;
  children: ReactNode;
  className?: string;
  title?: string;
}) {
  return (
    <span
      title={title}
      className={cn(
        "inline-flex items-center gap-1.5 whitespace-nowrap rounded border px-1.5 py-0.5 text-[11px] font-medium leading-4",
        badgeTones[tone],
        className,
      )}
    >
      {dot ? <span aria-hidden="true" className={cn("size-1.5 rounded-full", dotTones[tone])} /> : null}
      {children}
    </span>
  );
}

export type DotTone = "ok" | "warn" | "danger" | "muted" | "accent";

const statusDotTones: Record<DotTone, string> = {
  ok: "bg-ok",
  warn: "bg-warn",
  danger: "bg-danger",
  muted: "bg-faint",
  accent: "bg-accent",
};

export function StatusDot({ tone, pulse = false, className }: { tone: DotTone; pulse?: boolean; className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn("inline-block size-1.5 shrink-0 rounded-full", statusDotTones[tone], pulse && "live-pulse", className)}
    />
  );
}

export function Spinner({ className }: { className?: string }) {
  return <Loader2 aria-hidden="true" className={cn("size-4 animate-spin text-accent", className)} />;
}

export function InlineCode({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <code className={cn("rounded border border-line bg-black/30 px-1 py-0.5 font-mono text-[11px] text-ink", className)}>
      {children}
    </code>
  );
}

// ---------------------------------------------------------------------------
// Estados de dados
// ---------------------------------------------------------------------------

export function LoadingPanel({ label = "Carregando…", rows = 3 }: { label?: string; rows?: number }) {
  return (
    <div role="status" aria-live="polite" className="panel p-5">
      <div className="mb-4 flex items-center gap-2 text-sm text-muted">
        <Spinner />
        <span>{label}</span>
      </div>
      <div className="space-y-2.5" aria-hidden="true">
        {Array.from({ length: rows }, (_, index) => (
          <div
            key={index}
            className="h-7 animate-pulse rounded bg-white/[0.04]"
            style={{ width: `${100 - index * 9}%` }}
          />
        ))}
      </div>
    </div>
  );
}

export function ErrorState({
  error,
  onRetry,
  title = "Não foi possível carregar",
}: {
  error: unknown;
  onRetry?: () => void;
  title?: string;
}) {
  return (
    <div role="alert" className="panel border-danger/25 p-5">
      <div className="flex items-start gap-3">
        <AlertTriangle aria-hidden="true" className="mt-0.5 size-5 shrink-0 text-danger" />
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold text-ink">{title}</h2>
          <p className="mt-1 text-sm leading-6 text-muted">{errorMessage(error)}</p>
          {onRetry ? (
            <Button size="sm" className="mt-3" onClick={onRetry}>
              <RefreshCw aria-hidden="true" className="size-3.5" />
              Tentar novamente
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

export function EmptyState({
  title,
  description,
  action,
  icon,
}: {
  title: string;
  description?: ReactNode;
  action?: ReactNode;
  icon?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-line-strong/80 bg-white/[0.015] px-5 py-10 text-center">
      <div className="mb-3 flex size-9 items-center justify-center rounded-lg border border-line bg-panel-2 text-faint">
        {icon ?? <Inbox aria-hidden="true" className="size-4" />}
      </div>
      <p className="text-sm font-medium text-ink">{title}</p>
      {description ? <p className="mt-1 max-w-md text-xs leading-5 text-muted">{description}</p> : null}
      {action ? <div className="mt-4">{action}</div> : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Formulários
// ---------------------------------------------------------------------------

interface FieldProps {
  label: string;
  htmlFor: string;
  hint?: ReactNode;
  error?: ReactNode;
  children: ReactNode;
  className?: string;
  optional?: boolean;
}

export function Field({ label, htmlFor, hint, error, children, className, optional }: FieldProps) {
  return (
    <div className={cn("space-y-1.5", className)}>
      <div className="flex items-baseline justify-between gap-2">
        <label htmlFor={htmlFor} className="text-xs font-medium text-ink">
          {label}
        </label>
        {optional ? <span className="text-[10px] uppercase tracking-wider text-faint">opcional</span> : null}
      </div>
      {children}
      {hint && !error ? (
        <p id={`${htmlFor}-hint`} className="text-[11px] leading-4 text-faint">
          {hint}
        </p>
      ) : null}
      {error ? (
        <p id={`${htmlFor}-error`} role="alert" className="text-[11px] leading-4 text-danger">
          {error}
        </p>
      ) : null}
    </div>
  );
}

export function Input({ className, ...props }: ComponentPropsWithoutRef<"input">) {
  return <input className={cn("field-input", className)} {...props} />;
}

export function Textarea({ className, ...props }: ComponentPropsWithoutRef<"textarea">) {
  return <textarea className={cn("field-input min-h-24 resize-y", className)} {...props} />;
}

export function Select({ className, children, ...props }: ComponentPropsWithoutRef<"select">) {
  return (
    <select className={cn("field-input", className)} {...props}>
      {children}
    </select>
  );
}

// ---------------------------------------------------------------------------
// Cópia
// ---------------------------------------------------------------------------

interface CopyButtonProps {
  text: string;
  label?: string;
  copiedLabel?: string;
  variant?: ButtonVariant;
  size?: ButtonSize;
  className?: string;
  disabled?: boolean;
}

export function CopyButton({
  text,
  label = "Copiar",
  copiedLabel = "Copiado",
  variant = "subtle",
  size = "sm",
  className,
  disabled,
}: CopyButtonProps) {
  const iconOnly = label.length === 0;
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  const { push } = useToast();

  useEffect(() => {
    return () => {
      if (timer.current !== undefined) window.clearTimeout(timer.current);
    };
  }, []);

  async function handleCopy() {
    try {
      await copyToClipboard(text);
      setCopied(true);
      if (timer.current !== undefined) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(false), 1800);
    } catch {
      push({ tone: "error", title: "Não foi possível copiar", description: "Selecione o texto manualmente." });
    }
  }

  return (
    <Button
      type="button"
      variant={variant}
      size={size}
      onClick={handleCopy}
      disabled={disabled || !text}
      aria-label={copied ? copiedLabel || "Copiado" : label || "Copiar"}
      title={iconOnly ? (copied ? "Copiado" : "Copiar") : undefined}
      className={className}
    >
      {copied ? <Check aria-hidden="true" className="size-3.5 text-ok" /> : <Copy aria-hidden="true" className="size-3.5" />}
      {!iconOnly ? <span>{copied ? copiedLabel : label}</span> : null}
    </Button>
  );
}

// ---------------------------------------------------------------------------
// Métricas
// ---------------------------------------------------------------------------

type StatTone = "default" | "warn" | "danger" | "ok" | "accent";

const statToneClasses: Record<StatTone, string> = {
  default: "text-ink",
  warn: "text-warn",
  danger: "text-danger",
  ok: "text-ok",
  accent: "text-accent",
};

export function StatCard({
  label,
  value,
  hint,
  icon,
  tone = "default",
}: {
  label: string;
  value: ReactNode;
  hint?: ReactNode;
  icon?: ReactNode;
  tone?: StatTone;
}) {
  return (
    <div className="panel p-3.5">
      <div className="flex items-start justify-between gap-2">
        <p className="text-[10px] font-medium uppercase tracking-[0.14em] text-faint">{label}</p>
        {icon ? <span className="text-faint">{icon}</span> : null}
      </div>
      <p className={cn("mt-1.5 text-2xl font-semibold tabular-nums tracking-tight", statToneClasses[tone])}>
        {value}
      </p>
      {hint ? <div className="mt-1 text-[11px] leading-4 text-muted">{hint}</div> : null}
    </div>
  );
}

export function PageToastError({ error }: { error: unknown }) {
  return (
    <p role="alert" className="text-xs leading-5 text-danger">
      {errorMessage(error)}
    </p>
  );
}


export function LinkButton({
  variant = "secondary",
  size = "sm",
  className,
  children,
  ...props
}: LinkProps & { variant?: ButtonVariant; size?: ButtonSize }) {
  return (
    <Link
      className={cn(
        "inline-flex shrink-0 items-center justify-center border font-medium transition-colors",
        buttonVariants[variant],
        buttonSizes[size],
        className,
      )}
      {...props}
    >
      {children}
    </Link>
  );
}
