import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Cloud,
  Globe2,
  Plus,
  RefreshCw,
  RotateCw,
  Settings2,
  ShieldAlert,
  Trash2,
} from "lucide-react";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { formatDateTime } from "../lib/format";
import {
  invalidateExtensionQueries,
  useCloudflaredOverview,
  useNodes,
} from "../lib/queries";
import type {
  CloudflaredMode,
  CloudflaredNodeView,
  CloudflaredRouteInput,
  CloudflaredRouteView,
  CloudflaredNodeSettingsInput,
  ExtensionStatus,
  NodeView,
} from "../lib/types";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  CopyButton,
  EmptyState,
  ErrorState,
  Field,
  InlineCode,
  Input,
  LinkButton,
  LoadingPanel,
  PageHeader,
  Select,
  StatCard,
  Textarea,
} from "../components/ui";
import { ConfirmDialog, Modal } from "../components/Modal";
import { useToast } from "../components/Toast";

type BadgeTone = "neutral" | "ok" | "warn" | "danger" | "info" | "accent";

function stateTone(state: string | null | undefined): BadgeTone {
  if (state === "running") return "ok";
  if (state === "starting" || state === "pending") return "warn";
  if (state === "error") return "danger";
  return "neutral";
}

function sidecarTone(status: ExtensionStatus | null | undefined): BadgeTone {
  return stateTone(status?.state);
}

function stateLabel(state: string): string {
  if (state === "running") return "ativo";
  if (state === "starting") return "iniciando";
  if (state === "error") return "erro";
  if (state === "pending") return "aguardando";
  if (state === "stopped") return "parado";
  return state;
}

function sidecarBadge(status: ExtensionStatus | null | undefined) {
  if (!status) return <Badge tone="neutral">sem report</Badge>;
  return (
    <Badge tone={sidecarTone(status)} dot title={status.message ?? undefined}>
      {stateLabel(status.state)}
    </Badge>
  );
}

function routeStatusBadge(route: CloudflaredRouteView) {
  if (!route.enabled) return <Badge tone="neutral">desativada</Badge>;
  if (!route.status) return <Badge tone="neutral">sem report</Badge>;
  return (
    <Badge tone={stateTone(route.status.state)} dot title={route.status.message ?? undefined}>
      {stateLabel(route.status.state)}
    </Badge>
  );
}

function modeLabel(mode: CloudflaredMode): string {
  return mode === "named" ? "named" : "quick";
}

function shortId(value: string | null | undefined): string {
  if (!value) return "—";
  return value.length > 16 ? `${value.slice(0, 8)}…${value.slice(-4)}` : value;
}

// ---------------------------------------------------------------------------
// Formulário de rota
// ---------------------------------------------------------------------------

function RouteEditorModal({
  open,
  initial,
  nodes,
  onClose,
  onSaved,
}: {
  open: boolean;
  initial: CloudflaredRouteView | null;
  nodes: NodeView[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const { push } = useToast();
  const editing = initial != null;
  const [project, setProject] = useState("");
  const [nodeId, setNodeId] = useState("");
  const [localUrl, setLocalUrl] = useState("");
  const [mode, setMode] = useState<CloudflaredMode>("quick");
  const [hostname, setHostname] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [formError, setFormError] = useState<string | null>(null);

  const selectable = useMemo(
    () => nodes.filter((node) => node.status !== "disabled"),
    [nodes],
  );

  useEffect(() => {
    if (!open) return;
    setProject(initial?.project ?? "");
    setNodeId(initial?.node_id ?? selectable[0]?.id ?? "");
    setLocalUrl(initial?.local_url ?? "");
    setMode(initial?.mode ?? "quick");
    setHostname(initial?.hostname ?? "");
    setEnabled(initial?.enabled ?? true);
    setFormError(null);
    // Só reinicializa ao abrir; refetches da lista não devem apagar o formulário.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const saveMutation = useMutation({
    mutationFn: (body: CloudflaredRouteInput) =>
      initial ? api.updateCloudflaredRoute(initial.id, body) : api.createCloudflaredRoute(body),
    onSuccess: () => {
      push({
        tone: "ok",
        title: editing ? "Rota atualizada" : "Rota criada",
        description: "O nó reconcilia o cloudflared ao receber o próximo mapa.",
      });
      onSaved();
      onClose();
    },
    onError: (error) => setFormError(errorMessage(error)),
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setFormError(null);
    if (!nodeId) {
      setFormError("Escolha um nó.");
      return;
    }
    const payload: CloudflaredRouteInput = {
      project: project.trim(),
      node_id: nodeId,
      local_url: localUrl.trim(),
      mode,
      hostname: mode === "named" ? hostname.trim() || null : null,
      enabled,
    };
    saveMutation.mutate(payload);
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={editing ? "Editar rota" : "Nova rota"}
      description="O alvo precisa ser loopback do próprio nó. O agente executa o cloudflared localmente."
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancelar
          </Button>
          <Button type="submit" form="cloudflared-route-form" loading={saveMutation.isPending}>
            {editing ? "Salvar rota" : "Criar rota"}
          </Button>
        </>
      }
    >
      <form id="cloudflared-route-form" className="space-y-3" onSubmit={submit}>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field label="Projeto" htmlFor="cf-project" hint="Um slug curto para identificar o serviço.">
            <Input
              id="cf-project"
              value={project}
              onChange={(event) => setProject(event.target.value)}
              placeholder="seance-site"
              maxLength={63}
              required
            />
          </Field>
          <Field label="Nó" htmlFor="cf-node" hint="Onde o sidecar cloudflared vai rodar.">
            <Select
              id="cf-node"
              value={nodeId}
              onChange={(event) => setNodeId(event.target.value)}
              required
            >
              <option value="" disabled>
                escolha um nó
              </option>
              {selectable.map((node) => (
                <option key={node.id} value={node.id}>
                  {node.name} · {node.overlay_ip}
                  {node.online ? "" : " (offline)"}
                </option>
              ))}
            </Select>
          </Field>
        </div>

        <Field
          label="Serviço local"
          htmlFor="cf-local-url"
          hint="http(s)://127.0.0.1:porta, localhost ou [::1]. Portas em outros hosts são recusadas."
        >
          <Input
            id="cf-local-url"
            value={localUrl}
            onChange={(event) => setLocalUrl(event.target.value)}
            placeholder="http://127.0.0.1:3000"
            required
          />
        </Field>

        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field label="Modo" htmlFor="cf-mode" hint="Quick não exige conta Cloudflare; named usa seu DNS.">
            <Select
              id="cf-mode"
              value={mode}
              onChange={(event) => setMode(event.target.value as CloudflaredMode)}
            >
              <option value="quick">Quick tunnel (URL temporária)</option>
              <option value="named">Named tunnel (hostname próprio)</option>
            </Select>
          </Field>
          {mode === "named" ? (
            <Field
              label="Hostname público"
              htmlFor="cf-hostname"
              hint="Precisa ter DNS na Cloudflare apontando para o túnel."
            >
              <Input
                id="cf-hostname"
                value={hostname}
                onChange={(event) => setHostname(event.target.value)}
                placeholder="app.exemplo.com"
                required
              />
            </Field>
          ) : (
            <div className="rounded-md border border-line bg-black/20 p-3 text-[11px] leading-4 text-faint">
              O cloudflared gera uma URL <InlineCode>*.trycloudflare.com</InlineCode> efêmera, sem
              autenticação. Use apenas para testes.
            </div>
          )}
        </div>

        <label className="flex items-center gap-2 text-xs text-muted">
          <input
            type="checkbox"
            checked={enabled}
            onChange={(event) => setEnabled(event.target.checked)}
            className="size-3.5 accent-accent"
          />
          Rota ativa
        </label>

        {formError ? (
          <p role="alert" className="rounded border border-danger/30 bg-danger/[0.06] px-2.5 py-2 text-[11px] leading-4 text-danger">
            {formError}
          </p>
        ) : null}
      </form>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Formulário de configuração do nó
// ---------------------------------------------------------------------------

function NodeSettingsModal({
  open,
  node,
  onClose,
  onSaved,
}: {
  open: boolean;
  node: CloudflaredNodeView | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { push } = useToast();
  const [binary, setBinary] = useState("cloudflared");
  const [tunnelId, setTunnelId] = useState("");
  const [credentials, setCredentials] = useState("");
  const [replaceCredentials, setReplaceCredentials] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setBinary(node?.binary || "cloudflared");
    setTunnelId(node?.tunnel_id ?? "");
    setCredentials("");
    setReplaceCredentials(false);
    setFormError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const saveMutation = useMutation({
    mutationFn: (body: CloudflaredNodeSettingsInput) =>
      api.updateCloudflaredNode(node?.node_id ?? "", body),
    onSuccess: () => {
      push({ tone: "ok", title: "Configuração do nó salva", description: "O agente aplica no próximo mapa." });
      onSaved();
      onClose();
    },
    onError: (error) => setFormError(errorMessage(error)),
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setFormError(null);
    const payload: CloudflaredNodeSettingsInput = {
      binary: binary.trim(),
      tunnel_id: tunnelId.trim(),
    };
    const trimmedCredentials = credentials.trim();
    if (replaceCredentials || (!node?.has_credentials && trimmedCredentials)) {
      payload.credentials_json = trimmedCredentials;
    }
    saveMutation.mutate(payload);
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Configurar cloudflared do nó"
      description={node ? `${node.node_name} · ${node.overlay_ip}` : undefined}
      size="md"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancelar
          </Button>
          <Button type="submit" form="cloudflared-node-form" loading={saveMutation.isPending}>
            Salvar configuração
          </Button>
        </>
      }
    >
      <form id="cloudflared-node-form" className="space-y-3" onSubmit={submit}>
        <Field
          label="Binário do cloudflared"
          htmlFor="cf-binary"
          hint="Nome no PATH ou caminho absoluto no nó."
        >
          <Input
            id="cf-binary"
            value={binary}
            onChange={(event) => setBinary(event.target.value)}
            placeholder="cloudflared"
          />
        </Field>
        <Field
          label="Tunnel ID"
          htmlFor="cf-tunnel-id"
          hint="O UUID do túnel criado com cloudflared tunnel create."
        >
          <Input
            id="cf-tunnel-id"
            value={tunnelId}
            onChange={(event) => setTunnelId(event.target.value)}
            placeholder="00000000-0000-0000-0000-000000000000"
          />
        </Field>
        <Field
          label="Credentials JSON"
          htmlFor="cf-credentials"
          hint={
            node?.has_credentials
              ? "Já existe um segredo configurado; ele nunca é exibido de volta."
              : "Cole o conteúdo do JSON de credenciais do túnel."
          }
        >
          <Textarea
            id="cf-credentials"
            className="min-h-32 font-mono text-[11px]"
            spellCheck={false}
            value={credentials}
            onChange={(event) => setCredentials(event.target.value)}
            placeholder={node?.has_credentials ? "deixe em branco para manter" : "{\"AccountTag\":\"…\"}"}
          />
        </Field>
        {node?.has_credentials ? (
          <label className="flex items-center gap-2 text-xs text-muted">
            <input
              type="checkbox"
              checked={replaceCredentials}
              onChange={(event) => setReplaceCredentials(event.target.checked)}
              className="size-3.5 accent-accent"
            />
            Substituir as credenciais existentes
          </label>
        ) : null}
        <p className="rounded border border-warn/30 bg-warn/[0.05] px-2.5 py-2 text-[11px] leading-4 text-warn">
          Para limpar o túnel, apague o Tunnel ID e salve. Rotas named ativas impedem a limpeza.
          O segredo fica no SQLite do control plane (arquivo 0600) e só é entregue no mapa deste nó.
        </p>
        {formError ? (
          <p role="alert" className="rounded border border-danger/30 bg-danger/[0.06] px-2.5 py-2 text-[11px] leading-4 text-danger">
            {formError}
          </p>
        ) : null}
      </form>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Página
// ---------------------------------------------------------------------------

export function CloudflarePage() {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const overviewQuery = useCloudflaredOverview();
  const nodesQuery = useNodes();
  const [routeEditor, setRouteEditor] = useState<{ open: boolean; route: CloudflaredRouteView | null }>({
    open: false,
    route: null,
  });
  const [nodeEditor, setNodeEditor] = useState<{ open: boolean; node: CloudflaredNodeView | null }>({
    open: false,
    node: null,
  });
  const [deleteTarget, setDeleteTarget] = useState<CloudflaredRouteView | null>(null);

  const refresh = () => {
    void overviewQuery.refetch();
    void nodesQuery.refetch();
  };

  const invalidate = () => {
    invalidateExtensionQueries(queryClient);
    refresh();
  };

  const deleteMutation = useMutation({
    mutationFn: (route: CloudflaredRouteView) => api.deleteCloudflaredRoute(route.id),
    onSuccess: () => {
      push({ tone: "ok", title: "Rota excluída", description: "O sidecar será reconciliado no próximo mapa." });
      setDeleteTarget(null);
      invalidate();
    },
    onError: (error) =>
      push({ tone: "error", title: "Falha ao excluir", description: errorMessage(error) }),
  });

  const restartMutation = useMutation({
    mutationFn: (node: CloudflaredNodeView) => api.restartCloudflaredNode(node.node_id),
    onSuccess: (_result, node) => {
      push({ tone: "ok", title: "Reinício solicitado", description: node.node_name });
      invalidate();
    },
    onError: (error) =>
      push({ tone: "error", title: "Falha ao reiniciar", description: errorMessage(error) }),
  });

  const activationMutation = useMutation({
    mutationFn: ({ node, active }: { node: CloudflaredNodeView; active: boolean }) =>
      api.setExtensionActivation("cloudflared", node.node_id, active),
    onSuccess: (_result, variables) => {
      push({
        tone: "ok",
        title: variables.active ? "Extensão ativada" : "Extensão desativada",
        description: variables.node.node_name,
      });
      invalidate();
    },
    onError: (error) =>
      push({ tone: "error", title: "Falha ao alterar ativação", description: errorMessage(error) }),
  });

  const header = (
    <PageHeader
      eyebrow="Extensão"
      title="Cloudflare Tunnel"
      description="Exponha serviços locais de um nó pela Cloudflare. O control plane guarda o desejo; o agente roda e reporta o sidecar."
      actions={
        <>
          <Button variant="ghost" size="sm" onClick={refresh} loading={overviewQuery.isFetching}>
            <RefreshCw aria-hidden="true" className="size-3.5" />
            Atualizar
          </Button>
          <Button size="sm" onClick={() => setRouteEditor({ open: true, route: null })}>
            <Plus aria-hidden="true" className="size-3.5" />
            Nova rota
          </Button>
        </>
      }
    />
  );

  if (overviewQuery.isLoading) {
    return (
      <>
        {header}
        <LoadingPanel label="Lendo rotas e estado do cloudflared…" rows={4} />
      </>
    );
  }

  if (overviewQuery.isError || !overviewQuery.data) {
    return (
      <>
        {header}
        <ErrorState error={overviewQuery.error} onRetry={() => overviewQuery.refetch()} />
      </>
    );
  }

  const overview = overviewQuery.data;
  const nodes = overview.nodes;
  const routes = overview.routes;
  const configuredNodes = nodes.filter((node) => node.tunnel_id && node.has_credentials).length;
  const runningNodes = nodes.filter((node) => node.status?.state === "running").length;
  const quickRoutes = routes.filter((route) => route.mode === "quick").length;
  const namedRoutes = routes.filter((route) => route.mode === "named").length;

  return (
    <>
      <Link to="/extensions" className="mb-3 inline-flex items-center gap-1 text-xs text-muted hover:text-ink">
        <ArrowLeft aria-hidden="true" className="size-3.5" />
        Extensões
      </Link>
      {header}

      <div className="mb-4 grid grid-cols-2 gap-2.5 xl:grid-cols-4">
        <StatCard
          label="Sidecars ativos"
          value={runningNodes}
          hint={`${nodes.length} nó(s) na malha`}
          tone={runningNodes > 0 ? "ok" : "default"}
          icon={<Cloud aria-hidden="true" className="size-4" />}
        />
        <StatCard
          label="Rotas"
          value={routes.length}
          hint={`${routes.filter((route) => route.enabled).length} ativa(s)`}
          tone="accent"
          icon={<Globe2 aria-hidden="true" className="size-4" />}
        />
        <StatCard label="Quick" value={quickRoutes} hint="URL temporária, sem conta" />
        <StatCard
          label="Named"
          value={namedRoutes}
          hint={`${configuredNodes} nó(s) com túnel configurado`}
          tone={namedRoutes > 0 && configuredNodes === 0 ? "warn" : "default"}
        />
      </div>

      <div className="mb-4 flex items-start gap-2.5 rounded-lg border border-info/25 bg-info/[0.04] px-3.5 py-3">
        <Globe2 aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-info" />
        <div className="text-xs leading-5 text-muted">
          <p className="font-medium text-ink">Quick ou named</p>
          <p className="mt-0.5">
            <strong className="text-ink">Quick</strong> cria uma URL <InlineCode>*.trycloudflare.com</InlineCode>{" "}
            efêmera por rota. <strong className="text-ink">Named</strong> usa um túnel com credenciais e
            hostnames do seu DNS, com um processo por nó. O <InlineCode>cloudflared</InlineCode> precisa
            estar instalado no nó ou o caminho do binário configurado.
          </p>
        </div>
      </div>

      <div className="mb-4 flex items-start gap-2.5 rounded-lg border border-warn/30 bg-warn/[0.06] px-3.5 py-3">
        <ShieldAlert aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warn" />
        <p className="text-xs leading-5 text-muted">
          Túneis quick são públicos e sem autenticação. Para projetos reais, prefira named + Cloudflare
          Access. O agente recusa alvos que não sejam loopback do próprio nó.
        </p>
      </div>

      <Card className="mb-4">
        <CardHeader
          title="Nós e túneis"
          description="O sidecar reporta o estado por nó; o segredo das credenciais nunca é exibido de volta."
          actions={<Badge tone="neutral">{nodes.length} nó(s)</Badge>}
        />
        {nodes.length === 0 ? (
          <EmptyState title="Nenhum nó na malha" description="Inscreva um nó para rodar o cloudflared." />
        ) : (
          <div className="overflow-x-auto">
            <table className="data-table min-w-[980px]">
              <caption>Configuração e estado do cloudflared por nó</caption>
              <thead>
                <tr>
                  <th scope="col">Nó</th>
                  <th scope="col">Ativa</th>
                  <th scope="col">Binário</th>
                  <th scope="col">Túnel</th>
                  <th scope="col">Credenciais</th>
                  <th scope="col">Rotas</th>
                  <th scope="col">Sidecar</th>
                  <th scope="col" className="text-right">Ações</th>
                </tr>
              </thead>
              <tbody>
                {nodes.map((node) => (
                  <tr key={node.node_id}>
                    <td>
                      <p className="font-medium text-ink">{node.node_name}</p>
                      <p className="font-mono text-[10px] text-faint">
                        {node.overlay_ip} · {node.hostname}
                      </p>
                    </td>
                    <td>
                      {node.active ? <Badge tone="ok">on</Badge> : <Badge>off</Badge>}
                    </td>
                    <td>
                      <InlineCode>{node.binary}</InlineCode>
                    </td>
                    <td>
                      {node.tunnel_id ? (
                        <span className="font-mono text-[11px] text-muted" title={node.tunnel_id}>
                          {shortId(node.tunnel_id)}
                        </span>
                      ) : (
                        <span className="text-[11px] text-faint">—</span>
                      )}
                    </td>
                    <td>
                      {node.has_credentials ? (
                        <Badge tone="ok">configuradas</Badge>
                      ) : (
                        <Badge tone="warn">faltando</Badge>
                      )}
                    </td>
                    <td className="whitespace-nowrap">
                      {node.quick_routes > 0 ? <Badge tone="info">{node.quick_routes} quick</Badge> : null}
                      {node.named_routes > 0 ? <Badge tone="info">{node.named_routes} named</Badge> : null}
                      {node.quick_routes === 0 && node.named_routes === 0 ? (
                        <span className="text-[11px] text-faint">—</span>
                      ) : null}
                    </td>
                    <td>
                      <div className="space-y-1">
                        {sidecarBadge(node.status)}
                        <p className="max-w-[240px] text-[10px] leading-4 text-faint">
                          {node.status?.message ?? "sem mensagem"}
                        </p>
                      </div>
                    </td>
                    <td className="text-right">
                      <div className="flex justify-end gap-1.5">
                        <Button
                          size="sm"
                          variant={node.active ? "ghost" : "primary"}
                          onClick={() => activationMutation.mutate({ node, active: !node.active })}
                          loading={
                            activationMutation.isPending &&
                            activationMutation.variables?.node.node_id === node.node_id
                          }
                        >
                          {node.active ? "Desativar" : "Ativar"}
                        </Button>
                        <Button
                          size="sm"
                          variant="secondary"
                          onClick={() => setNodeEditor({ open: true, node })}
                        >
                          <Settings2 aria-hidden="true" className="size-3.5" />
                          Configurar
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            if (window.confirm(`Reiniciar o cloudflared em ${node.node_name}?`)) {
                              restartMutation.mutate(node);
                            }
                          }}
                          loading={restartMutation.isPending && restartMutation.variables?.node_id === node.node_id}
                        >
                          <RotateCw aria-hidden="true" className="size-3.5" />
                          Reiniciar
                        </Button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      <Card>
        <CardHeader
          title="Rotas de projeto"
          description="Cada rota vira um ingress do cloudflared. O status/URL vem do report do agente."
          actions={
            <Button size="sm" onClick={() => setRouteEditor({ open: true, route: null })}>
              <Plus aria-hidden="true" className="size-3.5" />
              Nova rota
            </Button>
          }
        />
        {routes.length === 0 ? (
          <EmptyState
            title="Nenhuma rota ainda"
            description="Crie a primeira rota para expor um serviço local de um nó. No modo quick, a URL aparece em alguns segundos."
            action={
              <Button size="sm" onClick={() => setRouteEditor({ open: true, route: null })}>
                <Plus aria-hidden="true" className="size-3.5" />
                Criar rota
              </Button>
            }
          />
        ) : (
          <div className="overflow-x-auto">
            <table className="data-table min-w-[980px]">
              <caption>Rotas cloudflared por projeto</caption>
              <thead>
                <tr>
                  <th scope="col">Projeto</th>
                  <th scope="col">Nó</th>
                  <th scope="col">Destino</th>
                  <th scope="col">Modo</th>
                  <th scope="col">Público / status</th>
                  <th scope="col">Ativa</th>
                  <th scope="col" className="text-right">Ações</th>
                </tr>
              </thead>
              <tbody>
                {routes.map((route) => {
                  const url =
                    route.status?.public_url ??
                    (route.mode === "named" && route.hostname ? `https://${route.hostname}` : null);
                  return (
                    <tr key={route.id}>
                      <td>
                        <p className="font-medium text-ink">{route.project}</p>
                        <p className="font-mono text-[10px] text-faint">{route.id.slice(0, 8)}</p>
                      </td>
                      <td>
                        <p className="text-[12px] text-ink">{route.node_name}</p>
                        <p className="text-[10px] text-faint">
                          {route.node_online ? "online" : "offline"}
                        </p>
                      </td>
                      <td>
                        <InlineCode>{route.local_url}</InlineCode>
                      </td>
                      <td>
                        <Badge tone={route.mode === "named" ? "accent" : "neutral"}>
                          {modeLabel(route.mode)}
                        </Badge>
                      </td>
                      <td>
                        <div className="flex max-w-[360px] flex-col gap-1">
                          {url ? (
                            <div className="flex items-center gap-1.5">
                              <a
                                href={url}
                                target="_blank"
                                rel="noreferrer"
                                className="truncate text-xs text-accent hover:underline"
                                title={url}
                              >
                                {url}
                              </a>
                              <CopyButton text={url} label="" variant="ghost" />
                            </div>
                          ) : (
                            <span className="text-[11px] text-faint">aguardando URL</span>
                          )}
                          <div className="flex flex-wrap items-center gap-1.5">
                            {routeStatusBadge(route)}
                            {route.status?.message ? (
                              <span className="text-[10px] leading-4 text-faint">{route.status.message}</span>
                            ) : null}
                          </div>
                        </div>
                      </td>
                      <td>{route.enabled ? <Badge tone="ok">ativa</Badge> : <Badge>off</Badge>}</td>
                      <td className="text-right">
                        <div className="flex justify-end gap-1.5">
                          <Button
                            size="sm"
                            variant="secondary"
                            onClick={() => setRouteEditor({ open: true, route })}
                          >
                            <Settings2 aria-hidden="true" className="size-3.5" />
                            Editar
                          </Button>
                          <Button
                            size="sm"
                            variant="danger"
                            onClick={() => setDeleteTarget(route)}
                          >
                            <Trash2 aria-hidden="true" className="size-3.5" />
                            Excluir
                          </Button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      <p className="mt-4 text-[10px] leading-4 text-faint">
        Detalhes de auditoria: cada criação, edição, exclusão e reinício gera um evento. Último report do
        sidecar: {formatDateTime(nodes.find((node) => node.status)?.status?.updated_at)}.
      </p>

      <RouteEditorModal
        open={routeEditor.open}
        initial={routeEditor.route}
        nodes={nodesQuery.data ?? []}
        onClose={() => setRouteEditor({ open: false, route: null })}
        onSaved={invalidate}
      />
      <NodeSettingsModal
        open={nodeEditor.open}
        node={nodeEditor.node}
        onClose={() => setNodeEditor({ open: false, node: null })}
        onSaved={invalidate}
      />
      <ConfirmDialog
        open={deleteTarget != null}
        onClose={() => setDeleteTarget(null)}
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget);
        }}
        title="Excluir rota"
        description={
          <>
            Remover a rota <strong className="text-ink">{deleteTarget?.project}</strong> do nó{" "}
            <strong className="text-ink">{deleteTarget?.node_name}</strong>? O sidecar será reconciliado no
            próximo mapa.
          </>
        }
        confirmLabel="Excluir rota"
        pending={deleteMutation.isPending}
        error={deleteMutation.error}
      />

      <div className="mt-4">
        <LinkButton to="/extensions" variant="ghost" size="sm">
          <ArrowLeft aria-hidden="true" className="size-3.5" />
          Voltar para extensões
        </LinkButton>
      </div>
    </>
  );
}
