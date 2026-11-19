import { useEffect, useRef, useState } from "react";
import { NavLink, Outlet, useLocation, useNavigate } from "react-router-dom";
import {
  Cable,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Menu,
  Plug,
  Puzzle,
  RadioTower,
  ScrollText,
  Server,
  Settings,
  ShieldCheck,
  Waypoints,
  X,
  type LucideIcon,
} from "lucide-react";
import { useAuth } from "../auth/AuthProvider";
import { isMockMode } from "../lib/env";
import { useMeta, useOverview } from "../lib/queries";
import { formatRelative } from "../lib/format";
import { useControlEvents, type LiveStatus } from "../lib/useControlEvents";
import { Button, cn, Spinner, StatusDot } from "./ui";
import { useToast } from "./Toast";

interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
}

interface NavGroup {
  label: string;
  items: NavItem[];
}

const navGroups: NavGroup[] = [
  {
    label: "Rede",
    items: [
      { to: "/overview", label: "Visão geral", icon: LayoutDashboard },
      { to: "/nodes", label: "Nós", icon: Server },
      { to: "/tokens", label: "Tokens de entrada", icon: KeyRound },
    ],
  },
  {
    label: "Política",
    items: [
      { to: "/acl", label: "ACL", icon: ShieldCheck },
      { to: "/relay-status", label: "Relay e STUN", icon: RadioTower },
      { to: "/integration", label: "Integração", icon: Plug },
    ],
  },
  {
    label: "Plataforma",
    items: [
      { to: "/extensions", label: "Extensões", icon: Puzzle },
    ],
  },
  {
    label: "Sistema",
    items: [
      { to: "/audit", label: "Auditoria", icon: ScrollText },
      { to: "/settings", label: "Configurações", icon: Settings },
    ],
  },
];

const pageTitles: Record<string, { section: string; title: string }> = {
  "/overview": { section: "Rede", title: "Visão geral" },
  "/nodes": { section: "Rede", title: "Nós" },
  "/tokens": { section: "Rede", title: "Tokens de entrada" },
  "/acl": { section: "Política", title: "ACL" },
  "/relay-status": { section: "Política", title: "Relay e STUN" },
  "/integration": { section: "Política", title: "Integração" },
  "/audit": { section: "Sistema", title: "Auditoria" },
  "/settings": { section: "Sistema", title: "Configurações" },
  "/extensions": { section: "Plataforma", title: "Extensões" },
  "/extensions/cloudflared": { section: "Plataforma", title: "Cloudflare Tunnel" },
};

const liveCopy: Record<LiveStatus, { label: string; tone: "ok" | "warn" | "danger" | "muted" | "accent"; pulse: boolean }> = {
  open: { label: "ao vivo", tone: "ok", pulse: true },
  connecting: { label: "conectando", tone: "warn", pulse: false },
  reconnecting: { label: "reconectando", tone: "warn", pulse: false },
  mock: { label: "mock", tone: "accent", pulse: false },
  disabled: { label: "sem SSE", tone: "muted", pulse: false },
};

function Brand() {
  return (
    <div className="flex items-center gap-2.5">
      <span className="flex size-8 items-center justify-center rounded-lg border border-accent/30 bg-accent/10 text-accent">
        <Waypoints aria-hidden="true" className="size-4" />
      </span>
      <span className="min-w-0">
        <span className="block font-mono text-[13px] font-semibold tracking-tight text-ink">wired-magic</span>
        <span className="block text-[10px] uppercase tracking-[0.18em] text-faint">control panel</span>
      </span>
    </div>
  );
}

function NavContent({ pendingCount, onNavigate }: { pendingCount: number; onNavigate?: () => void }) {
  return (
    <nav aria-label="Navegação principal" className="flex-1 overflow-y-auto px-2.5 py-3">
      {navGroups.map((group) => (
        <div key={group.label} className="mb-4">
          <p className="mb-1.5 px-2 font-mono text-[9px] uppercase tracking-[0.2em] text-faint">{group.label}</p>
          <ul className="space-y-0.5">
            {group.items.map((item) => {
              const Icon = item.icon;
              return (
                <li key={item.to}>
                  <NavLink
                    to={item.to}
                    onClick={onNavigate}
                    className={({ isActive }) =>
                      cn(
                        "group flex items-center gap-2.5 rounded-md px-2 py-1.5 text-[13px] transition-colors",
                        isActive
                          ? "border border-accent/20 bg-accent/[0.08] text-accent"
                          : "border border-transparent text-muted hover:bg-white/[0.04] hover:text-ink",
                      )
                    }
                  >
                    {({ isActive }) => (
                      <>
                        <Icon aria-hidden="true" className={cn("size-4 shrink-0", isActive ? "text-accent" : "text-faint group-hover:text-muted")} />
                        <span className="min-w-0 flex-1 truncate">{item.label}</span>
                        {item.to === "/nodes" && pendingCount > 0 ? (
                          <span
                            title={`${pendingCount} nó(s) aguardando aprovação`}
                            className="rounded border border-warn/30 bg-warn/10 px-1 text-[10px] font-medium leading-4 text-warn"
                          >
                            {pendingCount}
                          </span>
                        ) : null}
                      </>
                    )}
                  </NavLink>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </nav>
  );
}

export function Shell() {
  const { user, csrfReady, csrfInvalid, logout } = useAuth();
  const metaQuery = useMeta();
  const overviewQuery = useOverview();
  const live = useControlEvents(true);
  const location = useLocation();
  const navigate = useNavigate();
  const { push } = useToast();
  const [menuOpen, setMenuOpen] = useState(false);
  const asideRef = useRef<HTMLElement>(null);

  const page = pageTitles[location.pathname] ?? { section: "Sistema", title: "Painel" };
  const pendingCount = overviewQuery.data?.nodes_pending ?? 0;
  const liveInfo = liveCopy[live.status];
  const controlOnline = metaQuery.isSuccess;
  const network = metaQuery.data?.network;

  useEffect(() => {
    setMenuOpen(false);
  }, [location.pathname]);

  useEffect(() => {
    if (!menuOpen) return;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") setMenuOpen(false);
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.body.style.overflow = previousOverflow;
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [menuOpen]);

  async function handleLogout() {
    const remote = await logout();
    if (remote) {
      push({ tone: "ok", title: "Sessão encerrada" });
    } else {
      push({
        tone: "info",
        title: "Sessão local encerrada",
        description: "O servidor não confirmou o logout. Se necessário, entre novamente.",
      });
    }
  }

  const statusWarning = !csrfReady || csrfInvalid;

  return (
    <div className="flex min-h-screen">
      <a
        href="#main-content"
        className="sr-only z-[80] rounded bg-accent px-3 py-2 text-sm font-semibold text-[#04211d] focus:not-sr-only focus:fixed focus:left-3 focus:top-3"
      >
        Pular para o conteúdo
      </a>

      {menuOpen ? (
        <button
          type="button"
          aria-label="Fechar menu"
          onClick={() => setMenuOpen(false)}
          className="fixed inset-0 z-30 bg-black/70 backdrop-blur-sm lg:hidden"
        />
      ) : null}

      <aside
        ref={asideRef}
        id="sidebar"
        aria-label="Barra lateral"
        className={cn(
          "fixed inset-y-0 left-0 z-40 flex w-60 shrink-0 flex-col border-r border-line bg-[#080e14]/95 backdrop-blur-md transition-transform duration-200",
          "lg:sticky lg:top-0 lg:z-auto lg:h-screen lg:translate-x-0 lg:self-start lg:bg-[#080e14]",
          menuOpen ? "translate-x-0" : "-translate-x-full",
        )}
      >
        <div className="flex h-14 items-center justify-between border-b border-line px-3.5">
          <Brand />
          <button
            type="button"
            onClick={() => setMenuOpen(false)}
            aria-label="Fechar menu"
            className="rounded p-1.5 text-faint hover:bg-white/5 hover:text-ink lg:hidden"
          >
            <X aria-hidden="true" className="size-4" />
          </button>
        </div>

        <NavContent pendingCount={pendingCount} onNavigate={() => setMenuOpen(false)} />

        <div className="border-t border-line px-3 py-3">
          <div className="rounded-lg border border-line bg-white/[0.02] px-2.5 py-2">
            <div className="flex items-center gap-2 text-xs">
              <StatusDot
                tone={metaQuery.isPending ? "warn" : controlOnline ? "ok" : "danger"}
                pulse={controlOnline}
              />
              <span className="font-medium text-ink">
                {metaQuery.isPending ? "control verificando" : controlOnline ? "control online" : "control inacessível"}
              </span>
            </div>
            <div className="mt-1 pl-3.5 font-mono text-[10px] leading-4 text-faint">
              {network ? `${network.name} · ${network.overlay_cidr}` : metaQuery.isError ? "sem resposta de /meta" : "—"}
            </div>
          </div>

          <div className="mt-2.5 flex items-center gap-2">
            <span className="flex size-7 shrink-0 items-center justify-center rounded-full border border-line-strong bg-panel-2 font-mono text-[10px] font-semibold uppercase text-muted">
              {user?.username.slice(0, 2) ?? "?"}
            </span>
            <div className="min-w-0 flex-1">
              <p className="truncate text-xs font-medium text-ink">{user?.username ?? "—"}</p>
              <p className="text-[10px] text-faint">administrador</p>
            </div>
            <button
              type="button"
              onClick={handleLogout}
              title="Sair"
              aria-label="Sair"
              className="rounded p-1.5 text-faint transition-colors hover:bg-white/5 hover:text-danger"
            >
              <LogOut aria-hidden="true" className="size-3.5" />
            </button>
          </div>

          <p className="mt-2.5 flex items-center justify-between font-mono text-[9px] text-faint">
            <span>{metaQuery.data ? `v${metaQuery.data.version} · proto ${metaQuery.data.protocol}` : "—"}</span>
            {isMockMode ? <span className="text-accent">mock</span> : null}
          </p>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="sticky top-0 z-20 flex h-14 items-center gap-3 border-b border-line bg-bg/85 px-3 backdrop-blur-md sm:px-5">
          <button
            type="button"
            onClick={() => setMenuOpen(true)}
            aria-label="Abrir menu"
            aria-expanded={menuOpen}
            aria-controls="sidebar"
            className="rounded p-1.5 text-muted hover:bg-white/5 hover:text-ink lg:hidden"
          >
            <Menu aria-hidden="true" className="size-5" />
          </button>

          <div className="min-w-0">
            <p className="hidden text-[10px] uppercase tracking-[0.16em] text-faint sm:block">{page.section}</p>
            <p className="truncate text-sm font-medium text-ink">{page.title}</p>
          </div>

          <div className="ml-auto flex items-center gap-2 sm:gap-3">
            <span className="hidden items-center gap-1.5 rounded border border-line bg-panel/70 px-2 py-1 font-mono text-[10px] text-muted xl:flex">
              <Cable aria-hidden="true" className="size-3 text-faint" />
              {network?.overlay_cidr ?? "—"}
            </span>

            <span
              className="flex items-center gap-1.5 text-[11px] text-muted"
              title={live.lastEventAt ? `último evento ${formatRelative(new Date(live.lastEventAt).toISOString())}` : "stream de eventos do control"}
            >
              <StatusDot tone={liveInfo.tone} pulse={liveInfo.pulse} />
              <span>{liveInfo.label}</span>
            </span>

            <span className="hidden items-center gap-1.5 text-[11px] text-muted sm:flex lg:hidden">
              <StatusDot tone={metaQuery.isPending ? "warn" : controlOnline ? "ok" : "danger"} />
              <span className="hidden sm:inline">{controlOnline ? "control ok" : "control offline"}</span>
            </span>

            <div className="hidden items-center gap-1.5 border-l border-line pl-3 md:flex">
              <span className="flex size-6 items-center justify-center rounded-full border border-line-strong bg-panel-2 font-mono text-[9px] font-semibold uppercase text-muted">
                {user?.username.slice(0, 2) ?? "?"}
              </span>
            </div>

            <Button
              variant="ghost"
              size="sm"
              onClick={handleLogout}
              className="px-2 sm:px-3"
              title="Sair"
              aria-label="Sair"
            >
              <LogOut aria-hidden="true" className="size-3.5" />
              <span className="hidden sm:inline">Sair</span>
            </Button>
          </div>
        </header>

        <main id="main-content" className="flex-1 px-3 py-4 sm:px-5 sm:py-5 lg:px-7">
          <div className="mx-auto w-full max-w-[1450px]">
            {statusWarning ? (
              <div
                role="alert"
                className="mb-4 flex flex-wrap items-center gap-3 rounded-lg border border-warn/30 bg-warn/[0.07] px-3.5 py-2.5"
              >
                <ShieldCheck aria-hidden="true" className="size-4 shrink-0 text-warn" />
                <div className="min-w-0 flex-1 text-xs leading-5 text-muted">
                  <span className="font-medium text-warn">
                    {csrfInvalid ? "A proteção CSRF recusou a última ação." : "Esta aba não tem o token CSRF."}
                  </span>{" "}
                  Leitura funciona normalmente; para alterar algo, entre novamente.
                </div>
                <Button size="sm" variant="subtle" onClick={handleLogout}>
                  Entrar novamente
                </Button>
              </div>
            ) : null}

            {metaQuery.isPending && !overviewQuery.data ? (
              <div className="mb-4 flex items-center gap-2 text-xs text-faint">
                <Spinner className="size-3" />
                conectando ao control plane…
              </div>
            ) : null}

            <Outlet />
          </div>
        </main>

        <footer className="border-t border-line px-3 py-3 text-[10px] text-faint sm:px-5">
          <div className="mx-auto flex w-full max-w-[1450px] flex-wrap items-center justify-between gap-2">
            <span className="font-mono">
              wired-magic {metaQuery.data ? `v${metaQuery.data.version}` : ""} · protocolo{" "}
              {metaQuery.data?.protocol ?? "—"} ·{" "}
              {network?.name ?? "rede não informada"}
            </span>
            <button
              type="button"
              onClick={() => navigate("/settings")}
              className="inline-flex items-center gap-1 hover:text-muted"
            >
              Detalhes do control <span aria-hidden="true">→</span>
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}
