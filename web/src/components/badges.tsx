import type { NodeStatus } from "../lib/types";
import { nodeStatusLabel } from "../lib/nodes";
import { Badge, StatusDot, type BadgeTone } from "./ui";

const statusTones: Record<NodeStatus, BadgeTone> = {
  active: "ok",
  pending: "warn",
  disabled: "danger",
};

export function NodeStatusBadge({ status }: { status: NodeStatus }) {
  return (
    <Badge tone={statusTones[status]} dot>
      {nodeStatusLabel[status]}
    </Badge>
  );
}

export function OnlineBadge({
  online,
  viaRelay = false,
}: {
  online: boolean;
  viaRelay?: boolean;
}) {
  if (online) {
    return (
      <span className="inline-flex items-center gap-1.5 whitespace-nowrap text-xs text-ok">
        <StatusDot tone="ok" pulse />
        online
      </span>
    );
  }
  if (viaRelay) {
    return (
      <span className="inline-flex items-center gap-1.5 whitespace-nowrap text-xs text-accent" title="Presença via relay WSS">
        <StatusDot tone="accent" />
        via relay
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1.5 whitespace-nowrap text-xs text-faint">
      <StatusDot tone="muted" />
      offline
    </span>
  );
}

export function PathBadge({ kind }: { kind: "direct" | "relay" | "none" | "mixed" }) {
  const map: Record<typeof kind, { tone: BadgeTone; label: string }> = {
    direct: { tone: "accent", label: "direto" },
    relay: { tone: "warn", label: "relay" },
    mixed: { tone: "info", label: "misto" },
    none: { tone: "neutral", label: "sem relatório" },
  };
  const entry = map[kind];
  return <Badge tone={entry.tone}>{entry.label}</Badge>;
}
