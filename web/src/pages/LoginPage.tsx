import { useMemo, useState, type FormEvent } from "react";
import { useLocation, useNavigate, Navigate } from "react-router-dom";
import {
  Activity,
  Eye,
  EyeOff,
  KeyRound,
  Lock,
  Network,
  RefreshCw,
  ShieldCheck,
  Waypoints,
} from "lucide-react";
import { useAuth } from "../auth/AuthProvider";
import { errorMessage } from "../lib/errors";
import { isMockMode } from "../lib/env";
import { useMeta } from "../lib/queries";
import { Badge, Button, Field, Input, Spinner, StatusDot } from "../components/ui";

interface LocationState {
  from?: string;
}

function safeRedirect(value: string | undefined): string {
  if (!value || !value.startsWith("/") || value.startsWith("//") || value.startsWith("/login")) {
    return "/overview";
  }
  return value;
}

export function LoginPage() {
  const { status, login, retry, bootError } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const metaQuery = useMeta();
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [forceForm, setForceForm] = useState(false);

  const from = safeRedirect((location.state as LocationState | null)?.from);
  const controlLabel = useMemo(() => {
    if (metaQuery.isPending) return "verificando control…";
    if (metaQuery.isError) return "control inacessível";
    return `${metaQuery.data.network.name} · protocolo ${metaQuery.data.protocol}`;
  }, [metaQuery.isPending, metaQuery.isError, metaQuery.data]);

  if (status === "checking") {
    return (
      <div className="flex min-h-screen items-center justify-center bg-bg">
        <div className="flex items-center gap-3 text-sm text-muted">
          <Spinner />
          Verificando sessão…
        </div>
      </div>
    );
  }

  if (status === "authed") {
    return <Navigate to={from} replace />;
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    if (!username.trim() || !password) {
      setError("Informe usuário e senha.");
      return;
    }
    setSubmitting(true);
    try {
      await login(username.trim(), password);
      navigate(from, { replace: true });
    } catch (submitError) {
      setError(errorMessage(submitError));
      setPassword("");
    } finally {
      setSubmitting(false);
    }
  }

  const showBootError = status === "error" && !forceForm;

  return (
    <div className="relative min-h-screen overflow-hidden">
      <div aria-hidden="true" className="grid-backdrop pointer-events-none absolute inset-0 opacity-60" />
      <div className="relative mx-auto grid min-h-screen w-full max-w-6xl grid-cols-1 items-center gap-8 px-4 py-8 sm:px-6 lg:grid-cols-[minmax(0,1fr)_420px] lg:gap-16 lg:py-12">
        <section className="hidden min-w-0 lg:block">
          <div className="mb-8 flex items-center gap-2.5">
            <span className="flex size-9 items-center justify-center rounded-lg border border-accent/30 bg-accent/10 text-accent">
              <Waypoints aria-hidden="true" className="size-5" />
            </span>
            <span>
              <span className="block font-mono text-sm font-semibold text-ink">wired-magic</span>
              <span className="block text-[10px] uppercase tracking-[0.2em] text-faint">self-hosted mesh vpn</span>
            </span>
          </div>

          <p className="mb-3 font-mono text-[10px] uppercase tracking-[0.24em] text-accent/80">
            control plane · operação
          </p>
          <h1 className="max-w-xl text-4xl font-semibold leading-[1.1] tracking-tight text-ink">
            A malha sob
            <br />
            controle.
          </h1>
          <p className="mt-4 max-w-lg text-sm leading-6 text-muted">
            Gerencie nós, tokens de entrada, ACL e o caminho direto ou via relay da sua malha
            WireGuard. O control plane é a fonte da verdade; o tráfego nunca passa por aqui.
          </p>

          <dl className="mt-8 grid max-w-lg grid-cols-2 gap-2.5">
            {[
              { icon: Network, title: "Nós e overlay", text: "presença, endpoint, NAT e RTT reportado" },
              { icon: ShieldCheck, title: "ACL versionada", text: "validação, compilação e simulação" },
              { icon: KeyRound, title: "Convites", text: "tokens de uso único com TTL e tags" },
              { icon: Activity, title: "Auditoria viva", text: "eventos SSE e histórico do operador" },
            ].map(({ icon: Icon, title, text }) => (
              <div key={title} className="rounded-lg border border-line bg-panel/60 px-3 py-2.5">
                <dt className="flex items-center gap-2 text-xs font-medium text-ink">
                  <Icon aria-hidden="true" className="size-3.5 text-accent" />
                  {title}
                </dt>
                <dd className="mt-1 pl-[22px] text-[11px] leading-4 text-faint">{text}</dd>
              </div>
            ))}
          </dl>

          <div className="mt-8 flex flex-wrap items-center gap-x-4 gap-y-2 text-[11px] text-faint">
            <span className="inline-flex items-center gap-1.5">
              <StatusDot tone={metaQuery.isSuccess ? "ok" : metaQuery.isPending ? "warn" : "danger"} pulse={metaQuery.isSuccess} />
              {controlLabel}
            </span>
            <span className="font-mono">{metaQuery.data ? `v${metaQuery.data.version}` : "—"}</span>
            <span className="font-mono">{metaQuery.data?.network.overlay_cidr ?? "—"}</span>
          </div>
        </section>

        <section className="mx-auto w-full max-w-md">
          <div className="mb-6 flex items-center gap-2.5 lg:hidden">
            <span className="flex size-8 items-center justify-center rounded-lg border border-accent/30 bg-accent/10 text-accent">
              <Waypoints aria-hidden="true" className="size-4" />
            </span>
            <span>
              <span className="block font-mono text-[13px] font-semibold text-ink">wired-magic</span>
              <span className="block text-[10px] uppercase tracking-[0.18em] text-faint">control panel</span>
            </span>
          </div>

          <div className="panel border-line-strong bg-panel/95 p-5 shadow-2xl shadow-black/40 sm:p-6">
            <div className="mb-5">
              <div className="mb-2 flex items-center justify-between gap-2">
                <h2 className="text-lg font-semibold tracking-tight text-ink">Entrar no painel</h2>
                {isMockMode ? <Badge tone="accent">mock</Badge> : <Badge tone="neutral">operador</Badge>}
              </div>
              <p className="text-xs leading-5 text-muted">
                Sessão de administrador do control plane. Não há login de aluno neste painel.
              </p>
            </div>

            {showBootError ? (
              <div role="alert" className="mb-4 rounded-lg border border-danger/30 bg-danger/[0.07] p-3">
                <p className="text-xs font-medium text-danger">Falha ao verificar a sessão</p>
                <p className="mt-1 text-xs leading-5 text-muted">{bootError ?? "O control plane não respondeu."}</p>
                <div className="mt-2.5 flex flex-wrap gap-2">
                  <Button size="sm" onClick={retry}>
                    <RefreshCw aria-hidden="true" className="size-3.5" />
                    Tentar novamente
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setForceForm(true)}>
                    Usar o formulário
                  </Button>
                </div>
              </div>
            ) : null}

            <form onSubmit={handleSubmit} className="space-y-3.5" noValidate>
              <Field label="Usuário" htmlFor="login-username">
                <Input
                  id="login-username"
                  name="username"
                  type="text"
                  autoComplete="username"
                  autoCapitalize="none"
                  spellCheck={false}
                  value={username}
                  onChange={(event) => setUsername(event.target.value)}
                  aria-invalid={Boolean(error) || undefined}
                  disabled={submitting}
                  autoFocus
                />
              </Field>

              <Field label="Senha" htmlFor="login-password">
                <div className="relative">
                  <Input
                    id="login-password"
                    name="password"
                    type={showPassword ? "text" : "password"}
                    autoComplete="current-password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                    aria-invalid={Boolean(error) || undefined}
                    disabled={submitting}
                    className="pr-10"
                  />
                  <button
                    type="button"
                    onClick={() => setShowPassword((current) => !current)}
                    aria-label={showPassword ? "Ocultar senha" : "Mostrar senha"}
                    aria-pressed={showPassword}
                    className="absolute right-1 top-1/2 -translate-y-1/2 rounded p-1.5 text-faint hover:bg-white/5 hover:text-ink"
                  >
                    {showPassword ? <EyeOff aria-hidden="true" className="size-4" /> : <Eye aria-hidden="true" className="size-4" />}
                  </button>
                </div>
              </Field>

              {error ? (
                <div role="alert" className="rounded-md border border-danger/30 bg-danger/[0.07] px-3 py-2 text-xs leading-5 text-danger">
                  {error}
                </div>
              ) : null}

              {isMockMode ? (
                <p className="rounded-md border border-accent/20 bg-accent/[0.05] px-3 py-2 text-[11px] leading-4 text-muted">
                  Mock ativo para desenvolvimento. Use <span className="font-mono text-accent">admin</span> /{" "}
                  <span className="font-mono text-accent">admin</span>.
                </p>
              ) : null}

              <Button type="submit" variant="primary" className="w-full" loading={submitting}>
                <Lock aria-hidden="true" className="size-4" />
                {submitting ? "Entrando…" : "Entrar"}
              </Button>
            </form>

            <div className="mt-5 border-t border-line pt-3.5">
              <p className="flex items-start gap-2 text-[11px] leading-4 text-faint">
                <ShieldCheck aria-hidden="true" className="mt-0.5 size-3.5 shrink-0 text-faint" />
                A sessão usa cookie HttpOnly com SameSite=Lax. O token CSRF fica somente nesta aba, em
                sessionStorage.
              </p>
              <div className="mt-3 flex flex-wrap items-center justify-between gap-2 font-mono text-[10px] text-faint">
                <span className="inline-flex items-center gap-1.5">
                  <StatusDot tone={metaQuery.isSuccess ? "ok" : metaQuery.isPending ? "warn" : "danger"} />
                  {controlLabel}
                </span>
                <span>{metaQuery.data?.network.dns_zone ?? "wm.internal"}</span>
              </div>
            </div>
          </div>

          <p className="mt-4 text-center text-[10px] leading-4 text-faint">
            wired-magic · malha self-hosted · acesso restrito ao operador
          </p>
        </section>
      </div>
    </div>
  );
}
