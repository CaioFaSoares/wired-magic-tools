import { useMemo, useState } from "react";
import { RefreshCw, ScrollText, Search, X } from "lucide-react";
import { formatDateTimeSeconds, formatRelative, normalizeSearch } from "../lib/format";
import { useAudit } from "../lib/queries";
import type { AuditEventView } from "../lib/types";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  EmptyState,
  ErrorState,
  Input,
  LoadingPanel,
  PageHeader,
  Select,
  cn,
  type BadgeTone,
} from "../components/ui";

type Category = "all" | "auth" | "nodes" | "tokens" | "acl" | "service" | "other";

const categoryLabels: Record<Category, string> = {
  all: "Todas",
  auth: "Sessão",
  nodes: "Nós",
  tokens: "Convites",
  acl: "ACL",
  service: "Integração",
  other: "Outras",
};

const categoryTones: Record<Category, BadgeTone> = {
  all: "neutral",
  auth: "info",
  nodes: "accent",
  tokens: "warn",
  acl: "ok",
  service: "info",
  other: "neutral",
};

function categoryOf(action: string): Category {
  if (action.startsWith("admin.")) return "auth";
  if (action.startsWith("node.")) return "nodes";
  if (action.startsWith("join_token.")) return "tokens";
  if (action.startsWith("acl.")) return "acl";
  if (action.startsWith("service_token.")) return "service";
  return "other";
}

function ActionBadge({ action }: { action: string }) {
  const category = categoryOf(action);
  return (
    <Badge tone={categoryTones[category]} className="font-mono">
      {action}
    </Badge>
  );
}

function matchesFilter(event: AuditEventView, needle: string, category: Category): boolean {
  if (category !== "all" && categoryOf(event.action) !== category) return false;
  if (!needle) return true;
  const haystack = normalizeSearch(
    [event.actor, event.action, event.target, event.detail ?? "", event.ip ?? "", String(event.id)].join(" "),
  );
  return haystack.includes(needle);
}

export function AuditPage() {
  const [limit, setLimit] = useState(200);
  const auditQuery = useAudit(limit);
  const [search, setSearch] = useState("");
  const [category, setCategory] = useState<Category>("all");

  const events = auditQuery.data ?? [];
  const needle = normalizeSearch(search);
  const filtered = useMemo(
    () => events.filter((event) => matchesFilter(event, needle, category)),
    [events, needle, category],
  );

  const counts = useMemo(() => {
    const map: Record<Category, number> = {
      all: events.length,
      auth: 0,
      nodes: 0,
      tokens: 0,
      acl: 0,
      service: 0,
      other: 0,
    };
    for (const event of events) map[categoryOf(event.action)] += 1;
    return map;
  }, [events]);

  const header = (
    <PageHeader
      eyebrow="Sistema"
      title="Auditoria"
      description="Trilha do operador e dos agentes. Novos eventos chegam por SSE e invalidam esta consulta."
      actions={
        <>
          <div className="flex items-center gap-2">
            <span className="text-[10px] text-faint">Limite</span>
            <Select
              aria-label="Limite de eventos"
              value={String(limit)}
              onChange={(event) => setLimit(Number(event.target.value))}
              className="h-8 w-[86px] py-1 text-xs"
            >
              <option value="100">100</option>
              <option value="200">200</option>
              <option value="500">500</option>
              <option value="1000">1000</option>
            </Select>
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void auditQuery.refetch()}
            loading={auditQuery.isFetching}
          >
            <RefreshCw aria-hidden="true" className="size-3.5" />
            Atualizar
          </Button>
        </>
      }
    />
  );

  if (auditQuery.isLoading) {
    return (
      <>
        {header}
        <LoadingPanel label="Carregando auditoria…" rows={6} />
      </>
    );
  }

  if (auditQuery.isError || !auditQuery.data) {
    return (
      <>
        {header}
        <ErrorState error={auditQuery.error} onRetry={() => void auditQuery.refetch()} />
      </>
    );
  }

  return (
    <>
      {header}

      <Card>
        <CardHeader
          title="Eventos"
          description={`${events.length} eventos no lote · ${filtered.length} exibidos · máximo de ${limit} por consulta`}
          icon={<ScrollText aria-hidden="true" className="size-4" />}
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
                aria-label="Filtrar auditoria"
                placeholder="ator, ação, alvo, detalhe ou IP"
                className="h-8 w-64 py-1 pl-8 pr-8 text-xs"
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
          {(Object.keys(categoryLabels) as Category[]).map((key) => (
            <button
              key={key}
              type="button"
              onClick={() => setCategory(key)}
              aria-pressed={category === key}
              className={cn(
                "inline-flex h-7 items-center gap-1.5 rounded-md border px-2 text-[11px] font-medium transition-colors",
                category === key
                  ? "border-accent/30 bg-accent/10 text-accent"
                  : "border-transparent text-muted hover:bg-white/[0.04] hover:text-ink",
              )}
            >
              {categoryLabels[key]}
              <span className="font-mono text-[10px] opacity-70">{counts[key]}</span>
            </button>
          ))}
          {(search || category !== "all") && (
            <Button
              variant="ghost"
              size="sm"
              className="ml-auto h-7 text-[11px]"
              onClick={() => {
                setSearch("");
                setCategory("all");
              }}
            >
              Limpar filtros
            </Button>
          )}
        </div>

        {events.length === 0 ? (
          <div className="p-4">
            <EmptyState
              icon={<ScrollText aria-hidden="true" className="size-4" />}
              title="Nenhum evento de auditoria"
              description="Login, aprovação de nó, tokens e ACL geram registros. Ações do operador aparecem primeiro."
            />
          </div>
        ) : filtered.length === 0 ? (
          <div className="p-4">
            <EmptyState
              title="Nenhum evento corresponde ao filtro"
              description="Busque por outro termo ou selecione todas as categorias."
              action={
                <Button
                  onClick={() => {
                    setSearch("");
                    setCategory("all");
                  }}
                >
                  Limpar filtros
                </Button>
              }
            />
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="data-table min-w-[1020px]">
              <caption>Eventos de auditoria do control plane</caption>
              <thead>
                <tr>
                  <th scope="col">Quando</th>
                  <th scope="col">Ator</th>
                  <th scope="col">Ação</th>
                  <th scope="col">Alvo</th>
                  <th scope="col">Detalhe</th>
                  <th scope="col">IP</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((event) => (
                  <tr key={event.id}>
                    <td className="whitespace-nowrap" title={formatRelative(event.ts)}>
                      <span className="font-mono text-[11px] text-ink">{formatDateTimeSeconds(event.ts)}</span>
                      <span className="block text-[10px] text-faint">{formatRelative(event.ts)}</span>
                    </td>
                    <td className="max-w-[190px] truncate font-mono text-[11px] text-muted" title={event.actor}>
                      {event.actor}
                    </td>
                    <td>
                      <ActionBadge action={event.action} />
                    </td>
                    <td className="max-w-[190px] truncate text-[11px] text-muted" title={event.target}>
                      {event.target}
                    </td>
                    <td className="max-w-[300px]">
                      {event.detail ? (
                        <span className="block truncate font-mono text-[10px] text-faint" title={event.detail}>
                          {event.detail}
                        </span>
                      ) : (
                        <span className="text-[11px] text-faint">—</span>
                      )}
                    </td>
                    <td className="whitespace-nowrap font-mono text-[10px] text-faint">{event.ip ?? "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        <div className="border-t border-line px-4 py-2 text-[10px] text-faint">
          Consulta: <span className="font-mono">GET /api/v1/admin/audit?limit={limit}</span>. O backend limita a
          1000 eventos por chamada; não há paginação na API.
        </div>
      </Card>
    </>
  );
}
