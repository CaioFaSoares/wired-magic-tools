import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  ApiError,
  api,
  getCsrfToken,
  loadStoredCsrf,
  setApiHandlers,
  setCsrfToken,
} from "../lib/api";
import { errorMessage } from "../lib/errors";
import type { AdminUser } from "../lib/types";

type AuthStatus = "checking" | "anon" | "authed" | "error";

interface AuthState {
  status: AuthStatus;
  user: AdminUser | null;
}

interface AuthContextValue {
  status: AuthStatus;
  user: AdminUser | null;
  csrfReady: boolean;
  csrfInvalid: boolean;
  bootError: string | null;
  login: (username: string, password: string) => Promise<void>;
  /** `true` quando o servidor confirmou o encerramento da sessão. */
  logout: () => Promise<boolean>;
  retry: () => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const [state, setState] = useState<AuthState>({ status: "checking", user: null });
  const [csrfReady, setCsrfReady] = useState(false);
  const [csrfInvalid, setCsrfInvalid] = useState(false);
  const [bootError, setBootError] = useState<string | null>(null);
  const [bootNonce, setBootNonce] = useState(0);

  useEffect(() => {
    return setApiHandlers({
      onUnauthorized: () => {
        setCsrfToken(null);
        setCsrfReady(false);
        setCsrfInvalid(false);
        setState({ status: "anon", user: null });
        queryClient.clear();
      },
      onCsrfError: () => setCsrfInvalid(true),
    });
  }, [queryClient]);

  useEffect(() => {
    let cancelled = false;
    const stored = loadStoredCsrf();
    setCsrfReady(Boolean(stored));
    setCsrfInvalid(false);
    setBootError(null);
    setState({ status: "checking", user: null });

    async function boot() {
      try {
        let user: AdminUser;
        try {
          const session = await api.session();
          if (cancelled) return;
          setCsrfToken(session.csrf_token);
          setCsrfReady(true);
          user = session.user;
        } catch (sessionError) {
          // Compatibilidade com um control plane anterior, que só expõe /admin/me.
          if (!(sessionError instanceof ApiError) || sessionError.status !== 404) throw sessionError;
          user = await api.me();
        }
        if (!cancelled) setState({ status: "authed", user });
      } catch (error) {
        if (cancelled) return;
        if (error instanceof ApiError && error.status === 401) {
          if (getCsrfToken()) {
            setCsrfToken(null);
            setCsrfReady(false);
          }
          setState({ status: "anon", user: null });
          return;
        }
        setBootError(errorMessage(error));
        setState({ status: "error", user: null });
      }
    }

    void boot();

    return () => {
      cancelled = true;
    };
  }, [bootNonce]);

  const login = useCallback(
    async (username: string, password: string) => {
      const response = await api.login({ username, password });
      setCsrfToken(response.csrf_token);
      setCsrfReady(true);
      setCsrfInvalid(false);
      setBootError(null);
      queryClient.clear();
      setState({ status: "authed", user: response.user });
    },
    [queryClient],
  );

  const logout = useCallback(async () => {
    const hadCsrf = Boolean(getCsrfToken());
    let remote = false;
    try {
      if (hadCsrf) {
        await api.logout();
        remote = true;
      }
    } catch {
      // Mesmo sem confirmação remota, limpamos o estado local.
    } finally {
      setCsrfToken(null);
      setCsrfReady(false);
      setCsrfInvalid(false);
      setState({ status: "anon", user: null });
      queryClient.clear();
    }
    return remote;
  }, [queryClient]);

  const retry = useCallback(() => {
    setBootNonce((current) => current + 1);
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({
      status: state.status,
      user: state.user,
      csrfReady,
      csrfInvalid,
      bootError,
      login,
      logout,
      retry,
    }),
    [state, csrfReady, csrfInvalid, bootError, login, logout, retry],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext);
  if (!context) throw new Error("useAuth precisa estar dentro de AuthProvider");
  return context;
}
