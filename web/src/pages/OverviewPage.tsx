import { Link } from "react-router-dom";
import {
  Activity,
  AlertTriangle,
  ArrowLeftRight,
  ArrowUpRight,
  CheckCircle2,
  Clock,
  KeyRound,
  RadioTower,
  RefreshCw,
  Server,
  Wifi,
} from "lucide-react";
import { PageHeader, StatCard, Button, Card, CardHeader, LinkButton, ErrorState, LoadingPanel, StatusDot, CopyButton } from "../components/ui";
import { Badge } from "../components/ui";
import { MeshTopology } from "../components/MeshTopology";
import { NodeStatusBadge, OnlineBadge, PathBadge } from "../components/badges";
import { useMeta, useNodes, useOverview, useSettings } from "../lib/queries";
import {
  formatDuration,
  formatNumber,
  formatRelative,
  formatRtt,
  formatUptimeSince,
  shortenHash,
} from "../lib/format";
import {
  isNodeOnline,
  isNodeOnlineViaRelay,
  pathHeadline,
  sortNodesByAttention,
  summarizePaths,
} from "../lib/nodes";

interface AlertItem {
  id: string;
  tone: "warn" | "danger" | "info";
  title: string;
  description: string;
  to?: string;
  action?: string;
}

const alertToneClasses = {
  warn: "border-warn/25 bg-warn/[0.06]",
  danger: "border-danger/25 bg-danger/[0.06]",
  info: "border-line bg-white/[0.02]",
};

const alertTextClasses = {
  warn: "text-warn",
  danger: "text-danger",
  info: "text-info",
};

export function OverviewPage() {
  const overviewQuery = useOverview();
  const nodesQuery = useNodes();
  const settingsQuery = useSettings();
  const metaQuery = useMeta();

  const header = (
    <PageHeader
      eyebrow="Rede"
      title="Visão geral"
      description="Estado atual da malha, presença dos nós e caminhos reportados entre peers."
      actions={
        <>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              void overviewQuery.refetch();
              void nodesQuery.refetch();
              void settingsQuery.refetch();
            }}
            loading={overviewQuery.isFetching || nodesQuery.isFetching}
          >
            <RefreshCw aria-hidden="true" className="size-3.5" />
            Atualizar
          </Button>
          <LinkButton to="/tokens" variant="primary" size="sm">
            <KeyRound aria-hidden="true" className="size-3.5" />
            Criar convite
          </LinkButton>
        </>
      }
    />
  );

  if (overviewQuery.isLoading || nodesQuery.isLoading) {
    return (
      <>
        {header}
        <LoadingPanel label="Lendo o estado da malha…" rows={4} />
      </>
    );
  }

  if (overviewQuery.isError || nodesQuery.isError || !overviewQuery.data || !nodesQuery.data) {
    return (
      <>
        {header}
        <ErrorState
          error={overviewQuery.error ?? nodesQuery.error}
          onRetry={() => {
            void overviewQuery.refetch();
            void nodesQuery.refetch();
          }}
        />
      </>
    );
  }

  const overview = overviewQuery.data;
  const nodes = nodesQuery.data;
  const settings = settingsQuery.data;
  const meta = metaQuery.data;

  const relayKnown = settings != null || meta != null;
  const relayEnabled = settings ? Boolean(settings.relay_url) : meta ? Boolean(meta.relay) : false;
  const onlinePct = overview.nodes_total > 0 ? Math.round((overview.nodes_online / overview.nodes_total) * 100) : 0;
  const sortedNodes = sortNodesByAttention(nodes);
  const visibleNodes = sortedNodes.slice(0, 6);
  const activeOffline = nodes.filter((node) => node.status === "active" && !isNodeOnline(node)).length;

  const alerts: AlertItem[] = [];
  if (overview.nodes_pending > 0) {
    alerts.push({
      id: "pending",
      tone: "warn",
      title: `${overview.nodes_pending} ${overview.nodes_pending === 1 ? "nó pendente" : "nós pendentes"} de aprovação`,
      description: "Um nó pendente não recebe mapa nem entra na malha até ser aprovado.",
      to: "/nodes?status=pending",
      action: "Revisar nós",
    });
  }
  if (relayKnown && !relayEnabled) {
    alerts.push({
      id: "relay-off",
      tone: "warn",
      title: "Relay WSS desabilitado",
      description: "Sem relay, peers atrás de NAT restritivo ou com UDP bloqueado não têm fallback.",
      to: "/relay-status",
      action: "Ver relay",
    });
  }
  if (relayEnabled && overview.relay_pairs > 0 && overview.relay_connected === 0) {
    alerts.push({
      id: "relay-stale",
      tone: "danger",
      title: "Pares via relay sem nó conectado",
      description:
        "Há peers reportando caminho relay, mas nenhuma conexão WSS ativa. Os relatórios podem estar defasados.",
      to: "/relay-status",
      action: "Diagnosticar",
    });
  }
  if (activeOffline > 0) {
    alerts.push({
      id: "offline",
      tone: "info",
      title: `${activeOffline} ${activeOffline === 1 ? "nó ativo offline" : "nós ativos offline"}`,
      description: "Offline é esperado para máquinas desligadas; verifique last_seen antes de agir.",
      to: "/nodes?status=active",
      action: "Abrir nós",
    });
  }

  const nodesWithErrors = nodes
    .map((node) => ({ node, summary: summarizePaths(node.reported_peers) }))
    .filter(({ summary }) => summary.errors.length > 0);
  if (nodesWithErrors.length > 0) {
    const sample = nodesWithErrors
      .slice(0, 2)
      .map(({ node, summary }) => `${node.name}: ${summary.errors[0]?.last_error ?? "erro reportado"}`)
      .join(" · ");
    alerts.push({
      id: "peer-errors",
      tone: "danger",
      title: `${nodesWithErrors.length} ${nodesWithErrors.length === 1 ? "nó reporta" : "nós reportam"} erro de peer`,
      description: sample,
      to: "/nodes",
      action: "Inspecionar",
    });
  }

  return (
    <>
      {header}

      <div className="mb-4 grid grid-cols-2 gap-2.5 md:grid-cols-3 xl:grid-cols-6">
        <StatCard
          label="Nós"
          value={formatNumber(overview.nodes_total)}
          hint={`${formatNumber(overview.nodes_online)} online · ${formatNumber(overview.nodes_pending)} pendentes`}
          icon={<Server aria-hidden="true" className="size-4" />}
        />
        <StatCard
          label="Online"
          value={formatNumber(overview.nodes_online)}
          hint={`${onlinePct}% da malha`}
          tone={overview.nodes_online > 0 ? "ok" : "default"}
          icon={<Wifi aria-hidden="true" className="size-4" />}
        />
        <StatCard
          label="Pendentes"
          value={formatNumber(overview.nodes_pending)}
          hint={overview.nodes_pending > 0 ? "aguardando aprovação" : "nada na fila"}
          tone={overview.nodes_pending > 0 ? "warn" : "default"}
          icon={<Clock aria-hidden="true" className="size-4" />}
        />
        <StatCard
          label="Pares diretos"
          value={formatNumber(overview.direct_pairs)}
          hint="handshake direto reportado"
          tone="accent"
          icon={<ArrowLeftRight aria-hidden="true" className="size-4" />}
        />
        <StatCard
          label="Pares via relay"
          value={formatNumber(overview.relay_pairs)}
          hint={`${overview.relay_connected} ${overview.relay_connected === 1 ? "nó no relay" : "nós no relay"}`}
          tone={overview.relay_pairs > 0 ? "warn" : "default"}
          icon={<RadioTower aria-hidden="true" className="size-4" />}
        />
        <StatCard
          label="Uptime do control"
          value={formatDuration(overview.control_uptime_secs)}
          hint={`desde ${formatUptimeSince(overview.control_uptime_secs)}`}
          icon={<Activity aria-hidden="true" className="size-4" />}
        />
      </div>

      <div className="mb-4 grid grid-cols-1 gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader
            title="Topologia da malha"
            description="O traço de cada nó indica o caminho predominante relatado ao control plane."
            actions={<LinkButton to="/relay-status" variant="ghost" size="sm">Analisar relay</LinkButton>}
          />
          <div className="px-3 pb-2 pt-1 sm:px-4">
            <MeshTopology nodes={nodes} relayEnabled={relayEnabled} maxNodes={9} caption="Topologia da malha" />
          </div>
        </Card>

        <Card>
          <CardHeader
            title="Alertas"
            description="Pendências, relay e erros reportados pelos agentes."
            actions={<Badge tone={alerts.length > 0 ? "warn" : "ok"}>{alerts.length}</Badge>}
          />
          <div className="space-y-2 p-3">
            {alerts.length === 0 ? (
              <div className="flex items-start gap-2.5 rounded-lg border border-ok/20 bg-ok/[0.05] px-3 py-3">
                <CheckCircle2 aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-ok" />
                <div>
                  <p className="text-xs font-medium text-ink">Nenhum alerta ativo</p>
                  <p className="mt-0.5 text-[11px] leading-4 text-muted">
                    Sem pendências, relay desligado ou erros de peer no último estado.
                  </p>
                </div>
              </div>
            ) : (
              alerts.map((alert) => (
                <div
                  key={alert.id}
                  className={`rounded-lg border px-3 py-2.5 ${alertToneClasses[alert.tone]}`}
                >
                  <div className="flex items-start gap-2">
                    <AlertTriangle
                      aria-hidden="true"
                      className={`mt-0.5 size-3.5 shrink-0 ${alertTextClasses[alert.tone]}`}
                    />
                    <div className="min-w-0 flex-1">
                      <p className="text-xs font-medium text-ink">{alert.title}</p>
                      <p className="mt-0.5 text-[11px] leading-4 text-muted">{alert.description}</p>
                      {alert.to ? (
                        <Link
                          to={alert.to}
                          className="mt-1.5 inline-flex items-center gap-1 text-[11px] font-medium text-accent hover:underline"
                        >
                          {alert.action}
                          <ArrowUpRight aria-hidden="true" className="size-3" />
                        </Link>
                      ) : null}
                    </div>
                  </div>
                </div>
              ))
            )}
          </div>
        </Card>
      </div>

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader
            title="Nós"
            description={`${nodes.length} no total · ordenados por atenção e presença`}
            actions={<LinkButton to="/nodes" variant="secondary" size="sm">Gerenciar todos</LinkButton>}
          />
          {visibleNodes.length === 0 ? (
            <div className="p-4">
              <p className="rounded-lg border border-dashed border-line px-4 py-6 text-center text-xs text-muted">
                Nenhum nó registrado. Crie um token de entrada para começar.
              </p>
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="data-table min-w-[720px]">
                <caption>Resumo dos nós da malha</caption>
                <thead>
                  <tr>
                    <th scope="col">Nó</th>
                    <th scope="col">Status</th>
                    <th scope="col">Presença</th>
                    <th scope="col">Caminho / RTT</th>
                    <th scope="col">Visto</th>
                  </tr>
                </thead>
                <tbody>
                  {visibleNodes.map((node) => {
                    const summary = summarizePaths(node.reported_peers);
                    const headline = pathHeadline(summary);
                    const rtt = summary.minDirectRtt ?? summary.minRelayRtt;
                    return (
                      <tr key={node.id}>
                        <td className="max-w-[230px]">
                          <Link to={`/nodes?q=${encodeURIComponent(node.name)}`} className="group block">
                            <span className="block truncate text-[13px] font-medium text-ink group-hover:text-accent">
                              {node.name}
                            </span>
                            <span className="block truncate font-mono text-[10px] text-faint">
                              {node.overlay_ip} · {node.hostname}
                            </span>
                          </Link>
                        </td>
                        <td>
                          <NodeStatusBadge status={node.status} />
                        </td>
                        <td>
                          <OnlineBadge online={isNodeOnline(node)} viaRelay={isNodeOnlineViaRelay(node)} />
                        </td>
                        <td>
                          <div className="flex flex-wrap items-center gap-1.5">
                            <PathBadge kind={headline.kind} />
                            <span className="font-mono text-[11px] text-muted">
                              {rtt != null ? formatRtt(rtt) : `${summary.direct}d · ${summary.relay}r`}
                            </span>
                          </div>
                        </td>
                        <td className="whitespace-nowrap text-xs text-muted" title={node.last_seen ?? undefined}>
                          {formatRelative(node.last_seen)}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </Card>

        <Card>
          <CardHeader
            title="Control plane"
            description="Versão e parâmetros da rede."
            actions={
              <span className="inline-flex items-center gap-1.5 text-[11px] text-muted">
                <StatusDot tone={metaQuery.isSuccess ? "ok" : metaQuery.isError ? "danger" : "warn"} />
                {metaQuery.isSuccess ? "online" : metaQuery.isError ? "offline" : "…"}
              </span>
            }
          />
          <dl className="divide-y divide-line/70 px-4 text-xs">
            <div className="flex items-center justify-between gap-3 py-2.5">
              <dt className="text-faint">Versão</dt>
              <dd className="font-mono text-ink">{meta?.version ?? settings?.version ?? "—"}</dd>
            </div>
            <div className="flex items-center justify-between gap-3 py-2.5">
              <dt className="text-faint">Protocolo</dt>
              <dd className="font-mono text-ink">{meta?.protocol ?? settings?.protocol ?? "—"}</dd>
            </div>
            <div className="flex items-center justify-between gap-3 py-2.5">
              <dt className="text-faint">Rede</dt>
              <dd className="truncate font-mono text-ink">{overview.network.name}</dd>
            </div>
            <div className="flex items-center justify-between gap-3 py-2.5">
              <dt className="text-faint">CIDR do overlay</dt>
              <dd className="font-mono text-ink">{overview.network.overlay_cidr}</dd>
            </div>
            <div className="flex items-center justify-between gap-3 py-2.5">
              <dt className="text-faint">DNS</dt>
              <dd className="font-mono text-ink">{overview.network.dns_zone}</dd>
            </div>
            <div className="flex items-center justify-between gap-3 py-2.5">
              <dt className="text-faint">MTU</dt>
              <dd className="font-mono text-ink">{overview.network.mtu}</dd>
            </div>
            <div className="flex items-center justify-between gap-3 py-2.5">
              <dt className="text-faint">Admin</dt>
              <dd className="font-mono text-ink">{overview.admin.username}</dd>
            </div>
            {meta?.control_pubkey ? (
              <div className="flex items-center justify-between gap-2 py-2.5">
                <dt className="shrink-0 text-faint">Chave do control</dt>
                <dd className="flex min-w-0 items-center gap-1">
                  <span className="truncate font-mono text-[11px] text-ink" title={meta.control_pubkey}>
                    {shortenHash(meta.control_pubkey, 6, 4)}
                  </span>
                  <CopyButton text={meta.control_pubkey} label="" copiedLabel="" className="px-1.5" />
                </dd>
              </div>
            ) : null}
          </dl>
          <div className="border-t border-line px-4 py-2.5">
            <Link to="/settings" className="inline-flex items-center gap-1 text-[11px] font-medium text-accent hover:underline">
              Ver configurações completas
              <ArrowUpRight aria-hidden="true" className="size-3" />
            </Link>
          </div>
        </Card>
      </div>
    </>
  );
}
