import { useMemo, useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  CheckCircle2,
  Code2,
  Eye,
  EyeOff,
  KeyRound,
  Plug,
  Plus,
  RefreshCw,
  ShieldCheck,
  Terminal,
  Trash2,
  X,
} from "lucide-react";
import { api } from "../lib/api";
import { defaultControlUrl, normalizeBaseUrl } from "../lib/env";
import { errorMessage } from "../lib/errors";
import { formatDateTime, formatRelative } from "../lib/format";
import { useServiceTokens } from "../lib/queries";
import type { IntegrationNodesResponse, ServiceTokenView } from "../lib/types";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  CopyButton,
  EmptyState,
  ErrorState,
  Field,
  Input,
  LoadingPanel,
  PageHeader,
  cn,
} from "../components/ui";
import { ConfirmDialog } from "../components/Modal";
import { useToast } from "../components/Toast";

const SCOPE = "integration:nodes:read";

const goExample = [
  "package main",
  "",
  "import (",
  '  "encoding/json"',
  '  "fmt"',
  '  "net/http"',
  '  "os"',
  ")",
  "",
  "type node struct {",
  '  Name      string            `json:"name"`',
  '  Hostname  string            `json:"hostname"`',
  '  OverlayIP string            `json:"overlay_ip"`',
  '  Tags      []string          `json:"tags"`',
  '  Labels    map[string]string `json:"labels"`',
  '  Online    bool              `json:"online"`',
  "}",
  "",
  "type response struct {",
  '  Nodes []node `json:"nodes"`',
  "}",
  "",
  "func main() {",
  '  url := os.Getenv("WM_CONTROL") + "/api/v1/integration/nodes?online=true"',
  '  req, err := http.NewRequest("GET", url, nil)',
  "  if err != nil {",
  "    panic(err)",
  "  }",
  '  req.Header.Set("Authorization", "Bearer "+os.Getenv("WM_SERVICE_TOKEN"))',
  "",
  "  res, err := http.DefaultClient.Do(req)",
  "  if err != nil {",
  "    panic(err)",
  "  }",
  "  defer res.Body.Close()",
  "  if res.StatusCode != http.StatusOK {",
  "    panic(res.Status)",
  "  }",
  "",
  "  var payload response",
  "  if err := json.NewDecoder(res.Body).Decode(&payload); err != nil {",
  "    panic(err)",
  "  }",
  "  for _, n := range payload.Nodes {",
  '    port := n.Labels["baguette_port"]',
  '    fmt.Printf("%s -> http://%s:%s (online=%v)\\n", n.Name, n.OverlayIP, port, n.Online)',
  "  }",
  "}",
].join("\n");

const sampleJson = `{
  "nodes": [
    {
      "id": "1f0b…",
      "name": "mac-ana",
      "hostname": "MacBook-Pro-de-Ana.local",
      "overlay_ip": "10.66.0.3",
      "tags": ["mac", "seance"],
      "labels": { "seance": "true", "baguette_port": "8421" },
      "online": true,
      "last_seen": "2026-11-12T23:40:00Z"
    }
  ],
  "server_time": "2026-11-12T23:41:00Z"
}`;

type ExampleTab = "curl" | "go" | "json";

export function IntegrationPage() {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const tokensQuery = useServiceTokens();

  const [name, setName] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [created, setCreated] = useState<ServiceTokenView | null>(null);
  const [revokeTarget, setRevokeTarget] = useState<ServiceTokenView | null>(null);
  const [revokedIds, setRevokedIds] = useState<Set<string>>(new Set());
  const [controlUrl, setControlUrl] = useState(defaultControlUrl());
  const [tab, setTab] = useState<ExampleTab>("curl");

  const [testToken, setTestToken] = useState("");
  const [showTestToken, setShowTestToken] = useState(false);
  const [testResult, setTestResult] = useState<IntegrationNodesResponse | null>(null);
  const [testError, setTestError] = useState<string | null>(null);

  const createMutation = useMutation({
    mutationFn: api.createServiceToken,
    onSuccess: (token) => {
      setCreated(token);
      setName("");
      void queryClient.invalidateQueries({ queryKey: ["service-tokens"] });
      push({
        tone: "ok",
        title: "Service token criado",
        description: "Copie o token agora; ele não será exibido de novo.",
      });
    },
    onError: (error) => setFormError(errorMessage(error)),
  });

  const revokeMutation = useMutation({
    mutationFn: (token: ServiceTokenView) => api.revokeServiceToken(token.id),
    onSuccess: (_result, token) => {
      setRevokedIds((current) => new Set(current).add(token.id));
      void queryClient.invalidateQueries({ queryKey: ["service-tokens"] });
      push({ tone: "ok", title: "Service token revogado", description: token.name });
      setRevokeTarget(null);
    },
    onError: (error) => push({ tone: "error", title: "Falha ao revogar", description: errorMessage(error) }),
  });

  const testMutation = useMutation({
    mutationFn: (token: string) => api.integrationNodes(token),
    onSuccess: (result) => {
      setTestResult(result);
      setTestError(null);
    },
    onError: (error) => {
      setTestResult(null);
      setTestError(errorMessage(error));
    },
  });

  const tokens = useMemo(
    () => (tokensQuery.data ?? []).filter((token) => !revokedIds.has(token.id)),
    [tokensQuery.data, revokedIds],
  );

  const baseUrl = normalizeBaseUrl(controlUrl) || defaultControlUrl();

  const curlExample = `curl -fsS "${baseUrl}/api/v1/integration/nodes?online=true&tag=mac" \\
  -H "Authorization: Bearer $WM_SERVICE_TOKEN"`;

  const codeExamples: Record<ExampleTab, string> = {
    curl: curlExample,
    go: goExample,
    json: sampleJson,
  };

  function handleCreate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      setFormError("Informe um nome para identificar o consumidor.");
      return;
    }
    if (trimmed.length > 64) {
      setFormError("Use no máximo 64 caracteres.");
      return;
    }
    setFormError(null);
    createMutation.mutate({ name: trimmed, scopes: [SCOPE] });
  }

  function handleTest(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const token = testToken.trim();
    if (!token) {
      setTestError("Informe um service token para testar.");
      return;
    }
    setTestError(null);
    testMutation.mutate(token);
  }

  const header = (
    <PageHeader
      eyebrow="Política"
      title="Integração"
      description="Service tokens e API read-only para o Seance. Nenhum segredo de nó ou IP de overlay vai para o navegador do aluno."
      actions={
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void tokensQuery.refetch()}
          loading={tokensQuery.isFetching}
        >
          <RefreshCw aria-hidden="true" className="size-3.5" />
          Atualizar
        </Button>
      }
    />
  );

  if (tokensQuery.isLoading) {
    return (
      <>
        {header}
        <LoadingPanel label="Carregando service tokens…" rows={3} />
      </>
    );
  }

  if (tokensQuery.isError || !tokensQuery.data) {
    return (
      <>
        {header}
        <ErrorState error={tokensQuery.error} onRetry={() => void tokensQuery.refetch()} />
      </>
    );
  }

  return (
    <>
      {header}

      {created ? (
        <Card className="mb-4 border-accent/35 bg-accent/[0.035]">
          <CardHeader
            title={
              <span className="flex items-center gap-2">
                <CheckCircle2 aria-hidden="true" className="size-4 text-ok" />
                Service token criado — copie agora
              </span>
            }
            description="A resposta de criação é a única vez que o token bruto aparece. O control guarda apenas o hash."
            actions={
              <Button variant="ghost" size="sm" onClick={() => setCreated(null)} aria-label="Fechar token criado">
                <X aria-hidden="true" className="size-4" />
              </Button>
            }
          />
          <div className="space-y-3 p-4">
            <div>
              <div className="mb-1 flex items-center justify-between gap-2">
                <span className="text-[10px] uppercase tracking-[0.16em] text-faint">
                  {created.name} · {created.scopes.join(", ")}
                </span>
                <CopyButton text={created.token ?? ""} label="Copiar token" variant="secondary" />
              </div>
              <code className="block select-all overflow-x-auto rounded-md border border-accent/25 bg-black/40 px-3 py-2 font-mono text-xs text-accent">
                {created.token ?? ""}
              </code>
            </div>
            <p className="text-[11px] leading-5 text-warn">
              Configure este token como secret no servidor do Seance (variável de ambiente). Nunca
              inclua em bundle Next, log de navegador ou repositório.
            </p>
          </div>
        </Card>
      ) : null}

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,0.95fr)_minmax(0,1.25fr)]">
        <div className="space-y-4">
          <Card>
            <CardHeader
              title="Novo service token"
              description="Escopo fixo e read-only para a API de integração."
              icon={<KeyRound aria-hidden="true" className="size-4" />}
              actions={<Badge tone="info" className="font-mono">{SCOPE}</Badge>}
            />
            <form onSubmit={handleCreate} className="space-y-3.5 p-4" noValidate>
              <Field label="Nome do consumidor" htmlFor="service-token-name" hint="Ex.: seance-producao, seance-staging.">
                <Input
                  id="service-token-name"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  placeholder="seance-producao"
                  autoComplete="off"
                  spellCheck={false}
                  maxLength={64}
                  disabled={createMutation.isPending}
                />
              </Field>
              <div className="rounded-md border border-line bg-white/[0.02] px-3 py-2">
                <p className="text-[10px] uppercase tracking-[0.14em] text-faint">Escopo</p>
                <p className="mt-0.5 font-mono text-[11px] text-muted">{SCOPE}</p>
                <p className="mt-1 text-[10px] leading-4 text-faint">
                  A API não permite criar, remover ou alterar nós, ACL ou relay.
                </p>
              </div>
              {formError ? (
                <p role="alert" className="rounded-md border border-danger/30 bg-danger/[0.06] px-3 py-2 text-xs leading-5 text-danger">
                  {formError}
                </p>
              ) : null}
              {createMutation.isError && !formError ? (
                <p role="alert" className="rounded-md border border-danger/30 bg-danger/[0.06] px-3 py-2 text-xs leading-5 text-danger">
                  {errorMessage(createMutation.error)}
                </p>
              ) : null}
              <Button type="submit" variant="primary" className="w-full" loading={createMutation.isPending}>
                <Plus aria-hidden="true" className="size-4" />
                Gerar service token
              </Button>
            </form>
          </Card>

          <Card>
            <CardHeader
              title="Tokens de serviço"
              description={`${tokens.length} ativo(s) nesta sessão`}
              icon={<Plug aria-hidden="true" className="size-4" />}
            />
            {tokens.length === 0 ? (
              <div className="p-4">
                <EmptyState
                  icon={<Plug aria-hidden="true" className="size-4" />}
                  title="Nenhum service token ativo"
                  description="Crie um token para o servidor do Seance consumir a API read-only."
                />
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="data-table min-w-[460px]">
                  <caption>Service tokens da API de integração</caption>
                  <thead>
                    <tr>
                      <th scope="col">Token</th>
                      <th scope="col">Uso</th>
                      <th scope="col" className="text-right">Ações</th>
                    </tr>
                  </thead>
                  <tbody>
                    {tokens.map((token) => (
                      <tr key={token.id}>
                        <td className="max-w-[210px]">
                          <span className="block truncate text-[13px] font-medium text-ink" title={token.name}>
                            {token.name}
                          </span>
                          <div className="mt-1 flex flex-wrap gap-1">
                            {token.scopes.map((scope) => (
                              <Badge key={scope} tone="info" className="font-mono">
                                {scope}
                              </Badge>
                            ))}
                          </div>
                        </td>
                        <td className="whitespace-nowrap text-[10px] leading-4 text-muted">
                          <span className="block" title={formatDateTime(token.created_at)}>
                            criado {formatRelative(token.created_at)}
                          </span>
                          <span
                            className="block text-faint"
                            title={token.last_used_at ? formatDateTime(token.last_used_at) : undefined}
                          >
                            usado {token.last_used_at ? formatRelative(token.last_used_at) : "nunca"}
                          </span>
                        </td>
                        <td>
                          <div className="flex justify-end">
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => {
                                revokeMutation.reset();
                                setRevokeTarget(token);
                              }}
                              aria-label={`Revogar service token ${token.name}`}
                              title={`Revogar service token ${token.name}`}
                              className="h-7 px-2 text-[11px] text-faint hover:text-danger"
                            >
                              <Trash2 aria-hidden="true" className="size-3.5" />
                              Revogar
                            </Button>
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            <p className="border-t border-line px-4 py-2 text-[10px] leading-4 text-faint">
              Observação do contrato: a listagem não expõe <span className="font-mono">revoked_at</span>. Tokens
              revogados nesta sessão são ocultados; após recarregar, a API pode listá-los sem indicar
              o estado. Revogar de novo é inofensivo.
            </p>
          </Card>
        </div>

        <div className="space-y-4">
          <Card>
            <CardHeader
              title="API para o Seance"
              description="Servidor do Seance → control plane. Nunca o navegador do aluno."
              icon={<Code2 aria-hidden="true" className="size-4" />}
              actions={<Badge tone="ok">somente leitura</Badge>}
            />
            <div className="space-y-3 p-4">
              <div className="grid grid-cols-1 gap-2 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end">
                <Field label="URL do control nos exemplos" htmlFor="integration-control-url">
                  <Input
                    id="integration-control-url"
                    value={controlUrl}
                    onChange={(event) => setControlUrl(event.target.value)}
                    autoComplete="off"
                    spellCheck={false}
                    className="font-mono text-xs"
                  />
                </Field>
                <div className="flex h-9 items-center">
                  <Badge tone="neutral" className="font-mono">GET /api/v1/integration/nodes</Badge>
                </div>
              </div>

              <div className="flex flex-wrap items-center gap-1.5" role="tablist" aria-label="Exemplos da API de integração">
                {(
                  [
                    { id: "curl", label: "curl", icon: Terminal },
                    { id: "go", label: "Go", icon: Code2 },
                    { id: "json", label: "resposta", icon: Plug },
                  ] as const
                ).map(({ id, label, icon: Icon }) => (
                  <button
                    key={id}
                    type="button"
                    role="tab"
                    id={`integration-tab-${id}`}
                    aria-selected={tab === id}
                    aria-controls={`integration-panel-${id}`}
                    onClick={() => setTab(id)}
                    className={cn(
                      "inline-flex h-7 items-center gap-1.5 rounded-md border px-2.5 text-[11px] font-medium transition-colors",
                      tab === id
                        ? "border-accent/30 bg-accent/10 text-accent"
                        : "border-transparent text-muted hover:bg-white/[0.04] hover:text-ink",
                    )}
                  >
                    <Icon aria-hidden="true" className="size-3" />
                    {label}
                  </button>
                ))}
                <span className="ml-auto">
                  <CopyButton text={codeExamples[tab]} label="Copiar exemplo" variant="ghost" />
                </span>
              </div>

              <div
                id={`integration-panel-${tab}`}
                role="tabpanel"
                aria-labelledby={`integration-tab-${tab}`}
                tabIndex={0}
              >
                <pre className="code-block max-h-[420px] whitespace-pre-wrap break-words">{codeExamples[tab]}</pre>
              </div>

              <p className="text-[10px] leading-4 text-faint">
                Filtros suportados: <code className="text-muted">?tag=mac</code> e{" "}
                <code className="text-muted">?online=true</code>. A resposta inclui apenas nós{" "}
                <span className="text-muted">active</span> e nunca retorna chave WireGuard, endpoints,
                tokens ou NAT.
              </p>
            </div>
          </Card>

          <Card>
            <CardHeader
              title="Mapeamento no Seance"
              description="Campos estáveis para montar o acesso do operador ao Baguette."
              icon={<Plug aria-hidden="true" className="size-4" />}
            />
            <div className="space-y-2.5 px-4 py-3 text-[11px] leading-5">
              <div className="grid grid-cols-[145px_minmax(0,1fr)] gap-2">
                <code className="font-mono text-accent">name</code>
                <p className="text-muted">Identificador do nó no Seance; combine com o aluno/operador.</p>
              </div>
              <div className="grid grid-cols-[145px_minmax(0,1fr)] gap-2">
                <code className="font-mono text-accent">labels.seance</code>
                <p className="text-muted">Marcador opcional (“true”) para filtrar nós do estúdio.</p>
              </div>
              <div className="grid grid-cols-[145px_minmax(0,1fr)] gap-2">
                <code className="font-mono text-accent">labels.baguette_port</code>
                <p className="text-muted">
                  Porta do Baguette no nó, tipicamente <span className="font-mono text-ink">8421</span>.
                  O Seance monta <span className="font-mono text-ink">http://&lt;overlay_ip&gt;:&lt;porta&gt;</span>{" "}
                  server-side.
                </p>
              </div>
              <div className="grid grid-cols-[145px_minmax(0,1fr)] gap-2">
                <code className="font-mono text-accent">overlay_ip</code>
                <p className="text-muted">Host de overlay do Mac; visível só para o operador no servidor.</p>
              </div>
              <div className="grid grid-cols-[145px_minmax(0,1fr)] gap-2">
                <code className="font-mono text-accent">online / last_seen</code>
                <p className="text-muted">Presença para indicar se o Baguette está alcançável agora.</p>
              </div>
            </div>
            <div className="mx-4 mb-3 flex items-start gap-2 rounded-md border border-warn/25 bg-warn/[0.05] px-3 py-2">
              <ShieldCheck aria-hidden="true" className="mt-0.5 size-3.5 shrink-0 text-warn" />
              <p className="text-[10px] leading-4 text-muted">
                Use o token no servidor do Seance, com allowlist de IP e TLS. O IP de overlay é um
                endpoint de operador; não o exponha ao navegador do aluno.
              </p>
            </div>
          </Card>

          <Card>
            <CardHeader
              title="Testar token"
              description="Chamada direta ao endpoint de integração, sem cookie de admin."
              icon={<Terminal aria-hidden="true" className="size-4" />}
            />
            <form onSubmit={handleTest} className="space-y-3 p-4" noValidate>
              <Field
                label="Service token"
                htmlFor="integration-test-token"
                hint="Fica apenas no estado deste formulário; não é salvo."
              >
                <div className="relative">
                  <Input
                    id="integration-test-token"
                    type={showTestToken ? "text" : "password"}
                    value={testToken}
                    onChange={(event) => setTestToken(event.target.value)}
                    autoComplete="off"
                    spellCheck={false}
                    placeholder="wms_…"
                    disabled={testMutation.isPending}
                    className="pr-10 font-mono text-xs"
                  />
                  <button
                    type="button"
                    onClick={() => setShowTestToken((current) => !current)}
                    aria-label={showTestToken ? "Ocultar token" : "Mostrar token"}
                    className="absolute right-1 top-1/2 -translate-y-1/2 rounded p-1.5 text-faint hover:bg-white/5 hover:text-ink"
                  >
                    {showTestToken ? <EyeOff aria-hidden="true" className="size-3.5" /> : <Eye aria-hidden="true" className="size-3.5" />}
                  </button>
                </div>
              </Field>
              <Button type="submit" variant="secondary" className="w-full" loading={testMutation.isPending}>
                <Terminal aria-hidden="true" className="size-3.5" />
                Chamar /integration/nodes
              </Button>

              {testError ? (
                <p role="alert" className="rounded-md border border-danger/30 bg-danger/[0.06] px-3 py-2 text-xs leading-5 text-danger">
                  {testError}
                </p>
              ) : null}
              {testResult ? (
                <div className="rounded-md border border-ok/25 bg-ok/[0.05] px-3 py-2">
                  <p className="text-xs font-medium text-ok">
                    {testResult.nodes.length} nó(s) retornado(s)
                  </p>
                  <p className="mt-0.5 font-mono text-[10px] leading-4 text-muted">
                    {testResult.nodes.slice(0, 4).map((node) => node.name).join(", ") || "lista vazia"} ·{" "}
                    {formatRelative(testResult.server_time)}
                  </p>
                </div>
              ) : null}

              <p className="flex items-start gap-1.5 text-[10px] leading-4 text-faint">
                <AlertTriangle aria-hidden="true" className="mt-0.5 size-3 shrink-0" />
                Token sem o escopo retorna 403; revogado retorna 401. A chamada não renova a sessão do
                painel.
              </p>
            </form>
          </Card>
        </div>
      </div>

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
        title="Revogar service token"
        description={
          revokeTarget ? (
            <>
              O token <strong className="text-ink">{revokeTarget.name}</strong> deixa de autenticar
              imediatamente na API de integração.
            </>
          ) : null
        }
        confirmLabel="Revogar token"
        pending={revokeMutation.isPending}
        error={revokeMutation.error}
      >
        <p className="text-[11px] leading-5 text-muted">
          O Seance passará a receber 401 até que um novo token seja configurado. A ação fica na
          auditoria.
        </p>
      </ConfirmDialog>
    </>
  );
}
