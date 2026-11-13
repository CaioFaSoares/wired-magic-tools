import type { NatType, NodeStatus, NodeView, PathKind, PeerStats } from "./types";

export const nodeStatusLabel: Record<NodeStatus, string> = {
  active: "ativo",
  pending: "pendente",
  disabled: "revogado",
};

export const natLabel: Record<NatType, string> = {
  unknown: "desconhecido",
  cone: "cone",
  symmetric: "simétrico",
};

export const pathLabel: Record<PathKind, string> = {
  none: "sem caminho",
  direct: "direto",
  relay: "relay",
};

export const endpointKindLabel = {
  stun: "STUN",
  lan: "LAN",
  previous: "anterior",
  manual: "manual",
} as const;

/**
 * O overview conta presença via relay, mas `NodeView.online` vem apenas do
 * last_seen. Para não divergir, a UI considera online quem também está
 * conectado ao relay.
 */
export function isNodeOnline(node: NodeView): boolean {
  return node.online || node.relay_connected;
}

export function isNodeOnlineViaRelay(node: NodeView): boolean {
  return !node.online && node.relay_connected;
}

export interface NodePathSummary {
  direct: number;
  relay: number;
  none: number;
  minDirectRtt: number | null;
  minRelayRtt: number | null;
  errors: PeerStats[];
}

export function summarizePaths(peers: PeerStats[]): NodePathSummary {
  const summary: NodePathSummary = {
    direct: 0,
    relay: 0,
    none: 0,
    minDirectRtt: null,
    minRelayRtt: null,
    errors: [],
  };
  for (const peer of peers) {
    if (peer.path === "direct") {
      summary.direct += 1;
      if (peer.rtt_ms != null) {
        summary.minDirectRtt =
          summary.minDirectRtt == null ? peer.rtt_ms : Math.min(summary.minDirectRtt, peer.rtt_ms);
      }
    } else if (peer.path === "relay") {
      summary.relay += 1;
      if (peer.rtt_ms != null) {
        summary.minRelayRtt =
          summary.minRelayRtt == null ? peer.rtt_ms : Math.min(summary.minRelayRtt, peer.rtt_ms);
      }
    } else {
      summary.none += 1;
    }
    if (peer.last_error) summary.errors.push(peer);
  }
  return summary;
}

export function pathHeadline(summary: NodePathSummary): {
  kind: "direct" | "relay" | "none" | "mixed";
  label: string;
} {
  if (summary.direct > 0 && summary.relay > 0) {
    return { kind: "mixed", label: "misto" };
  }
  if (summary.direct > 0) return { kind: "direct", label: "direto" };
  if (summary.relay > 0) return { kind: "relay", label: "relay" };
  return { kind: "none", label: "sem relatório" };
}

/** Caminho predominante usado pela topologia. */
export function topologyPath(node: NodeView): "direct" | "relay" | "unknown" {
  const summary = summarizePaths(node.reported_peers);
  if (summary.direct > 0) return "direct";
  if (summary.relay > 0 || node.relay_connected) return "relay";
  return "unknown";
}

export function sortNodesByAttention(nodes: NodeView[]): NodeView[] {
  const statusWeight: Record<NodeStatus, number> = { pending: 0, active: 1, disabled: 2 };
  return [...nodes].sort((a, b) => {
    const status = statusWeight[a.status] - statusWeight[b.status];
    if (status !== 0) return status;
    const online = Number(isNodeOnline(b)) - Number(isNodeOnline(a));
    if (online !== 0) return online;
    return a.name.localeCompare(b.name, "pt-BR");
  });
}
