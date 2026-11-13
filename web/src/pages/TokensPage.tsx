import { useMemo, useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  CheckCircle2,
  Clock,
  Copy,
  KeyRound,
  RefreshCw,
  Terminal,
  Ticket,
  Trash2,
  X,
} from "lucide-react";
import { api } from "../lib/api";
import { defaultControlUrl, normalizeBaseUrl } from "../lib/env";
import { errorMessage } from "../lib/errors";
import { formatDateTime, formatRelative } from "../lib/format";
import { useJoinTokens } from "../lib/queries";
import type { JoinTokenView } from "../lib/types";
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
  Select,
  cn,
} from "../components/ui";
import { ConfirmDialog } from "../components/Modal";
import { useToast } from "../components/Toast";

type TtlUnit = "hours" | "days";

interface TokenState {
  label: string;
  tone: "ok" | "warn" | "danger" | "neutral";
}

function tokenState(token: JoinTokenView, now = Date.now()): TokenState {
  if (token.revoked_at) return { label: "revogado", tone: "danger" };
  const expires = new Date(token.expires_at).getTime();
  if (!Number.isNaN(expires) && expires <= now) return { label: "expirado", tone: "warn" };
  if (token.uses >= token.max_uses) return { label: "esgotado", tone: "warn" };
  return { label: "ativo", tone: "ok" };
}

function UsageBar({ uses, max }: { uses: number; max: number }) {
  const percentage = max > 0 ? Math.min(100, Math.round((uses / max) * 100)) : 0;
  return (
    <div className="w-24">
      <div className="flex items-baseline justify-between font-mono text-[10px] text-muted">
        <span>
          {uses}/{max}
        </span>
        <span className="text-faint">{percentage}%</span>
      </div>
      <div className="mt-1 h-1 overflow-hidden rounded-full bg-white/[0.06]" aria-hidden="true">
        <div
          className={cn("h-full rounded-full", percentage >= 100 ? "bg-warn" : "bg-accent/70")}
          style={{ width: `${percentage}%` }}
        />
      </div>
    </div>
  );
}

export function TokensPage() {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const tokensQuery = useJoinTokens();

  const [name, setName] = useState("convite");
  const [ttlValue, setTtlValue] = useState("24");
  const [ttlUnit, setTtlUnit] = useState<TtlUnit>("hours");
  const [maxUses, setMaxUses] = useState("1");
  const [tags, setTags] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [created, setCreated] = useState<JoinTokenView | null>(null);
  const [controlUrl, setControlUrl] = useState(defaultControlUrl());
  const [revokeTarget, setRevokeTarget] = useState<JoinTokenView | null>(null);

  const createMutation = useMutation({
    mutationFn: api.createJoinToken,
    onSuccess: (token) => {
      setCreated(token);
      void queryClient.invalidateQueries({ queryKey: ["join-tokens"] });
      void queryClient.invalidateQueries({ queryKey: ["audit"] });
      push({ tone: "ok", title: "Token criado", description: "Copie o token agora; ele não será mostrado de novo." });
      setName("convite");
      setTags("");
      setMaxUses("1");
    },
  });

  const revokeMutation = useMutation({
    mutationFn: (token: JoinTokenView) => api.revokeJoinToken(token.id),
    onSuccess: (_result, token) => {
      push({ tone: "ok", title: "Token revogado", description: token.name });
      void queryClient.invalidateQueries({ queryKey: ["join-tokens"] });
      void queryClient.invalidateQueries({ queryKey: ["audit"] });
      setRevokeTarget(null);
    },
    onError: (error) => push({ tone: "error", title: "Falha ao revogar", description: errorMessage(error) }),
  });

  const activeCount = useMemo(
    () => (tokensQuery.data ?? []).filter((token) => tokenState(token).tone === "ok").length,
    [tokensQuery.data],
  );

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName || trimmedName.length > 64) {
      setFormError("Informe um nome de 1 a 64 caracteres.");
      return;
    }
    const ttlNumber = Number(ttlValue);
    if (!Number.isFinite(ttlNumber) || ttlNumber < 1) {
      setFormError("Informe uma validade maior que zero.");
      return;
    }
    const ttlSecs = Math.round(ttlNumber * (ttlUnit === "hours" ? 3600 : 86_400));
    if (ttlSecs < 60 || ttlSecs > 30 * 24 * 3600) {
      setFormError("A validade deve ficar entre 1 minuto e 30 dias.");
      return;
    }
    const uses = Number(maxUses);
    if (!Number.isInteger(uses) || uses < 1 || uses > 1000) {
      setFormError("Usos deve ser um inteiro entre 1 e 1000.");
      return;
    }
    const parsedTags = Array.from(
      new Set(
        tags
          .split(/[,\n]/)
          .map((tag) => tag.trim())
          .filter(Boolean),
      ),
    );
    setFormError(null);
    createMutation.mutate({
      name: trimmedName,
      ttl_secs: ttlSecs,
      max_uses: uses,
      tags: parsedTags,
      labels: {},
    });
  }

  const command =
    created?.token != null
      ? `wm join ${normalizeBaseUrl(controlUrl) || defaultControlUrl()} --token ${created.token}`
      : "";

  const header = (
    <PageHeader
      eyebrow="Rede"
      title="Tokens de entrada"
      description="Convites de uso único ou controlado para `wm join`. O control guarda apenas o hash do token."
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
        <LoadingPanel label="Carregando tokens…" rows={3} />
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

  const tokens = tokensQuery.data;

  return (
    <>
      {header}

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,360px)_minmax(0,1fr)]">
        <div className="space-y-4">
          <Card>
            <CardHeader
              title="Novo token"
              description="O token bruto aparece uma única vez, na resposta de criação."
              icon={<KeyRound aria-hidden="true" className="size-4" />}
            />
            <form onSubmit={handleSubmit} className="space-y-3.5 p-4" noValidate>
              <Field label="Nome do convite" htmlFor="token-name" hint="Rótulo administrativo; não é o nome do nó.">
                <Input
                  id="token-name"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  placeholder="convite-mac-ana"
                  autoComplete="off"
                  spellCheck={false}
                  maxLength={64}
                  disabled={createMutation.isPending}
                />
              </Field>

              <div className="grid grid-cols-[minmax(0,1fr)_100px] gap-2">
                <Field label="Validade" htmlFor="token-ttl">
                  <Input
                    id="token-ttl"
                    type="number"
                    min={1}
                    step={1}
                    value={ttlValue}
                    onChange={(event) => setTtlValue(event.target.value)}
                    inputMode="numeric"
                    disabled={createMutation.isPending}
                  />
                </Field>
                <Field label="Unidade" htmlFor="token-ttl-unit">
                  <Select
                    id="token-ttl-unit"
                    value={ttlUnit}
                    onChange={(event) => setTtlUnit(event.target.value as TtlUnit)}
                    disabled={createMutation.isPending}
                  >
                    <option value="hours">horas</option>
                    <option value="days">dias</option>
                  </Select>
                </Field>
              </div>
              <p className="-mt-2 text-[11px] text-faint">Mínimo 1 minuto · máximo 30 dias (o backend aplica clamp).</p>

              <Field label="Usos máximos" htmlFor="token-uses" hint="1 = uso único. Cada enrollment consome um uso.">
                <Input
                  id="token-uses"
                  type="number"
                  min={1}
                  max={1000}
                  step={1}
                  value={maxUses}
                  onChange={(event) => setMaxUses(event.target.value)}
                  inputMode="numeric"
                  disabled={createMutation.isPending}
                />
              </Field>

              <Field
                label="Tags do nó"
                htmlFor="token-tags"
                hint="Aplicadas aos nós criados com este token. Separe por vírgula."
              >
                <Input
                  id="token-tags"
                  value={tags}
                  onChange={(event) => setTags(event.target.value)}
                  placeholder="operator, mac"
                  autoComplete="off"
                  spellCheck={false}
                  disabled={createMutation.isPending}
                />
              </Field>

              {formError || createMutation.isError ? (
                <p role="alert" className="rounded-md border border-danger/30 bg-danger/[0.07] px-3 py-2 text-xs leading-5 text-danger">
                  {formError ?? errorMessage(createMutation.error)}
                </p>
              ) : null}

              <Button type="submit" variant="primary" className="w-full" loading={createMutation.isPending}>
                <Ticket aria-hidden="true" className="size-4" />
                Gerar token
              </Button>
            </form>
          </Card>

          <div className="rounded-lg border border-line bg-white/[0.02] px-3.5 py-3">
            <p className="flex items-center gap-1.5 text-[11px] font-medium text-ink">
              <Terminal aria-hidden="true" className="size-3.5 text-accent" />
              Como o nó entra
            </p>
            <p className="mt-1 text-[11px] leading-5 text-muted">
              O convidado roda o comando abaixo em um Mac ou servidor. Sem acesso ao painel, sem
              credencial de admin.
            </p>
            <code className="mt-2 block overflow-x-auto rounded border border-line bg-black/30 px-2 py-1.5 font-mono text-[10px] text-accent">
              wm join &lt;url-do-control&gt; --token &lt;token&gt;
            </code>
          </div>
        </div>

        <div className="space-y-4">
          {created ? (
            <Card className="border-accent/35 bg-accent/[0.035]">
              <CardHeader
                title={
                  <span className="flex items-center gap-2">
                    <CheckCircle2 aria-hidden="true" className="size-4 text-ok" />
                    Token criado — copie agora
                  </span>
                }
                description="Esta é a única vez que o token bruto aparece. O servidor guarda somente o hash."
                actions={
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setCreated(null)}
                    aria-label="Fechar token criado"
                  >
                    <X aria-hidden="true" className="size-4" />
                  </Button>
                }
              />
              <div className="space-y-3 p-4">
                <div>
                  <div className="mb-1 flex items-center justify-between gap-2">
                    <span className="text-[10px] uppercase tracking-[0.16em] text-faint">Token</span>
                    <CopyButton text={created.token ?? ""} label="Copiar token" variant="secondary" />
                  </div>
                  <code className="block select-all overflow-x-auto rounded-md border border-accent/25 bg-black/40 px-3 py-2 font-mono text-xs text-accent">
                    {created.token ?? ""}
                  </code>
                </div>

                <div className="grid grid-cols-1 gap-2 sm:grid-cols-[minmax(0,1fr)_140px]">
                  <Field label="URL do control nos exemplos" htmlFor="token-control-url">
                    <Input
                      id="token-control-url"
                      value={controlUrl}
                      onChange={(event) => setControlUrl(event.target.value)}
                      placeholder="https://mesh.exemplo.com"
                      autoComplete="off"
                      spellCheck={false}
                      className="font-mono text-xs"
                    />
                  </Field>
                  <div className="flex items-end">
                    <div className="w-full rounded-md border border-line bg-white/[0.02] px-2.5 py-2 text-[10px] leading-4 text-faint">
                      <p className="uppercase tracking-[0.12em]">Resumo</p>
                      <p className="mt-0.5 font-mono text-muted">{formatRelative(created.expires_at)}</p>
                      <p className="font-mono text-faint">{created.max_uses} uso(s)</p>
                    </div>
                  </div>
                </div>

                <div>
                  <div className="mb-1 flex items-center justify-between gap-2">
                    <span className="text-[10px] uppercase tracking-[0.16em] text-faint">Comando</span>
                    <CopyButton text={command} label="Copiar comando" variant="secondary" />
                  </div>
                  <code className="code-block block select-all whitespace-pre-wrap break-all">{command}</code>
                </div>

                <p className="text-[11px] leading-5 text-warn">
                  Guarde o token agora. Ao fechar este aviso ou recarregar a página, ele não poderá
                  ser recuperado — só revogado.
                </p>
              </div>
            </Card>
          ) : null}

          <Card>
            <CardHeader
              title="Tokens emitidos"
              description={`${tokens.length} no histórico · ${activeCount} ativo${activeCount === 1 ? "" : "s"}`}
              icon={<Copy aria-hidden="true" className="size-4" />}
            />
            {tokens.length === 0 ? (
              <div className="p-4">
                <EmptyState
                  icon={<KeyRound aria-hidden="true" className="size-4" />}
                  title="Nenhum token emitido"
                  description="Crie o primeiro convite no formulário ao lado para inscrever um nó."
                />
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="data-table min-w-[720px]">
                  <caption>Tokens de entrada, uso, expiração e revogação</caption>
                  <thead>
                    <tr>
                      <th scope="col">Convite</th>
                      <th scope="col">Tags</th>
                      <th scope="col">Uso</th>
                      <th scope="col">Expira</th>
                      <th scope="col">Status</th>
                      <th scope="col" className="text-right">Ações</th>
                    </tr>
                  </thead>
                  <tbody>
                    {tokens.map((token) => {
                      const state = tokenState(token);
                      const revoking = revokeMutation.isPending && revokeMutation.variables?.id === token.id;
                      return (
                        <tr key={token.id}>
                          <td className="max-w-[220px]">
                            <span className="block truncate text-[13px] font-medium text-ink">{token.name}</span>
                            <span className="block font-mono text-[10px] text-faint">{token.prefix}…</span>
                            <span className="block text-[10px] text-faint" title={formatDateTime(token.created_at)}>
                              criado {formatRelative(token.created_at)}
                            </span>
                          </td>
                          <td>
                            {token.tags.length === 0 ? (
                              <span className="text-[11px] text-faint">—</span>
                            ) : (
                              <div className="flex max-w-[180px] flex-wrap gap-1">
                                {token.tags.map((tag) => (
                                  <Badge key={tag} tone="info" className="font-mono">
                                    {tag}
                                  </Badge>
                                ))}
                              </div>
                            )}
                          </td>
                          <td>
                            <UsageBar uses={token.uses} max={token.max_uses} />
                          </td>
                          <td className="whitespace-nowrap text-xs text-muted" title={formatDateTime(token.expires_at)}>
                            <span className="inline-flex items-center gap-1">
                              <Clock aria-hidden="true" className="size-3 text-faint" />
                              {formatRelative(token.expires_at)}
                            </span>
                          </td>
                          <td>
                            <Badge tone={state.tone} dot>
                              {state.label}
                            </Badge>
                          </td>
                          <td>
                            <div className="flex justify-end">
                              {token.revoked_at ? (
                                <span className="text-[10px] text-faint">revogado</span>
                              ) : (
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  loading={revoking}
                                  onClick={() => {
                                    revokeMutation.reset();
                                    setRevokeTarget(token);
                                  }}
                                  aria-label={`Revogar token ${token.name}`}
                                  title={`Revogar token ${token.name}`}
                                  className="h-7 px-2 text-[11px] text-faint hover:text-danger"
                                >
                                  <Trash2 aria-hidden="true" className="size-3.5" />
                                  Revogar
                                </Button>
                              )}
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
        title="Revogar token de entrada"
        description={
          revokeTarget ? (
            <>
              O convite <strong className="text-ink">{revokeTarget.name}</strong> deixará de aceitar
              novos enrollments imediatamente.
            </>
          ) : null
        }
        confirmLabel="Revogar token"
        pending={revokeMutation.isPending}
        error={revokeMutation.error}
      >
        <p className="text-[11px] leading-5 text-muted">
          Nós já inscritos com este token não são afetados; para removê-los, revogue o nó na página
          de nós.
        </p>
      </ConfirmDialog>
    </>
  );
}
