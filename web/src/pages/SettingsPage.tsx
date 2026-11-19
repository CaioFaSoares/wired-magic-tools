import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import {
  AlertTriangle,
  Check,
  Globe,
  KeyRound,
  Network,
  RefreshCw,
  Server,
  Settings,
  ShieldCheck,
} from "lucide-react";
import { useMeta, useSettings } from "../lib/queries";
import { shortenHash } from "../lib/format";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  CopyButton,
  ErrorState,
  LinkButton,
  LoadingPanel,
  PageHeader,
  StatusDot,
} from "../components/ui";

function ReadOnlyRow({
  label,
  value,
  copyText,
  title,
}: {
  label: string;
  value: ReactNode;
  copyText?: string;
  title?: string;
}) {
  return (
    <div className="grid grid-cols-[145px_minmax(0,1fr)] items-center gap-2 border-b border-line/60 py-2.5 last:border-0">
      <dt className="text-[11px] text-faint">{label}</dt>
      <dd className="flex min-w-0 items-center justify-between gap-2">
        <span className="min-w-0 break-words font-mono text-[12px] text-ink" title={title}>
          {value}
        </span>
        {copyText ? <CopyButton text={copyText} label="" copiedLabel="" variant="ghost" /> : null}
      </dd>
    </div>
  );
}

export function SettingsPage() {
  const settingsQuery = useSettings();
  const metaQuery = useMeta();

  const header = (
    <PageHeader
      eyebrow="Sistema"
      title="Configurações"
      description="Parâmetros efetivos do control plane. A API atual expõe leitura; alterações são feitas na configuração do processo."
      actions={
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            void settingsQuery.refetch();
            void metaQuery.refetch();
          }}
          loading={settingsQuery.isFetching}
        >
          <RefreshCw aria-hidden="true" className="size-3.5" />
          Atualizar
        </Button>
      }
    />
  );

  if (settingsQuery.isLoading) {
    return (
      <>
        {header}
        <LoadingPanel label="Carregando configurações…" rows={4} />
      </>
    );
  }

  if (settingsQuery.isError || !settingsQuery.data) {
    return (
      <>
        {header}
        <ErrorState error={settingsQuery.error} onRetry={() => void settingsQuery.refetch()} />
      </>
    );
  }

  const settings = settingsQuery.data;
  const meta = metaQuery.data;
  const network = settings.network;
  const controlUrl = typeof window !== "undefined" ? window.location.origin : "—";

  return (
    <>
      {header}

      <div className="mb-4 flex flex-wrap items-start gap-3 rounded-lg border border-line bg-white/[0.02] px-3.5 py-2.5">
        <ShieldCheck aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-accent" />
        <div className="min-w-0 flex-1 text-xs leading-5 text-muted">
          <span className="font-medium text-ink">Somente leitura.</span> Não existe rota de mutação para
          configurações no backend atual. A fonte é o processo do control (
          <span className="font-mono text-ink">wm host</span> /{" "}
          <span className="font-mono text-ink">wm serve</span>) e suas variáveis de ambiente; mudanças
          exigem reinício do serviço.
        </div>
        <Badge tone="neutral">GET /api/v1/admin/settings</Badge>
      </div>

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader
            title="Rede e overlay"
            description="Endereçamento e parâmetros entregues aos agentes no NetworkMap."
            icon={<Network aria-hidden="true" className="size-4" />}
          />
          <dl className="px-4 py-1">
            <ReadOnlyRow label="Nome da rede" value={network.name} />
            <ReadOnlyRow label="CIDR do overlay" value={network.overlay_cidr} copyText={network.overlay_cidr} />
            <ReadOnlyRow
              label="Endereço do control"
              value={network.server_address}
              copyText={network.server_address}
              title="Endereço .1 do overlay; não é um peer WireGuard."
            />
            <ReadOnlyRow label="Zona DNS" value={network.dns_zone} copyText={network.dns_zone} />
            <ReadOnlyRow label="MTU" value={`${network.mtu} bytes`} />
            <ReadOnlyRow
              label="Aprovação"
              value={
                <span className="inline-flex items-center gap-2">
                  <Badge tone={settings.approval_required ? "warn" : "ok"} dot>
                    {settings.approval_required ? "obrigatória" : "automática"}
                  </Badge>
                  <span className="font-sans text-[11px] text-faint">
                    {settings.approval_required
                      ? "novos nós entram como pendentes"
                      : "novos nós entram ativos"}
                  </span>
                </span>
              }
            />
            <ReadOnlyRow
              label="URL do control"
              value={controlUrl}
              copyText={controlUrl}
              title="Origem em que este painel está sendo servido."
            />
          </dl>
          {settings.approval_required ? (
            <div className="mx-4 mb-3 flex items-center justify-between gap-2 rounded-md border border-warn/25 bg-warn/[0.05] px-3 py-2">
              <p className="text-[11px] text-muted">Aprovação manual ativa: novos nós ficam pendentes até revisão.</p>
              <Link to="/nodes?status=pending" className="text-[11px] font-medium text-accent hover:underline">
                Revisar
              </Link>
            </div>
          ) : null}
        </Card>

        <Card>
          <CardHeader
            title="Relay e STUN"
            description="Configuração anunciada aos agentes; o estado vivo fica em Relay e STUN."
            icon={<Globe aria-hidden="true" className="size-4" />}
            actions={<LinkButton to="/relay-status" variant="ghost" size="sm">Abrir relay</LinkButton>}
          />
          <dl className="px-4 py-1">
            <ReadOnlyRow
              label="Relay WSS"
              value={
                settings.relay_url ? (
                  settings.relay_url
                ) : (
                  <Badge tone="warn">desligado</Badge>
                )
              }
              copyText={settings.relay_url ?? undefined}
            />
            <ReadOnlyRow
              label="STUN"
              value={
                settings.stun.length === 0 ? (
                  <span className="font-sans text-[11px] text-faint">nenhum endereço anunciado</span>
                ) : (
                  <span className="space-x-2">
                    {settings.stun.map((url) => (
                      <span key={url} className="inline-block font-mono text-[11px]">
                        {url}
                      </span>
                    ))}
                  </span>
                )
              }
              copyText={settings.stun.join(", ")}
            />
            <ReadOnlyRow
              label="No NetworkMap"
              value={
                metaQuery.isError ? "—" : meta?.relay ? "relay incluído" : "relay desligado"
              }
            />
          </dl>
          <div className="mx-4 mb-3 rounded-md border border-line bg-white/[0.02] px-3 py-2 text-[10px] leading-4 text-faint">
            STUN descobre o endpoint público; o relay WSS é fallback na 443. A API de settings não
            informa health de STUN — veja as conexões do relay e os caminhos reportados.
          </div>
        </Card>

        <Card>
          <CardHeader
            title="Versão e protocolo"
            description="Compatibilidade entre painel, control e agentes."
            icon={<Server aria-hidden="true" className="size-4" />}
          />
          <dl className="px-4 py-1">
            <ReadOnlyRow label="Versão do control" value={settings.version} />
            <ReadOnlyRow label="Protocolo" value={`v${settings.protocol}`} />
            <ReadOnlyRow label="Versão em /meta" value={meta?.version ?? "—"} />
            <ReadOnlyRow
              label="Chave do control"
              value={meta?.control_pubkey ? shortenHash(meta.control_pubkey, 10, 8) : "—"}
              copyText={meta?.control_pubkey}
              title={meta?.control_pubkey}
            />
            <ReadOnlyRow label="Prefixo da API" value="/api/v1" />
          </dl>
        </Card>

        <Card>
          <CardHeader
            title="Estado do control"
            description="Sinais observáveis pela API do painel."
            icon={<Settings aria-hidden="true" className="size-4" />}
          />
          <div className="space-y-2.5 p-4">
            <div className="flex items-start gap-2.5 rounded-md border border-line bg-white/[0.02] px-3 py-2.5">
              <div className="mt-1">
                <StatusDot tone={metaQuery.isSuccess ? "ok" : metaQuery.isError ? "danger" : "warn"} pulse={metaQuery.isSuccess} />
              </div>
              <div className="min-w-0">
                <p className="text-xs font-medium text-ink">
                  {metaQuery.isPending ? "verificando /meta" : metaQuery.isSuccess ? "/meta respondeu" : "/meta indisponível"}
                </p>
                <p className="mt-0.5 text-[11px] leading-4 text-muted">
                  {metaQuery.isSuccess
                    ? `${meta?.network.name} · ${meta?.network.overlay_cidr} · ${meta?.network.dns_zone}`
                    : metaQuery.isError
                      ? "O endpoint público não respondeu; a sessão admin pode continuar válida."
                      : "Consultando o endpoint público do control plane."}
                </p>
              </div>
            </div>

            <div className="flex items-start gap-2.5 rounded-md border border-line bg-white/[0.02] px-3 py-2.5">
              <KeyRound aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-faint" />
              <div>
                <p className="text-xs font-medium text-ink">Onde mudar</p>
                <p className="mt-0.5 text-[11px] leading-5 text-muted">
                  Flags do <span className="font-mono text-ink">wm host</span> (rede, MTU, aprovação) e
                  configuração do <span className="font-mono text-ink">wm serve --config</span>, com
                  variáveis <span className="font-mono text-ink">WM_*</span>. Reinicie o control e
                  confirme em /meta e /settings.
                </p>
              </div>
            </div>

            <div className="flex items-start gap-2.5 rounded-md border border-line bg-white/[0.02] px-3 py-2.5">
              <AlertTriangle aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-faint" />
              <div>
                <p className="text-xs font-medium text-ink">Pendência de contrato</p>
                <p className="mt-0.5 text-[11px] leading-5 text-muted">
                  Não há PUT/PATCH de settings no backend. Este painel não finge editar o que a API não
                  expõe; a configuração é somente leitura.
                </p>
              </div>
            </div>

            <div className="flex items-start gap-2.5 rounded-md border border-line bg-white/[0.02] px-3 py-2.5">
              <Check aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-ok" />
              <div>
                <p className="text-xs font-medium text-ink">Defaults do contrato</p>
                <p className="mt-0.5 text-[11px] leading-5 text-muted">
                  Overlay <span className="font-mono text-ink">10.66.0.0/24</span>, DNS{" "}
                  <span className="font-mono text-ink">wm.internal</span>, MTU{" "}
                  <span className="font-mono text-ink">1280</span>, protocolo{" "}
                  <span className="font-mono text-ink">1</span>. Os valores efetivos acima prevalecem.
                </p>
              </div>
            </div>
          </div>
        </Card>
      </div>
    </>
  );
}
