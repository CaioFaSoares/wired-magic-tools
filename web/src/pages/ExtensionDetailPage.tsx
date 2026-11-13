import { useEffect, useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, RefreshCw, Save, Trash2, TriangleAlert } from "lucide-react";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { formatDateTime } from "../lib/format";
import { invalidateExtensionQueries, useExtension } from "../lib/queries";
import type { ExtensionNodeView, ExtensionStatus } from "../lib/types";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  EmptyState,
  ErrorState,
  Field,
  InlineCode,
  LinkButton,
  LoadingPanel,
  PageHeader,
  Textarea,
} from "../components/ui";
import { ConfirmDialog, Modal } from "../components/Modal";
import { useToast } from "../components/Toast";

function statusBadge(status: ExtensionStatus | null | undefined) {
  if (!status) return <Badge tone="neutral">sem status</Badge>;
  const tone =
    status.state === "running"
      ? "ok"
      : status.state === "starting"
        ? "warn"
        : status.state === "error"
          ? "danger"
          : "neutral";
  return (
    <Badge tone={tone} dot title={status.message ?? undefined}>
      {status.state}
    </Badge>
  );
}

function configPreview(value: unknown): string {
  if (value == null) return "—";
  try {
    const text = JSON.stringify(value);
    return text.length > 120 ? `${text.slice(0, 117)}…` : text;
  } catch {
    return String(value);
  }
}

function initialConfigText(node: ExtensionNodeView, example: unknown): string {
  const value = node.config != null && Object.keys(node.config as object).length > 0 ? node.config : example;
  if (value == null || Object.keys(value as object).length === 0) return "{}";
  return JSON.stringify(value, null, 2);
}

export function ExtensionDetailPage() {
  const { extensionId = "" } = useParams<{ extensionId: string }>();
  const queryClient = useQueryClient();
  const { push } = useToast();
  const extensionQuery = useExtension(extensionId);
  const [editing, setEditing] = useState<ExtensionNodeView | null>(null);
  const [enabled, setEnabled] = useState(true);
  const [configText, setConfigText] = useState("{}");
  const [formError, setFormError] = useState<string | null>(null);
  const [removeTarget, setRemoveTarget] = useState<ExtensionNodeView | null>(null);

  const extension = extensionQuery.data;

  useEffect(() => {
    if (extension) document.title = `${extension.name} · wired-magic`;
  }, [extension]);

  const exampleText = useMemo(() => {
    if (extension?.config_example == null) return "";
    try {
      return JSON.stringify(extension.config_example, null, 2);
    } catch {
      return "";
    }
  }, [extension?.config_example]);

  const saveMutation = useMutation({
    mutationFn: ({ node, body }: { node: ExtensionNodeView; body: unknown }) =>
      api.putExtensionNodeConfig(extensionId, node.node_id, { enabled, config: body }),
    onSuccess: () => {
      invalidateExtensionQueries(queryClient);
      push({ tone: "ok", title: "Configuração salva", description: "O agente aplica no próximo mapa." });
      setEditing(null);
    },
    onError: (error) =>
      push({ tone: "error", title: "Falha ao salvar", description: errorMessage(error) }),
  });

  const removeMutation = useMutation({
    mutationFn: (node: ExtensionNodeView) =>
      api.deleteExtensionNodeConfig(extensionId, node.node_id),
    onSuccess: () => {
      invalidateExtensionQueries(queryClient);
      push({ tone: "ok", title: "Configuração removida", description: "O sidecar será reconciliado." });
      setRemoveTarget(null);
    },
    onError: (error) =>
      push({ tone: "error", title: "Falha ao remover", description: errorMessage(error) }),
  });

  const backLink = (
    <Link to="/extensions" className="inline-flex items-center gap-1 text-xs text-muted hover:text-ink">
      <ArrowLeft aria-hidden="true" className="size-3.5" />
      Extensões
    </Link>
  );

  if (extensionQuery.isLoading) {
    return (
      <>
        {backLink}
        <LoadingPanel label="Lendo configuração da extensão…" rows={4} />
      </>
    );
  }

  if (extensionQuery.isError || !extension) {
    return (
      <>
        {backLink}
        <ErrorState error={extensionQuery.error} onRetry={() => extensionQuery.refetch()} />
      </>
    );
  }

  const nodes = extension.nodes ?? [];
  const configured = nodes.filter((node) => node.enabled).length;
  const running = nodes.filter((node) => node.status?.state === "running").length;

  function openEditor(node: ExtensionNodeView) {
    setEditing(node);
    setEnabled(node.enabled);
    setConfigText(initialConfigText(node, extension?.config_example));
    setFormError(null);
  }

  function submitEditor() {
    if (!editing) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(configText);
    } catch (error) {
      setFormError(error instanceof Error ? `JSON inválido: ${error.message}` : "JSON inválido");
      return;
    }
    setFormError(null);
    saveMutation.mutate({ node: editing, body: parsed });
  }

  return (
    <>
      {backLink}
      <PageHeader
        eyebrow="Extensão"
        title={extension.name}
        description={extension.description || "Sem descrição."}
        actions={
          <>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => extensionQuery.refetch()}
              loading={extensionQuery.isFetching}
            >
              <RefreshCw aria-hidden="true" className="size-3.5" />
              Atualizar
            </Button>
            <LinkButton to="/extensions" variant="secondary" size="sm">
              Todas as extensões
            </LinkButton>
          </>
        }
      />

      <div className="mb-4 grid grid-cols-2 gap-2.5 sm:grid-cols-4">
        <div className="panel p-3">
          <p className="text-[10px] uppercase tracking-[0.14em] text-faint">Driver</p>
          <p className="mt-1 font-mono text-sm text-ink">{extension.node_driver}</p>
        </div>
        <div className="panel p-3">
          <p className="text-[10px] uppercase tracking-[0.14em] text-faint">Versão</p>
          <p className="mt-1 font-mono text-sm text-ink">v{extension.version}</p>
        </div>
        <div className="panel p-3">
          <p className="text-[10px] uppercase tracking-[0.14em] text-faint">Configurados</p>
          <p className="mt-1 text-sm text-ink">{configured} nó(s)</p>
        </div>
        <div className="panel p-3">
          <p className="text-[10px] uppercase tracking-[0.14em] text-faint">Ativos</p>
          <p className="mt-1 text-sm text-ink">{running} nó(s)</p>
        </div>
      </div>

      {extension.node_driver === "exec" ? (
        <div className="mb-4 flex items-start gap-2.5 rounded-lg border border-warn/30 bg-warn/[0.06] px-3.5 py-3">
          <TriangleAlert aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warn" />
          <div className="text-xs leading-5 text-muted">
            <p className="font-medium text-warn">O driver exec roda como root no nó</p>
            <p className="mt-0.5">
              Configure apenas binários confiáveis. Os campos usuais do JSON são{" "}
              <InlineCode>command</InlineCode>, <InlineCode>args</InlineCode>, <InlineCode>env</InlineCode>,{" "}
              <InlineCode>working_dir</InlineCode>, <InlineCode>public_urls</InlineCode> e{" "}
              <InlineCode>restart</InlineCode> (always, on_failure, never).
            </p>
          </div>
        </div>
      ) : null}

      {extension.node_driver === "none" ? (
        <div className="mb-4 rounded-lg border border-info/25 bg-info/[0.04] px-3.5 py-3 text-xs leading-5 text-muted">
          Esta extensão não tem componente no nó; a configuração fica apenas no control plane.
        </div>
      ) : null}

      <Card>
        <CardHeader
          title="Configuração por nó"
          description="O JSON é validado pelo driver antes de ser gravado."
          actions={<Badge tone="neutral">{nodes.length} nó(s)</Badge>}
        />
        {nodes.length === 0 ? (
          <EmptyState icon={<TriangleAlert aria-hidden="true" className="size-5" />} title="Nenhum nó na malha" description="Inscreva um nó para configurar esta extensão." />
        ) : (
          <div className="overflow-x-auto">
            <table className="data-table min-w-[860px]">
              <caption>Configuração da extensão por nó</caption>
              <thead>
                <tr>
                  <th scope="col">Nó</th>
                  <th scope="col">Habilitada</th>
                  <th scope="col">Revisão</th>
                  <th scope="col">Status</th>
                  <th scope="col">Config</th>
                  <th scope="col">Atualizado</th>
                  <th scope="col" className="text-right">Ações</th>
                </tr>
              </thead>
              <tbody>
                {nodes.map((node) => (
                  <tr key={node.node_id}>
                    <td>
                      <p className="font-medium text-ink">{node.node_name}</p>
                      <p className="font-mono text-[10px] text-faint">{node.overlay_ip}</p>
                    </td>
                    <td>{node.enabled ? <Badge tone="ok">sim</Badge> : <Badge>não</Badge>}</td>
                    <td className="font-mono text-xs text-muted">r{node.revision}</td>
                    <td>
                      {statusBadge(node.status)}
                      {node.status?.message ? (
                        <p className="mt-0.5 max-w-[220px] text-[10px] leading-4 text-faint">{node.status.message}</p>
                      ) : null}
                    </td>
                    <td>
                      <code className="break-all font-mono text-[10px] text-muted">{configPreview(node.config)}</code>
                    </td>
                    <td className="text-[10px] text-faint">{formatDateTime(node.updated_at)}</td>
                    <td className="text-right">
                      <div className="flex justify-end gap-1.5">
                        <Button size="sm" variant="secondary" onClick={() => openEditor(node)}>
                          <Save aria-hidden="true" className="size-3.5" />
                          Editar
                        </Button>
                        {node.enabled || node.config != null ? (
                          <Button size="sm" variant="danger" onClick={() => setRemoveTarget(node)}>
                            <Trash2 aria-hidden="true" className="size-3.5" />
                            Remover
                          </Button>
                        ) : null}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      <Modal
        open={editing != null}
        onClose={() => setEditing(null)}
        title={`Configurar ${extension.name}`}
        description={editing ? `${editing.node_name} · ${editing.overlay_ip}` : undefined}
        size="lg"
        footer={
          <>
            <Button variant="ghost" onClick={() => setEditing(null)}>
              Cancelar
            </Button>
            <Button onClick={submitEditor} loading={saveMutation.isPending}>
              Salvar configuração
            </Button>
          </>
        }
      >
        <div className="space-y-3">
          <label className="flex items-center gap-2 text-xs text-muted">
            <input
              type="checkbox"
              checked={enabled}
              onChange={(event) => setEnabled(event.target.checked)}
              className="size-3.5 accent-accent"
            />
            Habilitada neste nó
          </label>
          <Field
            label="Configuração (JSON)"
            htmlFor="extension-config"
            hint={exampleText ? "O exemplo do manifesto é usado como ponto de partida." : undefined}
            error={formError}
          >
            <Textarea
              id="extension-config"
              className="min-h-64 font-mono text-[11px]"
              spellCheck={false}
              value={configText}
              onChange={(event) => setConfigText(event.target.value)}
            />
          </Field>
          {exampleText ? (
            <details className="rounded border border-line bg-black/20 p-2.5">
              <summary className="cursor-pointer text-[11px] text-muted">Ver exemplo do manifesto</summary>
              <pre className="mt-2 overflow-x-auto font-mono text-[10px] leading-4 text-faint">{exampleText}</pre>
            </details>
          ) : null}
        </div>
      </Modal>

      <ConfirmDialog
        open={removeTarget != null}
        onClose={() => setRemoveTarget(null)}
        onConfirm={() => {
          if (removeTarget) removeMutation.mutate(removeTarget);
        }}
        title="Remover configuração"
        description={
          <>
            Remover a configuração de <strong className="text-ink">{extension.name}</strong> em{" "}
            <strong className="text-ink">{removeTarget?.node_name}</strong>? O sidecar será parado no próximo mapa.
          </>
        }
        confirmLabel="Remover"
        pending={removeMutation.isPending}
        error={removeMutation.error}
      />

      <p className="mt-4 text-[10px] leading-4 text-faint">
        O nó vê esta extensão em <InlineCode>/api/v1/map</InlineCode> e reporta o estado em{" "}
        <InlineCode>/api/v1/node/report</InlineCode>. Este painel não acessa o nó diretamente.
      </p>
    </>
  );
}
