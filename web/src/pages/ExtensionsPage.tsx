import { Link } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  ArrowRight,
  Box,
  Cloud,
  Puzzle,
  RefreshCw,
  Terminal,
} from "lucide-react";
import { api } from "../lib/api";
import { errorMessage } from "../lib/errors";
import { invalidateExtensionQueries, useExtensions } from "../lib/queries";
import type { ExtensionView } from "../lib/types";
import {
  Badge,
  Button,
  Card,
  ErrorState,
  InlineCode,
  LinkButton,
  LoadingPanel,
  PageHeader,
} from "../components/ui";
import { useToast } from "../components/Toast";

function extensionIcon(extension: ExtensionView) {
  if (extension.node_driver === "cloudflared") return <Cloud aria-hidden="true" className="size-4" />;
  if (extension.node_driver === "exec") return <Terminal aria-hidden="true" className="size-4" />;
  return <Box aria-hidden="true" className="size-4" />;
}

function extensionPath(extension: ExtensionView): string {
  return `/extensions/${encodeURIComponent(extension.id)}`;
}

function ExtensionCard({ extension }: { extension: ExtensionView }) {
  return (
    <Card className="flex flex-col">
      <div className="flex items-start justify-between gap-3 p-4 pb-2">
        <div className="flex min-w-0 items-start gap-2.5">
          <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-md border border-accent/20 bg-accent/[0.07] text-accent">
            {extensionIcon(extension)}
          </span>
          <div className="min-w-0">
            <h2 className="truncate text-sm font-semibold text-ink">{extension.name}</h2>
            <p className="truncate font-mono text-[10px] text-faint">
              {extension.id} · v{extension.version}
            </p>
          </div>
        </div>
        <Badge tone={extension.builtin ? "accent" : "neutral"}>
          {extension.builtin ? "embutida" : "declarativa"}
        </Badge>
      </div>

      <p className="min-h-10 px-4 text-xs leading-5 text-muted">
        {extension.description || "Sem descrição."}
      </p>

      <div className="flex flex-wrap gap-1.5 px-4 py-3">
        <Badge tone={extension.node_driver === "none" ? "neutral" : "info"}>
          driver {extension.node_driver}
        </Badge>
        <Badge>{extension.nodes_configured} configurado(s)</Badge>
        {extension.nodes_running > 0 ? (
          <Badge tone="ok" dot>
            {extension.nodes_running} ativo(s)
          </Badge>
        ) : (
          <Badge tone="neutral" dot>
            parada
          </Badge>
        )}
      </div>

      {extension.warning ? (
        <p role="status" className="mx-4 mb-3 rounded border border-warn/30 bg-warn/[0.06] px-2 py-1.5 text-[11px] leading-4 text-warn">
          {extension.warning}
        </p>
      ) : null}

      <div className="mt-auto flex items-center justify-between gap-2 border-t border-line px-4 py-3">
        <span className="text-[10px] text-faint">
          {extension.builtin ? "código no control plane" : "config por nó"}
        </span>
        <LinkButton to={extensionPath(extension)} variant="secondary" size="sm">
          {extension.id === "cloudflared" ? "Gerenciar rotas" : "Configurar"}
          <ArrowRight aria-hidden="true" className="size-3.5" />
        </LinkButton>
      </div>
    </Card>
  );
}

export function ExtensionsPage() {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const extensionsQuery = useExtensions();

  const reloadMutation = useMutation({
    mutationFn: api.reloadExtensions,
    onSuccess: (result) => {
      invalidateExtensionQueries(queryClient);
      push({
        tone: result.warnings.length > 0 ? "info" : "ok",
        title: result.warnings.length > 0 ? "Manifestos recarregados com avisos" : "Manifestos recarregados",
        description:
          result.warnings.length > 0
            ? result.warnings.slice(0, 3).join(" · ")
            : `${result.manifests.length} extensão(ões) disponíveis.`,
      });
    },
    onError: (error) =>
      push({ tone: "error", title: "Falha ao recarregar", description: errorMessage(error) }),
  });

  const header = (
    <PageHeader
      eyebrow="Sistema"
      title="Extensões"
      description="Funcionalidades plugáveis no control plane e nos nós. O estado desejado viaja no mapa; o observado volta no report do agente."
      actions={
        <>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => extensionsQuery.refetch()}
            loading={extensionsQuery.isFetching}
          >
            <RefreshCw aria-hidden="true" className="size-3.5" />
            Atualizar
          </Button>
          <Button
            size="sm"
            onClick={() => reloadMutation.mutate()}
            loading={reloadMutation.isPending}
          >
            <RefreshCw aria-hidden="true" className="size-3.5" />
            Recarregar manifestos
          </Button>
        </>
      }
    />
  );

  if (extensionsQuery.isLoading) {
    return (
      <>
        {header}
        <LoadingPanel label="Lendo manifestos e estado das extensões…" rows={3} />
      </>
    );
  }

  if (extensionsQuery.isError || !extensionsQuery.data) {
    return (
      <>
        {header}
        <ErrorState error={extensionsQuery.error} onRetry={() => extensionsQuery.refetch()} />
      </>
    );
  }

  const extensions = extensionsQuery.data;

  return (
    <>
      {header}

      <div className="mb-4 flex items-start gap-2.5 rounded-lg border border-info/25 bg-info/[0.04] px-3.5 py-3">
        <Puzzle aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-info" />
        <div className="min-w-0 text-xs leading-5 text-muted">
          <p className="font-medium text-ink">Runtime declarativo + drivers embutidos</p>
          <p className="mt-0.5">
            Para adicionar uma extensão sem recompilar, crie{" "}
            <InlineCode>&lt;data_dir&gt;/extensions/&lt;id&gt;/extension.toml</InlineCode> com{" "}
            <InlineCode>node_driver = &quot;exec&quot;</InlineCode> e recarregue os manifestos. Extensões
            com lógica própria (como o cloudflared) registram um <InlineCode>ControlExtension</InlineCode> no
            control plane.
          </p>
        </div>
      </div>

      {extensions.length === 0 ? (
        <Card>
          <div className="flex flex-col items-center gap-2 px-4 py-12 text-center">
            <Puzzle aria-hidden="true" className="size-6 text-faint" />
            <p className="text-sm font-medium text-ink">Nenhuma extensão carregada</p>
            <p className="max-w-md text-xs leading-5 text-muted">
              O control plane semeia o manifesto do cloudflared no primeiro boot. Verifique a pasta de
              extensões e recarregue os manifestos.
            </p>
          </div>
        </Card>
      ) : (
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
          {extensions.map((extension) => (
            <ExtensionCard key={extension.id} extension={extension} />
          ))}
        </div>
      )}

      <p className="mt-4 text-[10px] leading-4 text-faint">
        Dica: <Link to="/nodes" className="text-muted underline-offset-2 hover:text-ink hover:underline">nós</Link>{" "}
        precisam estar online para aplicar a configuração. O agente reconcilia apenas quando a revisão ou o
        config mudam, sem reiniciar sidecars à toa.
      </p>
    </>
  );
}
