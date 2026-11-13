import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { API_BASE, isMockMode } from "./env";
import { qk } from "./queries";

export type LiveStatus = "mock" | "connecting" | "open" | "reconnecting" | "disabled";

interface ControlEventEnvelope {
  type?: unknown;
  data?: unknown;
}

function eventType(value: unknown): string | null {
  if (typeof value !== "object" || value === null) return null;
  const type = (value as ControlEventEnvelope).type;
  return typeof type === "string" ? type : null;
}

/**
 * Assina o SSE de admin. O backend emite eventos nomeados (`event: wm`), por
 * isso escutamos os dois canais. Cada evento invalida apenas as queries que
 * dependem dele; EventSource reconecta sozinho, mas recriamos a conexão quando
 * o servidor fecha (ex.: sessão expirada).
 */
export function useControlEvents(enabled = true): { status: LiveStatus; lastEventAt: number | null } {
  const queryClient = useQueryClient();
  const [status, setStatus] = useState<LiveStatus>("connecting");
  const [lastEventAt, setLastEventAt] = useState<number | null>(null);

  useEffect(() => {
    if (!enabled) {
      setStatus("disabled");
      return;
    }
    if (isMockMode) {
      setStatus("mock");
      return;
    }

    let stopped = false;
    let source: EventSource | null = null;
    let retryTimer: number | undefined;
    let attempts = 0;

    const handleEvent = (event: MessageEvent<string>) => {
      let payload: unknown;
      try {
        payload = JSON.parse(event.data) as unknown;
      } catch {
        return;
      }
      const type = eventType(payload);
      if (!type) return;
      setLastEventAt(Date.now());
      switch (type) {
        case "node_updated":
        case "node_removed":
        case "presence":
          void queryClient.invalidateQueries({ queryKey: qk.nodes });
          void queryClient.invalidateQueries({ queryKey: qk.overview });
          break;
        case "join_token_created":
        case "join_token_revoked":
          void queryClient.invalidateQueries({ queryKey: qk.joinTokens });
          break;
        case "acl_updated":
          void queryClient.invalidateQueries({ queryKey: qk.acl });
          break;
        case "relay_updated":
          void queryClient.invalidateQueries({ queryKey: qk.overview });
          void queryClient.invalidateQueries({ queryKey: qk.nodes });
          void queryClient.invalidateQueries({ queryKey: qk.settings });
          void queryClient.invalidateQueries({ queryKey: qk.meta });
          break;
        case "audit":
          void queryClient.invalidateQueries({ queryKey: ["audit"] });
          break;
        case "extension_status":
          void queryClient.invalidateQueries({ queryKey: ["extensions"] });
          void queryClient.invalidateQueries({ queryKey: ["extension"] });
          void queryClient.invalidateQueries({ queryKey: ["cloudflared"] });
          break;
        default:
          break;
      }
    };

    const scheduleReconnect = () => {
      if (stopped) return;
      setStatus("reconnecting");
      const delay = Math.min(30_000, 2000 * 2 ** attempts) + Math.floor(Math.random() * 500);
      attempts += 1;
      retryTimer = window.setTimeout(connect, delay);
    };

    const connect = () => {
      if (stopped) return;
      setStatus(attempts === 0 ? "connecting" : "reconnecting");
      source = new EventSource(`${API_BASE}/admin/events`, { withCredentials: true });
      source.onopen = () => {
        attempts = 0;
        setStatus("open");
      };
      source.addEventListener("wm", handleEvent as EventListener);
      source.addEventListener("message", handleEvent as EventListener);
      source.onerror = () => {
        if (stopped) return;
        setStatus("reconnecting");
        if (source && source.readyState === EventSource.CLOSED) {
          source.close();
          source = null;
          scheduleReconnect();
        }
      };
    };

    connect();

    return () => {
      stopped = true;
      if (retryTimer !== undefined) window.clearTimeout(retryTimer);
      source?.close();
    };
  }, [enabled, queryClient]);

  return { status, lastEventAt };
}
