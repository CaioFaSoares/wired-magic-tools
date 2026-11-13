import { Link } from "react-router-dom";
import {
  Activity,
  ArrowLeftRight,
  CheckCircle2,
  RadioTower,
  RefreshCw,
  Server,
  Siren,
  AlertTriangle,
} from "lucide-react";
import { useMeta, useNodes, useOverview, useSettings } from "../lib/queries";
import { formatDateTime, formatRelative } from "../lib/format";
import {
  isNodeOnline,
  isNodeOnlineViaRelay,
  natLabel,
  summarizePaths,
} from "../lib/nodes";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  ErrorState,
  LinkButton,
  LoadingPanel,
  PageHeader,
  StatCard,
} from "../components/ui";
import { MeshTopology } from "../components/MeshTopology";
import { OnlineBadge } from "../components/badges";

export function RelayPage() {
  const settingsQuery = useSettings();
  const metaQuery = useMeta();
  const overviewQuery = useOverview();
  const nodesQuery = useNodes();

  const header = (
    <PageHeader
      eyebrow="Política"
      title="Relay e STUN"
      description="Fallback de pacotes WireGuard na 443 e descoberta de endpoints. O relay encaminha ciphertext, nunca plaintext."
      actions={
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            void settingsQuery.refetch();
            void overviewQuery.refetch();
            void nodesQuery.refetch();
            void metaQuery.refetch();
          }}
          loading={settingsQuery.isFetching || overviewQuery.isFetching || nodesQuery.isFetching}
        >
          <RefreshCw aria-hidden="true" className="size-3.5" />
          Atualizar
        </Button>
      }
    />
  );

  if (overviewQuery.isLoading || nodesQuery.isLoading) {
    return (
      <>
        {header}
        <LoadingPanel label="Lendo relay, STUN e caminhos…" rows={4} />
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
            void settingsQuery.refetch();
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
  const relayUrl = settings?.relay_url ?? meta?.relay ?? null;
  const stunUrls = settings?.stun ?? meta?.stun ?? [];
  const relayNodes = nodes.filter((node) => node.relay_connected);
  const activeNodes = nodes.filter((node) => node.status === "active");
  const errorNodes = nodes
    .map((node) => ({ node, summary: summarizePaths(node.reported_peers) }))
    .filter(({ summary }) => summary.errors.length > 0);

  return (
    <>
      {header}

      <div className="mb-4 grid grid-cols-1 gap-2.5 sm:grid-cols-2 xl:grid-cols-4">
        <StatCard
          label="Relay WSS"
          value={relayKnown ? (relayEnabled ? "habilitado" : "desligado") : "indisponível"}
          tone={relayEnabled ? "ok" : relayKnown ? "warn" : "default"}
          hint={
            relayEnabled && relayUrl ? (
              <span className="break-all font-mono text-[10px]">{relayUrl}</span>
            ) : relayKnown ? (
              "sem fallback WSS configurado"
            ) : (
              "settings/meta não responderam"
            )
          }
          icon={<RadioTower aria-hidden="true" className="size-4" />}
        />
        <StatCard
          label="Conexões no relay"
          value={overview.relay_connected}
          hint={`${relayNodes.length} nó(s) reportam relay_connected`}
          tone={overview.relay_connected > 0 ? "accent" : "default"}
          icon={<Activity aria-hidden="true" className="size-4" />}
        />
        <StatCard
          label="Pares diretos"
          value={overview.direct_pairs}
          hint="caminho preferido, sem relay"
          tone="ok"
          icon={<ArrowLeftRight aria-hidden="true" className="size-4" />}
        />
        <StatCard
          label="Pares via relay"
          value={overview.relay_pairs}
          hint="fallback quando o direto falha"
          tone={overview.relay_pairs > 0 ? "warn" : "default"}
          icon={<Siren aria-hidden="true" className="size-4" />}
        />
      </div>

      {!relayEnabled && relayKnown ? (
        <div role="status" className="mb-4 flex items-start gap-2.5 rounded-lg border border-warn/30 bg-warn/[0.06] px-3.5 py-3">
          <AlertTriangle aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warn" />
          <div className="text-xs leading-5 text-muted">
            <p className="font-medium text-warn">Relay desabilitado na configuração</p>
            <p className="mt-0.5">
              A malha continua funcionando com caminhos diretos. Nós atrás de NAT simétrico ou com UDP
              bloqueado não terão fallback até o relay ser habilitado no servidor.
            </p>
          </div>
        </div>
      ) : null}

      <div className="mb-4 grid grid-cols-1 gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader
            title="Topologia e caminhos"
            description="Traço cheio = direto reportado; tracejado âmbar = via relay; pontilhado = sem relatório recente."
            actions={<Badge tone={relayEnabled ? "ok" : "warn"}>{relayEnabled ? "relay on" : "relay off"}</Badge>}
          />
          <div className="px-3 pb-2 pt-1 sm:px-4">
            <MeshTopology nodes={nodes} relayEnabled={relayEnabled} maxNodes={12} caption="Topologia do relay" />
          </div>
        </Card>

        <Card>
          <CardHeader title="STUN" description="Endereços anunciados aos agentes para descoberta de endpoint." icon={<Server aria-hidden="true" className="size-4" />} />
          <div className="space-y-2 p-4">
            {stunUrls.length === 0 ? (
              <p className="text-xs text-muted">Nenhum STUN configurado.</p>
            ) : (
              <ul className="space-y-1.5">
                {stunUrls.map((url) => (
                  <li key={url} className="flex items-center gap-2 rounded-md border border-line bg-black/20 px-2.5 py-1.5">
                    <span aria-hidden="true" className="size-1.5 rounded-full bg-accent" />
                    <code className="break-all font-mono text-[11px] text-muted">{url}</code>
                  </li>
                ))}
              </ul>
            )}
            <p className="border-t border-line pt-2.5 text-[10px] leading-4 text-faint">
              A API informa a configuração anunciada; não existe endpoint de health de STUN. A
              descoberta efetiva é reportada pelos agentes via NAT/endpoints.
            </p>
          </div>
        </Card>
      </div>

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader
            title="Nós e caminhos reportados"
            description="Relatórios enviados por cada agente; valores podem ficar defasados se o nó estiver offline."
            actions={<LinkButton to="/nodes" variant="ghost" size="sm">Abrir nós</LinkButton>}
          />
          {activeNodes.length === 0 ? (
            <div className="p-4 text-xs text-muted">Nenhum nó ativo. Aprove um nó para ver caminhos.</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="data-table min-w-[680px]">
                <caption>Nós ativos e caminhos reportados</caption>
                <thead>
                  <tr>
                    <th scope="col">Nó</th>
                    <th scope="col">Presença</th>
                    <th scope="col">NAT</th>
                    <th scope="col">Relay</th>
                    <th scope="col">Diretos</th>
                    <th scope="col">Via relay</th>
                  </tr>
                </thead>
                <tbody>
                  {activeNodes.map((node) => {
                    const summary = summarizePaths(node.reported_peers);
                    return (
                      <tr key={node.id}>
                        <td className="max-w-[200px]">
                          <Link
                            to={`/nodes?q=${encodeURIComponent(node.name)}`}
                            className="block truncate text-[13px] font-medium text-ink hover:text-accent"
                          >
                            {node.name}
                          </Link>
                          <span className="font-mono text-[10px] text-faint">{node.overlay_ip}</span>
                          {summary.errors.length > 0 ? (
                            <span
                              className="mt-0.5 block text-[10px] text-danger"
                              title={summary.errors.map((error) => error.last_error).join("\n")}
                            >
                              {summary.errors.length} erro(s) de peer
                            </span>
                          ) : null}
                        </td>
                        <td>
                          <OnlineBadge online={isNodeOnline(node)} viaRelay={isNodeOnlineViaRelay(node)} />
                          <span className="mt-0.5 block text-[10px] text-faint" title={formatDateTime(node.last_seen)}>
                            {formatRelative(node.last_seen)}
                          </span>
                        </td>
                        <td className="text-xs text-muted">{natLabel[node.nat_type]}</td>
                        <td>
                          <Badge tone={node.relay_connected ? "accent" : "neutral"}>
                            {node.relay_connected ? "conectado" : "não"}
                          </Badge>
                        </td>
                        <td className="font-mono text-[11px] text-muted">{summary.direct}</td>
                        <td className="font-mono text-[11px] text-muted">{summary.relay}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </Card>

        <div className="space-y-4">
          <Card>
            <CardHeader
              title="Como o fallback funciona"
              description="O relay só entra quando o caminho direto não fecha."
              icon={<RadioTower aria-hidden="true" className="size-4" />}
            />
            <ol className="list-inside list-decimal space-y-2 px-4 py-3 text-[11px] leading-5 text-muted marker:text-faint">
              <li>
                STUN descobre o endpoint público; o control distribui candidatos de endpoint e NAT.
              </li>
              <li>
                Os agentes tentam o caminho direto primeiro. Se o handshake não completa, conectam ao
                relay WSS em <code className="text-accent">/relay</code>.
              </li>
              <li>
                O relay encaminha datagramas WireGuard entre UUIDs ativos. Não decifra o payload e
                não é caminho primário.
              </li>
              <li>
                O token de relay é curto e assinado pelo control; um nó revogado deixa de receber
                token e os peers removem a chave no próximo mapa.
              </li>
            </ol>
          </Card>

          <Card>
            <CardHeader title="Status observado" icon={<Activity aria-hidden="true" className="size-4" />} />
            <dl className="divide-y divide-line/70 px-4 text-xs">
              <div className="flex items-center justify-between gap-3 py-2.5">
                <dt className="text-faint">Configuração</dt>
                <dd>
                  <Badge tone={relayEnabled ? "ok" : "warn"} dot>
                    {relayKnown ? (relayEnabled ? "habilitado" : "desligado") : "indisponível"}
                  </Badge>
                </dd>
              </div>
              <div className="flex items-center justify-between gap-3 py-2.5">
                <dt className="text-faint">Conexões WSS</dt>
                <dd className="font-mono text-ink">{overview.relay_connected}</dd>
              </div>
              <div className="flex items-center justify-between gap-3 py-2.5">
                <dt className="text-faint">Nós com relay</dt>
                <dd className="font-mono text-ink">{relayNodes.length}</dd>
              </div>
              <div className="flex items-center justify-between gap-3 py-2.5">
                <dt className="text-faint">NAT simétrico</dt>
                <dd className="font-mono text-ink">
                  {nodes.filter((node) => node.nat_type === "symmetric").length}
                </dd>
              </div>
              <div className="flex items-center justify-between gap-3 py-2.5">
                <dt className="text-faint">Erros de peer</dt>
                <dd className="font-mono text-ink">{errorNodes.reduce((sum, item) => sum + item.summary.errors.length, 0)}</dd>
              </div>
              <div className="flex items-center justify-between gap-3 py-2.5">
                <dt className="text-faint">Protocolo</dt>
                <dd className="font-mono text-ink">{meta?.protocol ?? settings?.protocol ?? "—"}</dd>
              </div>
            </dl>
          </Card>

          {relayUrl ? (
            <div className="rounded-lg border border-line bg-white/[0.02] px-3.5 py-3">
              <p className="flex items-center gap-1.5 text-[11px] font-medium text-ink">
                <CheckCircle2 aria-hidden="true" className="size-3.5 text-accent" />
                URL do relay
              </p>
              <code className="mt-1 block break-all font-mono text-[10px] leading-4 text-muted">{relayUrl}</code>
              <p className="mt-1.5 text-[10px] leading-4 text-faint">
                Definida pelo control (<span className="font-mono">relay_url</span> ou{" "}
                <span className="font-mono">public_url</span> + <span className="font-mono">/relay</span>).
              </p>
            </div>
          ) : null}
        </div>
      </div>
    </>
  );
}
