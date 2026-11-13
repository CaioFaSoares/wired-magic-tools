import { useEffect, useId, useMemo, useState, type FormEvent } from "react";
import { useSearchParams } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Pencil,
  RefreshCw,
  RotateCcw,
  Search,
  ServerOff,
  Trash2,
  X,
} from "lucide-react";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import {
  formatBytes,
  formatDateTime,
  formatDuration,
  formatRelative,
  formatRtt,
  normalizeSearch,
} from "../lib/format";
import {
  endpointKindLabel,
  isNodeOnline,
  isNodeOnlineViaRelay,
  natLabel,
  pathHeadline,
  pathLabel,
  sortNodesByAttention,
  summarizePaths,
} from "../lib/nodes";
import { invalidateNodeQueries, useNodes } from "../lib/queries";
import type { NodeStatus, NodeView } from "../lib/types";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  EmptyState,
  ErrorState,
  Field,
  Input,
  LinkButton,
  LoadingPanel,
  PageHeader,
  Textarea,
  cn,
} from "../components/ui";
import { NodeStatusBadge, OnlineBadge, PathBadge } from "../components/badges";
import { ConfirmDialog, Modal } from "../components/Modal";
import { useToast } from "../components/Toast";
import { useAuth } from "../auth/AuthProvider";

type StatusFilter = "all" | NodeStatus;

const statusFilters: Array<{ value: StatusFilter; label: string }> = [
  { value: "all", label: "Todos" },
  { value: "active", label: "Ativos" },
  { value: "pending", label: "Pendentes" },
  { value: "disabled", label: "Revogados" },
];

function isStatusFilter(value: string | null): value is NodeStatus {
  return value === "active" || value === "pending" || value === "disabled";
}

function parseLabels(text: string): { labels: Record<string, string>; error: string | null } {
  const labels: Record<string, string> = {};
  const lines = text.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index].trim();
    if (!line) continue;
    const separator = line.indexOf("=");
    if (separator <= 0) {
      return { labels: {}, error: `Linha ${index + 1}: use o formato chave=valor.` };
    }
    const key = line.slice(0, separator).trim();
    const value = line.slice(separator + 1).trim();
    if (!key) return { labels: {}, error: `Linha ${index + 1}: chave vazia.` };
    labels[key] = value;
  }
  if (Object.keys(labels).length > 64) {
    return { labels: {}, error: "Use no máximo 64 labels." };
  }
  return { labels, error: null };
}

function labelsToText(labels: Record<string, string>): string {
  return Object.entries(labels)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

function tagsToText(tags: string[]): string {
  return tags.join(", ");
}

function parseTags(text: string): string[] {
  return Array.from(
    new Set(
      text
        .split(/[,\n]/)
        .map((tag) => tag.trim())
        .filter(Boolean),
    ),
  ).slice(0, 64);
}

function detailId(node: NodeView): string {
  return `node-detail-${node.id}`;
}

function NodeTagsCell({ node }: { node: NodeView }) {
  const tags = node.tags;
  const labels = Object.entries(node.labels);
  return (
    <div className="max-w-[250px] space-y-1">
      {tags.length === 0 && labels.length === 0 ? (
        <span className="text-[11px] text-faint">sem tags</span>
      ) : (
        <>
          {tags.length > 0 ? (
            <div className="flex flex-wrap gap-1">
              {tags.slice(0, 3).map((tag) => (
                <Badge key={tag} tone="info" className="font-mono">
                  {tag}
                </Badge>
              ))}
              {tags.length > 3 ? <span className="text-[10px] text-faint">+{tags.length - 3}</span> : null}
            </div>
          ) : null}
          {labels.length > 0 ? (
            <div className="space-y-0.5 font-mono text-[10px] leading-4 text-faint">
              {labels.slice(0, 2).map(([key, value]) => (
                <p key={key} className="truncate" title={`${key}=${value}`}>
                  {key}=<span className="text-muted">{value}</span>
                </p>
              ))}
              {labels.length > 2 ? <p className="text-faint">+{labels.length - 2} labels</p> : null}
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}

function PathCell({ node }: { node: NodeView }) {
  const summary = summarizePaths(node.reported_peers);
  const headline = pathHeadline(summary);
  const rtt = summary.minDirectRtt ?? summary.minRelayRtt;
  return (
    <div className="space-y-1">
      <div className="flex flex-wrap items-center gap-1.5">
        <PathBadge kind={headline.kind} />
        {rtt != null ? <span className="font-mono text-[11px] text-muted">{formatRtt(rtt)}</span> : null}
      </div>
      <p className="font-mono text-[10px] text-faint">
        {summary.direct}d · {summary.relay}r · {node.relay_connected ? "relay on" : "relay off"}
      </p>
    </div>
  );
}

function PeerTable({ node, nodes }: { node: NodeView; nodes: NodeView[] }) {
  const byId = useMemo(() => new Map(nodes.map((item) => [item.id, item])), [nodes]);
  if (node.reported_peers.length === 0) {
    return (
      <p className="rounded-md border border-dashed border-line px-3 py-2.5 text-[11px] text-muted">
        Este nó ainda não reportou peers. O agente envia caminho, RTT e handshake a cada ciclo.
      </p>
    );
  }
  return (
    <div className="overflow-x-auto rounded-md border border-line">
      <table className="data-table min-w-[680px]">
        <caption>Peers reportados pelo nó {node.name}</caption>
        <thead>
          <tr>
            <th scope="col">Peer</th>
            <th scope="col">Caminho</th>
            <th scope="col">RTT</th>
            <th scope="col">Handshake</th>
            <th scope="col">Endpoint</th>
            <th scope="col">Erro</th>
          </tr>
        </thead>
        <tbody>
          {node.reported_peers.map((peer) => {
            const target = byId.get(peer.peer_id);
            return (
              <tr key={peer.peer_id}>
                <td className="whitespace-nowrap">
                  <span className="text-xs font-medium text-ink">{target?.name ?? "peer removido"}</span>
                  <span className="ml-2 font-mono text-[10px] text-faint">{target?.overlay_ip ?? peer.peer_id.slice(0, 8)}</span>
                </td>
                <td>
                  <Badge tone={peer.path === "direct" ? "accent" : peer.path === "relay" ? "warn" : "neutral"}>
                    {pathLabel[peer.path]}
                  </Badge>
                </td>
                <td className="whitespace-nowrap font-mono text-[11px] text-muted">{formatRtt(peer.rtt_ms)}</td>
                <td className="whitespace-nowrap font-mono text-[11px] text-muted">
                  {peer.last_handshake_secs == null ? "—" : `há ${formatDuration(peer.last_handshake_secs)}`}
                </td>
                <td className="max-w-[220px] truncate font-mono text-[10px] text-muted" title={peer.endpoint ?? undefined}>
                  {peer.endpoint ?? "—"}
                </td>
                <td className="max-w-[220px]">
                  {peer.last_error ? (
                    <span className="block truncate text-[11px] text-danger" title={peer.last_error}>
                      {peer.last_error}
                    </span>
                  ) : (
                    <span className="text-[11px] text-faint">—</span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function NodeDetail({ node, nodes }: { node: NodeView; nodes: NodeView[] }) {
  const summary = summarizePaths(node.reported_peers);
  return (
    <div className="grid grid-cols-1 gap-4 border-l-2 border-accent/30 bg-black/10 px-4 py-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)]">
      <div className="space-y-3">
        <div>
          <p className="mb-1 text-[10px] uppercase tracking-[0.16em] text-faint">Identidade</p>
          <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-x-2 gap-y-1 text-[11px]">
            <dt className="text-faint">Hostname</dt>
            <dd className="truncate font-mono text-muted">{node.hostname || "—"}</dd>
            <dt className="text-faint">UUID</dt>
            <dd className="truncate font-mono text-muted" title={node.id}>{node.id}</dd>
            <dt className="text-faint">WireGuard</dt>
            <dd className="truncate font-mono text-muted" title={node.wg_public_key}>{node.wg_public_key}</dd>
            <dt className="text-faint">Criado</dt>
            <dd className="text-muted">{formatDateTime(node.created_at)}</dd>
            <dt className="text-faint">Aprovado</dt>
            <dd className="text-muted">{node.approved_at ? formatDateTime(node.approved_at) : "—"}</dd>
            <dt className="text-faint">Visto</dt>
            <dd className="text-muted" title={node.last_seen ?? undefined}>
              {node.last_seen ? `${formatDateTime(node.last_seen)} · ${formatRelative(node.last_seen)}` : "nunca"}
            </dd>
            {node.disabled_at ? (
              <>
                <dt className="text-faint">Revogado</dt>
                <dd className="text-muted">{formatDateTime(node.disabled_at)}</dd>
              </>
            ) : null}
          </dl>
        </div>

        <div>
          <p className="mb-1 text-[10px] uppercase tracking-[0.16em] text-faint">Rede</p>
          <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-x-2 gap-y-1 text-[11px]">
            <dt className="text-faint">NAT</dt>
            <dd className="text-muted">{natLabel[node.nat_type]}</dd>
            <dt className="text-faint">Relay WSS</dt>
            <dd className={node.relay_connected ? "text-accent" : "text-muted"}>
              {node.relay_connected ? "conectado" : "não conectado"}
            </dd>
            <dt className="text-faint">Pares</dt>
            <dd className="text-muted">
              {summary.direct} direto(s) · {summary.relay} via relay · {summary.none} sem caminho
            </dd>
            <dt className="text-faint">Tráfego</dt>
            <dd className="font-mono text-muted">
              rx {formatBytes(node.reported_peers.reduce((sum, peer) => sum + peer.rx_bytes, 0))} · tx{" "}
              {formatBytes(node.reported_peers.reduce((sum, peer) => sum + peer.tx_bytes, 0))}
            </dd>
          </dl>
        </div>

        <div>
          <p className="mb-1 text-[10px] uppercase tracking-[0.16em] text-faint">Tags e labels</p>
          {node.tags.length === 0 && Object.keys(node.labels).length === 0 ? (
            <p className="text-[11px] text-muted">Sem tags ou labels.</p>
          ) : (
            <div className="flex flex-wrap items-center gap-1.5">
              {node.tags.map((tag) => (
                <Badge key={tag} tone="info" className="font-mono">
                  {tag}
                </Badge>
              ))}
              {Object.entries(node.labels).map(([key, value]) => (
                <span key={key} className="rounded border border-line bg-black/20 px-1.5 py-0.5 font-mono text-[10px] text-faint">
                  {key}=<span className="text-muted">{value}</span>
                </span>
              ))}
            </div>
          )}
        </div>

        <div>
          <p className="mb-1 text-[10px] uppercase tracking-[0.16em] text-faint">Endpoints reportados</p>
          {node.endpoints.length === 0 ? (
            <p className="text-[11px] text-muted">Nenhum candidato de endpoint reportado.</p>
          ) : (
            <ul className="space-y-1">
              {node.endpoints.map((endpoint) => (
                <li key={`${endpoint.kind}-${endpoint.addr}`} className="flex items-center gap-2">
                  <Badge tone="neutral">{endpointKindLabel[endpoint.kind]}</Badge>
                  <code className="font-mono text-[11px] text-muted">{endpoint.addr}</code>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      <div>
        <p className="mb-1.5 text-[10px] uppercase tracking-[0.16em] text-faint">
          Caminhos reportados pelo agente
        </p>
        <PeerTable node={node} nodes={nodes} />
      </div>
    </div>
  );
}

interface EditNodeFormProps {
  node: NodeView;
  onClose: () => void;
  onSaved: () => void;
}

function EditNodeForm({ node, onClose, onSaved }: EditNodeFormProps) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const [name, setName] = useState(node.name);
  const [tags, setTags] = useState(tagsToText(node.tags));
  const [labelsText, setLabelsText] = useState(labelsToText(node.labels));
  const [formError, setFormError] = useState<string | null>(null);
  const ids = {
    name: useId(),
    tags: useId(),
    labels: useId(),
  };

  const mutation = useMutation({
    mutationFn: (body: { name: string; tags: string[]; labels: Record<string, string> }) =>
      api.updateNode(node.id, body),
    onSuccess: (updated) => {
      queryClient.setQueryData<NodeView[]>(["nodes"], (current) =>
        current?.map((item) => (item.id === updated.id ? updated : item)),
      );
      invalidateNodeQueries(queryClient);
      push({ tone: "ok", title: "Nó atualizado", description: updated.name });
      onSaved();
    },
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = name.trim();
    if (!/^[A-Za-z0-9._-]{1,63}$/.test(trimmed) || trimmed.startsWith(".")) {
      setFormError("Nome inválido: use 1–63 caracteres (letras, números, ponto, hífen ou underscore), sem começar por ponto.");
      return;
    }
    const parsed = parseLabels(labelsText);
    if (parsed.error) {
      setFormError(parsed.error);
      return;
    }
    setFormError(null);
    mutation.mutate({ name: trimmed, tags: parseTags(tags), labels: parsed.labels });
  }

  return (
    <form onSubmit={handleSubmit} noValidate>
      <div className="space-y-3.5">
        <Field label="Nome" htmlFor={ids.name} hint="Identifica o nó na malha e nos comandos wm.">
          <Input
            id={ids.name}
            value={name}
            onChange={(event) => setName(event.target.value)}
            autoComplete="off"
            spellCheck={false}
            disabled={mutation.isPending}
            data-autofocus
          />
        </Field>

        <Field label="Tags" htmlFor={ids.tags} hint="Separadas por vírgula. Usadas por regras tag: na ACL.">
          <Input
            id={ids.tags}
            value={tags}
            onChange={(event) => setTags(event.target.value)}
            placeholder="operator, mac, baguette"
            autoComplete="off"
            spellCheck={false}
            disabled={mutation.isPending}
          />
        </Field>

        <Field
          label="Labels"
          htmlFor={ids.labels}
          hint="Uma por linha no formato chave=valor. Ex.: baguette_port=8421 e seance=true."
        >
          <Textarea
            id={ids.labels}
            value={labelsText}
            onChange={(event) => setLabelsText(event.target.value)}
            placeholder={"seance=true\nbaguette_port=8421"}
            spellCheck={false}
            disabled={mutation.isPending}
            className="min-h-28 font-mono text-xs"
          />
        </Field>

        {formError || mutation.isError ? (
          <p role="alert" className="rounded-md border border-danger/30 bg-danger/[0.07] px-3 py-2 text-xs leading-5 text-danger">
            {formError ?? errorMessage(mutation.error)}
          </p>
        ) : null}
      </div>

      <div className="mt-4 flex flex-wrap items-center justify-end gap-2 border-t border-line pt-3">
        <Button variant="ghost" onClick={onClose} disabled={mutation.isPending}>
          Cancelar
        </Button>
        <Button type="submit" variant="primary" loading={mutation.isPending}>
          Salvar alterações
        </Button>
      </div>
    </form>
  );
}

export function NodesPage() {
  const { csrfReady } = useAuth();
  const queryClient = useQueryClient();
  const { push } = useToast();
  const nodesQuery = useNodes();
  const [searchParams, setSearchParams] = useSearchParams();
  const statusParam = searchParams.get("status");
  const statusFilter: StatusFilter = isStatusFilter(statusParam) ? statusParam : "all";
  const [search, setSearch] = useState(searchParams.get("q") ?? "");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [editTarget, setEditTarget] = useState<NodeView | null>(null);
  const [revokeTarget, setRevokeTarget] = useState<NodeView | null>(null);

  const qParam = searchParams.get("q");
  useEffect(() => {
    if (qParam) setSearch(qParam);
  }, [qParam]);

  const approveMutation = useMutation({
    mutationFn: (node: NodeView) => api.approveNode(node.id),
    onSuccess: (updated) => {
      push({ tone: "ok", title: "Nó aprovado", description: `${updated.name} agora recebe o mapa da malha.` });
      invalidateNodeQueries(queryClient);
    },
    onError: (error) => push({ tone: "error", title: "Falha ao aprovar", description: errorMessage(error) }),
  });

  const reactivateMutation = useMutation({
    mutationFn: (node: NodeView) => api.updateNode(node.id, { status: "active" }),
    onSuccess: (updated) => {
      push({ tone: "ok", title: "Nó reativado", description: updated.name });
      invalidateNodeQueries(queryClient);
    },
    onError: (error) => push({ tone: "error", title: "Falha ao reativar", description: errorMessage(error) }),
  });

  const revokeMutation = useMutation({
    mutationFn: (node: NodeView) => api.revokeNode(node.id),
    onSuccess: (_result, node) => {
      push({ tone: "ok", title: "Nó revogado", description: `${node.name} perdeu o acesso à malha.` });
      invalidateNodeQueries(queryClient);
      setRevokeTarget(null);
    },
    onError: (error) =>
      push({ tone: "error", title: "Falha ao revogar", description: errorMessage(error) }),
  });

  const nodes = nodesQuery.data ?? [];
  const sorted = useMemo(() => sortNodesByAttention(nodes), [nodes]);
  const counts = useMemo(
    () => ({
      all: nodes.length,
      active: nodes.filter((node) => node.status === "active").length,
      pending: nodes.filter((node) => node.status === "pending").length,
      disabled: nodes.filter((node) => node.status === "disabled").length,
    }),
    [nodes],
  );

  const filtered = useMemo(() => {
    const needle = normalizeSearch(search);
    return sorted.filter((node) => {
      if (statusFilter !== "all" && node.status !== statusFilter) return false;
      if (!needle) return true;
      const haystack = normalizeSearch(
        [
          node.name,
          node.hostname,
          node.overlay_ip,
          node.status,
          ...node.tags,
          ...Object.entries(node.labels).map(([key, value]) => `${key}=${value}`),
        ].join(" "),
      );
      return haystack.includes(needle);
    });
  }, [sorted, statusFilter, search]);

  function changeStatusFilter(next: StatusFilter) {
    const params = new URLSearchParams(searchParams);
    if (next === "all") params.delete("status");
    else params.set("status", next);
    setSearchParams(params, { replace: true });
  }

  function clearFilters() {
    setSearch("");
    const params = new URLSearchParams(searchParams);
    params.delete("status");
    params.delete("q");
    setSearchParams(params, { replace: true });
  }

  function toggleExpanded(id: string) {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  const header = (
    <PageHeader
      eyebrow="Rede"
      title="Nós"
      description="Aprove, edite ou revogue nós. Tags e labels alimentam as regras de ACL e a integração."
      actions={
        <>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void nodesQuery.refetch()}
            loading={nodesQuery.isFetching}
          >
            <RefreshCw aria-hidden="true" className="size-3.5" />
            Atualizar
          </Button>
          <LinkButton to="/tokens" variant="primary" size="sm">
            Convidar nó
          </LinkButton>
        </>
      }
    />
  );

  if (nodesQuery.isLoading) {
    return (
      <>
        {header}
        <LoadingPanel label="Carregando nós…" rows={5} />
      </>
    );
  }

  if (nodesQuery.isError || !nodesQuery.data) {
    return (
      <>
        {header}
        <ErrorState error={nodesQuery.error} onRetry={() => void nodesQuery.refetch()} />
      </>
    );
  }

  return (
    <>
      {header}

      <Card>
        <CardHeader
          title="Inventário da malha"
          description={`${counts.all} nós · ${counts.active} ativos · ${counts.pending} pendentes · ${counts.disabled} revogados`}
          actions={
            <div className="relative">
              <Search
                aria-hidden="true"
                className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-faint"
              />
              <Input
                type="search"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder="nome, IP, tag ou label"
                aria-label="Filtrar nós"
                className="h-8 w-56 py-1 pl-8 pr-8 text-xs"
              />
              {search ? (
                <button
                  type="button"
                  onClick={() => setSearch("")}
                  aria-label="Limpar busca"
                  className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-1 text-faint hover:text-ink"
                >
                  <X aria-hidden="true" className="size-3" />
                </button>
              ) : null}
            </div>
          }
        />

        <div className="flex flex-wrap items-center gap-1.5 border-b border-line px-4 py-2.5">
          {statusFilters.map((filter) => (
            <button
              key={filter.value}
              type="button"
              onClick={() => changeStatusFilter(filter.value)}
              aria-pressed={statusFilter === filter.value}
              className={cn(
                "inline-flex h-7 items-center gap-1.5 rounded-md border px-2 text-[11px] font-medium transition-colors",
                statusFilter === filter.value
                  ? "border-accent/30 bg-accent/10 text-accent"
                  : "border-transparent text-muted hover:bg-white/[0.04] hover:text-ink",
              )}
            >
              {filter.label}
              <span className="font-mono text-[10px] opacity-70">{counts[filter.value]}</span>
            </button>
          ))}
          <span className="ml-auto text-[10px] text-faint">
            {filtered.length} exibido{filtered.length === 1 ? "" : "s"}
          </span>
        </div>

        {!csrfReady ? (
          <p className="border-b border-warn/20 bg-warn/[0.05] px-4 py-2 text-[11px] text-warn">
            Sessão sem CSRF nesta aba: os botões de alteração vão pedir que você entre novamente.
          </p>
        ) : null}

        {nodes.length === 0 ? (
          <div className="p-4">
            <EmptyState
              icon={<ServerOff aria-hidden="true" className="size-4" />}
              title="Nenhum nó na malha"
              description="Gere um token de entrada e rode `wm join` em um Mac ou servidor para começar."
              action={<LinkButton to="/tokens" variant="primary">Criar token de entrada</LinkButton>}
            />
          </div>
        ) : filtered.length === 0 ? (
          <div className="p-4">
            <EmptyState
              title="Nenhum nó corresponde ao filtro"
              description="Ajuste a busca ou volte para todos os status."
              action={<Button onClick={clearFilters}>Limpar filtros</Button>}
            />
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="data-table min-w-[660px] xl:min-w-[980px]">
              <caption>Nós da malha, status, presença e caminhos reportados</caption>
              <thead>
                <tr>
                  <th scope="col" className="w-8">
                    <span className="sr-only">Detalhes</span>
                  </th>
                  <th scope="col">Nó</th>
                  <th scope="col">Overlay</th>
                  <th scope="col">Status</th>
                  <th scope="col" className="hidden xl:table-cell">Tags / labels</th>
                  <th scope="col">Presença</th>
                  <th scope="col">Caminho / RTT</th>
                  <th scope="col" className="hidden xl:table-cell">Visto</th>
                  <th scope="col" className="sticky right-0 z-10 border-l border-line/70 bg-panel text-right">
                    Ações
                  </th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((node) => {
                  const isExpanded = expanded.has(node.id);
                  const approvePending = approveMutation.isPending && approveMutation.variables?.id === node.id;
                  const reactivatePending =
                    reactivateMutation.isPending && reactivateMutation.variables?.id === node.id;
                  return (
                    <NodeRowFragment
                      key={node.id}
                      node={node}
                      nodes={nodes}
                      expanded={isExpanded}
                      approvePending={approvePending}
                      reactivatePending={reactivatePending}
                      onToggle={() => toggleExpanded(node.id)}
                      onApprove={() => approveMutation.mutate(node)}
                      onReactivate={() => reactivateMutation.mutate(node)}
                      onEdit={() => setEditTarget(node)}
                      onRevoke={() => {
                        revokeMutation.reset();
                        setRevokeTarget(node);
                      }}
                    />
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      <p className="mt-3 max-w-3xl text-[11px] leading-5 text-faint">
        Caminho e RTT vêm do último relatório do próprio agente. “Presença” combina last_seen com a
        conexão de relay; um nó offline pode ter relatórios defasados.
      </p>

      <Modal
        open={editTarget !== null}
        onClose={() => setEditTarget(null)}
        title="Editar nó"
        description={
          editTarget
            ? `Ajuste identidade e seletores de ${editTarget.name}. Alterações disparam uma nova versão do mapa e podem recompilar a ACL.`
            : undefined
        }
        size="md"
      >
        {editTarget ? (
          <EditNodeForm
            key={editTarget.id}
            node={editTarget}
            onClose={() => setEditTarget(null)}
            onSaved={() => setEditTarget(null)}
          />
        ) : null}
      </Modal>

      <ConfirmDialog
        open={revokeTarget !== null}
        onClose={() => {
          if (!revokeMutation.isPending) {
            setRevokeTarget(null);
            revokeMutation.reset();
          }
        }}
        onConfirm={() => {
          if (revokeTarget) revokeMutation.mutate(revokeTarget);
        }}
        title="Revogar nó"
        description={
          revokeTarget ? (
            revokeTarget.status === "pending" ? (
              <>
                <strong className="text-ink">{revokeTarget.name}</strong> será rejeitado e não entrará
                na malha. O registro fica disponível para uma futura reativação.
              </>
            ) : (
              <>
                <strong className="text-ink">{revokeTarget.name}</strong> perde acesso à malha até ser
                reativado. O agente deixa de receber mapa e os peers removem a chave.
              </>
            )
          ) : null
        }
        confirmLabel="Revogar nó"
        pending={revokeMutation.isPending}
        error={revokeMutation.error}
      >
        <div className="space-y-2 text-[11px] leading-5 text-muted">
          <p>O registro e o histórico são preservados. Você poderá reativar o nó depois, se necessário.</p>
          <p>Esta ação é registrada na auditoria com o seu usuário.</p>
        </div>
      </ConfirmDialog>
    </>
  );
}

interface NodeRowFragmentProps {
  node: NodeView;
  nodes: NodeView[];
  expanded: boolean;
  approvePending: boolean;
  reactivatePending: boolean;
  onToggle: () => void;
  onApprove: () => void;
  onReactivate: () => void;
  onEdit: () => void;
  onRevoke: () => void;
}

function NodeRowFragment({
  node,
  nodes,
  expanded,
  approvePending,
  reactivatePending,
  onToggle,
  onApprove,
  onReactivate,
  onEdit,
  onRevoke,
}: NodeRowFragmentProps) {
  const summary = summarizePaths(node.reported_peers);
  return (
    <>
      <tr className={expanded ? "bg-white/[0.02]" : undefined}>
        <td className="pr-0">
          <button
            type="button"
            onClick={onToggle}
            aria-expanded={expanded}
            aria-controls={detailId(node)}
            aria-label={expanded ? `Recolher detalhes de ${node.name}` : `Expandir detalhes de ${node.name}`}
            className="rounded p-1 text-faint hover:bg-white/5 hover:text-ink"
          >
            {expanded ? <ChevronDown aria-hidden="true" className="size-3.5" /> : <ChevronRight aria-hidden="true" className="size-3.5" />}
          </button>
        </td>
        <td className="max-w-[240px]">
          <span className="block truncate text-[13px] font-medium text-ink">{node.name}</span>
          <span className="block truncate text-[10px] text-faint">{node.hostname || "—"}</span>
          {summary.errors.length > 0 ? (
            <span className="mt-0.5 inline-flex items-center gap-1 text-[10px] text-danger">
              <span aria-hidden="true" className="size-1 rounded-full bg-danger" />
              {summary.errors.length} erro(s)
            </span>
          ) : null}
        </td>
        <td>
          <span className="block font-mono text-[11px] text-ink">{node.overlay_ip}</span>
          <span className="block font-mono text-[10px] text-faint">{natLabel[node.nat_type]}</span>
        </td>
        <td>
          <NodeStatusBadge status={node.status} />
        </td>
        <td className="hidden xl:table-cell">
          <NodeTagsCell node={node} />
        </td>
        <td>
          <div className="space-y-1">
            <OnlineBadge online={isNodeOnline(node)} viaRelay={isNodeOnlineViaRelay(node)} />
            <p className="font-mono text-[10px] text-faint">{node.relay_connected ? "relay on" : "relay off"}</p>
          </div>
        </td>
        <td>
          <PathCell node={node} />
        </td>
        <td className="hidden whitespace-nowrap text-xs text-muted xl:table-cell" title={node.last_seen ?? undefined}>
          {formatRelative(node.last_seen)}
        </td>
        <td className="sticky right-0 z-10 border-l border-line/60 bg-panel">
          <div className="flex items-center justify-end gap-1">
            {node.status === "pending" ? (
              <Button
                variant="primary"
                size="sm"
                loading={approvePending}
                onClick={onApprove}
                title={`Aprovar ${node.name}`}
                className="h-7 px-2 text-[11px]"
              >
                <Check aria-hidden="true" className="size-3.5" />
                <span className="hidden xl:inline">Aprovar</span>
              </Button>
            ) : null}
            {node.status === "disabled" ? (
              <Button
                variant="secondary"
                size="sm"
                loading={reactivatePending}
                onClick={onReactivate}
                title={`Reativar ${node.name}`}
                aria-label={`Reativar ${node.name}`}
                className="size-7 px-0"
              >
                <RotateCcw aria-hidden="true" className="size-3.5" />
              </Button>
            ) : null}
            <Button
              variant="ghost"
              size="sm"
              onClick={onEdit}
              title={`Editar ${node.name}`}
              aria-label={`Editar ${node.name}`}
              className="size-7 px-0"
            >
              <Pencil aria-hidden="true" className="size-3.5" />
            </Button>
            {node.status !== "disabled" ? (
              <Button
                variant="ghost"
                size="sm"
                onClick={onRevoke}
                title={`Revogar ${node.name}`}
                aria-label={`Revogar ${node.name}`}
                className="size-7 px-0 text-faint hover:text-danger"
              >
                <Trash2 aria-hidden="true" className="size-3.5" />
              </Button>
            ) : null}
          </div>
        </td>
      </tr>
      {expanded ? (
        <tr id={detailId(node)}>
          <td colSpan={9} className="p-0">
            <NodeDetail node={node} nodes={nodes} />
          </td>
        </tr>
      ) : null}
    </>
  );
}
