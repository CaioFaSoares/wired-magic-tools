import { useEffect } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Navigate, Route, Routes, useLocation } from "react-router-dom";
import { Waypoints, RefreshCw } from "lucide-react";
import { AuthProvider, useAuth } from "./auth/AuthProvider";
import { ToastProvider } from "./components/Toast";
import { Shell } from "./components/Shell";
import { Button, Spinner } from "./components/ui";
import { ApiError } from "./lib/api";
import { LoginPage } from "./pages/LoginPage";
import { OverviewPage } from "./pages/OverviewPage";
import { NodesPage } from "./pages/NodesPage";
import { TokensPage } from "./pages/TokensPage";
import { AclPage } from "./pages/AclPage";
import { RelayPage } from "./pages/RelayPage";
import { IntegrationPage } from "./pages/IntegrationPage";
import { AuditPage } from "./pages/AuditPage";
import { SettingsPage } from "./pages/SettingsPage";
import { ExtensionsPage } from "./pages/ExtensionsPage";
import { ExtensionDetailPage } from "./pages/ExtensionDetailPage";
import { CloudflarePage } from "./pages/CloudflarePage";
import { NotFoundPage } from "./pages/NotFoundPage";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => {
        if (error instanceof ApiError) {
          if (error.status === 0 || error.status >= 500) return failureCount < 1;
          return false;
        }
        return failureCount < 1;
      },
      refetchOnWindowFocus: true,
      refetchOnReconnect: true,
      staleTime: 5_000,
    },
    mutations: {
      retry: false,
    },
  },
});

function ScrollToTop() {
  const location = useLocation();
  useEffect(() => {
    window.scrollTo({ top: 0, behavior: "instant" });
  }, [location.pathname]);
  return null;
}

function DocumentTitle() {
  const location = useLocation();
  useEffect(() => {
    const titles: Record<string, string> = {
      "/overview": "Visão geral",
      "/nodes": "Nós",
      "/tokens": "Tokens de entrada",
      "/acl": "ACL",
      "/relay-status": "Relay e STUN",
      "/integration": "Integração",
      "/audit": "Auditoria",
      "/settings": "Configurações",
      "/extensions": "Extensões",
      "/extensions/cloudflared": "Cloudflare Tunnel",
      "/login": "Entrar",
    };
    const title = titles[location.pathname] ?? "Painel";
    document.title = `${title} · wired-magic`;
  }, [location.pathname]);
  return null;
}

function BootScreen({
  title,
  description,
  error = false,
  onRetry,
}: {
  title: string;
  description: string;
  error?: boolean;
  onRetry?: () => void;
}) {
  return (
    <div className="flex min-h-screen flex-col items-center justify-center gap-4 bg-bg px-4 text-center">
      <span className={`flex size-10 items-center justify-center rounded-lg border ${error ? "border-danger/30 bg-danger/10 text-danger" : "border-accent/30 bg-accent/10 text-accent"}`}>
        <Waypoints aria-hidden="true" className="size-5" />
      </span>
      <div>
        <h1 className="text-sm font-semibold text-ink">{title}</h1>
        <p className="mt-1 max-w-sm text-xs leading-5 text-muted">{description}</p>
      </div>
      {onRetry ? (
        <Button onClick={onRetry}>
          <RefreshCw aria-hidden="true" className="size-3.5" />
          Tentar novamente
        </Button>
      ) : (
        <Spinner />
      )}
      <a href="/login" className="text-[11px] text-faint hover:text-muted">
        Ir para o login
      </a>
    </div>
  );
}

function ProtectedLayout() {
  const { status, bootError, retry } = useAuth();
  const location = useLocation();

  if (status === "checking") {
    return <BootScreen title="Verificando sessão…" description="Consultando o control plane e o cookie de sessão." />;
  }

  if (status === "error") {
    return (
      <BootScreen
        error
        title="Control plane inacessível"
        description={bootError ?? "Não foi possível verificar a sessão. Verifique a rede e o processo do control."}
        onRetry={retry}
      />
    );
  }

  if (status === "anon") {
    return <Navigate to="/login" replace state={{ from: `${location.pathname}${location.search}` }} />;
  }

  return <Shell />;
}

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <ToastProvider>
        <BrowserRouter>
          <AuthProvider>
            <ScrollToTop />
            <DocumentTitle />
            <Routes>
              <Route path="/login" element={<LoginPage />} />
              <Route element={<ProtectedLayout />}>
                <Route index element={<Navigate to="/overview" replace />} />
                <Route path="/overview" element={<OverviewPage />} />
                <Route path="/nodes" element={<NodesPage />} />
                <Route path="/tokens" element={<TokensPage />} />
                <Route path="/acl" element={<AclPage />} />
                <Route path="/relay-status" element={<RelayPage />} />
                <Route path="/integration" element={<IntegrationPage />} />
                <Route path="/audit" element={<AuditPage />} />
                <Route path="/settings" element={<SettingsPage />} />
                <Route path="/extensions" element={<ExtensionsPage />} />
                <Route path="/extensions/cloudflared" element={<CloudflarePage />} />
                <Route path="/extensions/:extensionId" element={<ExtensionDetailPage />} />
                <Route path="*" element={<NotFoundPage />} />
              </Route>
            </Routes>
          </AuthProvider>
        </BrowserRouter>
      </ToastProvider>
    </QueryClientProvider>
  );
}
