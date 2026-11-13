import { useEffect, useMemo, useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  BookOpen,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Code2,
  FlaskConical,
  Play,
  RotateCcw,
  Save,
  ShieldCheck,
} from "lucide-react";
import { api } from "../lib/api";
import { aclExamples, policyToYaml } from "../lib/aclExamples";
import { errorMessage } from "../lib/errors";
import { formatDateTime, formatPorts, formatProtocol } from "../lib/format";
import { qk, useAcl, useNodes } from "../lib/queries";
import type { AclView, CompiledAcl, ValidateAclResponse } from "../lib/types";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  CopyButton,
  ErrorState,
  Field,
  Input,
  LoadingPanel,
  PageHeader,
  Select,
  Spinner,
  cn,
} from "../components/ui";
import { useToast } from "../components/Toast";

function CompiledTable({ compiled }: { compiled: CompiledAcl }) {
  const [expanded, setExpanded] = useState(false);
  const limit = 200;
  const shown = compiled.rules.slice(0, limit);

  return (
    <div className="border-t border-line">
      <div className="flex flex-wrap items-center gap-2 px-4 py-2.5">
        <button
          type="button"
          onClick={() => setExpanded((current) => !current)}
          aria-expanded={expanded}
          className="inline-flex items-center gap-1.5 text-[11px] font-medium text-ink hover:text-accent"
        >
          {expanded ? (
            <ChevronDown aria-hidden="true" className="size-3.5" />
          ) : (
            <ChevronRight aria-hidden="true" className="size-3.5" />
          )}
          Regras compiladas
        </button>
        <Badge tone="neutral" className="font-mono">
          v{compiled.version}
        </Badge>
        <Badge tone="accent" className="font-mono">
          {compiled.rules.length} regra{compiled.rules.length === 1 ? "" : "s"}
        </Badge>
        <Badge tone={compiled.default_allow ? "warn" : "ok"}>
          {compiled.default_allow ? "default allow" : "default deny"}
        </Badge>
      </div>

      {expanded ? (
        compiled.rules.length === 0 ? (
          <div className="border-t border-line px-4 py-3">
            <p className="text-[11px] leading-5 text-muted">
              Nenhuma regra compilada. Sem regras, o compilado fica aberto (default allow).
            </p>
          </div>
        ) : (
          <div className="max-h-72 overflow-auto border-t border-line">
            <table className="data-table min-w-[760px]">
              <caption>Regras compiladas da ACL salva</caption>
              <thead className="sticky top-0 z-10">
                <tr>
                  <th scope="col" className="w-12">#</th>
                  <th scope="col">Ação</th>
                  <th scope="col">Origem</th>
                  <th scope="col">Destino</th>
                  <th scope="col">Proto</th>
                  <th scope="col">Portas</th>
                  <th scope="col">Comentário</th>
                </tr>
              </thead>
              <tbody>
                {shown.map((rule, index) => (
                  <tr key={`${rule.src}-${rule.dst}-${index}`}>
                    <td className="font-mono text-[10px] text-faint">{index + 1}</td>
                    <td>
                      <Badge tone={rule.action === "accept" ? "ok" : "danger"}>{rule.action}</Badge>
                    </td>
                    <td className="font-mono text-[11px] text-muted">{rule.src}</td>
                    <td className="font-mono text-[11px] text-muted">{rule.dst}</td>
                    <td className="font-mono text-[11px] text-muted">{formatProtocol(rule.protocol)}</td>
                    <td className="font-mono text-[11px] text-muted">{formatPorts(rule.ports)}</td>
                    <td className="max-w-[220px] truncate text-[11px] text-faint" title={rule.comment ?? undefined}>
                      {rule.comment || "—"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            {compiled.rules.length > limit ? (
              <p className="border-t border-line px-3 py-2 text-[10px] text-faint">
                Mostrando as primeiras {limit} de {compiled.rules.length} regras compiladas.
              </p>
            ) : null}
          </div>
        )
      ) : null}
    </div>
  );
}

export function AclPage() {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const aclQuery = useAcl();
  const nodesQuery = useNodes();

  const [yamlText, setYamlText] = useState("");
  const [dirty, setDirty] = useState(false);
  const [validation, setValidation] = useState<ValidateAclResponse | null>(null);
  const [formError, setFormError] = useState<string | null>(null);

  const [simSrc, setSimSrc] = useState("");
  const [simDst, setSimDst] = useState("");
  const [simProtocol, setSimProtocol] = useState("tcp");
  const [simPort, setSimPort] = useState("8421");
  const [simError, setSimError] = useState<string | null>(null);

  const nodes = nodesQuery.data ?? [];
  const nodeNames = useMemo(() => nodes.map((node) => node.name), [nodes]);
  const nodeIps = useMemo(() => nodes.map((node) => node.overlay_ip), [nodes]);

  useEffect(() => {
    if (!aclQuery.data || dirty) return;
    const next = policyToYaml(aclQuery.data.policy);
    setYamlText((current) => (current === next ? current : next));
  }, [aclQuery.data, dirty]);

  useEffect(() => {
    simulateMutation.reset();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [aclQuery.data?.compiled.version]);

  useEffect(() => {
    if (!dirty) return;
    function handleBeforeUnload(event: BeforeUnloadEvent) {
      event.preventDefault();
      event.returnValue = "";
    }
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => window.removeEventListener("beforeunload", handleBeforeUnload);
  }, [dirty]);

  const validateMutation = useMutation({
    mutationFn: () => api.validateAcl(yamlText),
    onSuccess: (result) => {
      setValidation(result);
      setFormError(null);
    },
  });

  const saveMutation = useMutation({
    mutationFn: () => api.putAcl(yamlText),
    onSuccess: (view: AclView) => {
      queryClient.setQueryData(qk.acl, view);
      setDirty(false);
      setValidation(null);
      setFormError(null);
      void queryClient.invalidateQueries({ queryKey: qk.nodes });
      void queryClient.invalidateQueries({ queryKey: ["audit"] });
      push({
        tone: "ok",
        title: "ACL publicada",
        description: `${view.compiled.rules.length} regra(s) compiladas · versão ${view.compiled.version}.`,
      });
    },
    onError: (error) => setFormError(errorMessage(error)),
  });

  const simulateMutation = useMutation({
    mutationFn: api.simulateAcl,
    onSuccess: () => setSimError(null),
  });

  function handleYamlChange(value: string) {
    setYamlText(value);
    setDirty(true);
    setValidation(null);
    setFormError(null);
  }

  function loadExample(yaml: string) {
    setYamlText(yaml);
    setDirty(true);
    setValidation(null);
    setFormError(null);
  }

  function discardChanges() {
    if (!aclQuery.data) return;
    setYamlText(policyToYaml(aclQuery.data.policy));
    setDirty(false);
    setValidation(null);
    setFormError(null);
  }

  function handleValidate() {
    setFormError(null);
    validateMutation.mutate();
  }

  function handleSave() {
    setFormError(null);
    saveMutation.mutate();
  }

  function handleSimulate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setSimError(null);
    if (!simSrc.trim() || !simDst.trim()) {
      setSimError("Informe a origem e o destino (nome do nó ou IP).");
      return;
    }
    let port: number | null = null;
    if (simProtocol !== "icmp") {
      const parsed = Number(simPort);
      if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
        setSimError("Informe uma porta entre 1 e 65535.");
        return;
      }
      port = parsed;
    }
    simulateMutation.mutate({
      src: simSrc.trim(),
      dst: simDst.trim(),
      protocol: simProtocol,
      port,
    });
  }

  const header = (
    <PageHeader
      eyebrow="Política"
      title="ACL"
      description="Política YAML compilada pelo control plane e aplicada pelos agentes nos dois lados do fluxo."
      actions={
        <>
          <Button
            variant="secondary"
            size="sm"
            onClick={handleValidate}
            loading={validateMutation.isPending}
            disabled={!yamlText}
          >
            <ShieldCheck aria-hidden="true" className="size-3.5" />
            Validar
          </Button>
          <Button
            variant="primary"
            size="sm"
            onClick={handleSave}
            loading={saveMutation.isPending}
            disabled={!dirty || !yamlText}
          >
            <Save aria-hidden="true" className="size-3.5" />
            Salvar e publicar
          </Button>
        </>
      }
    />
  );

  if (aclQuery.isLoading) {
    return (
      <>
        {header}
        <LoadingPanel label="Carregando ACL…" rows={4} />
      </>
    );
  }

  if (aclQuery.isError || !aclQuery.data) {
    return (
      <>
        {header}
        <ErrorState error={aclQuery.error} onRetry={() => void aclQuery.refetch()} />
      </>
    );
  }

  const acl = aclQuery.data;
  const lines = yamlText.split("\n").length;
  const compiledPreview = validation?.compiled ?? acl.compiled;

  return (
    <>
      {header}

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,1.5fr)_minmax(340px,1fr)]">
        <div className="space-y-4">
          <Card>
            <CardHeader
              title="Política YAML"
              description="A ordem importa: a primeira regra que casa decide. Os seletores são resolvidos no momento de salvar."
              icon={<Code2 aria-hidden="true" className="size-4" />}
              actions={
                <>
                  <span className="text-[10px] text-faint">{lines} linhas</span>
                  {dirty ? <Badge tone="warn" dot>não salva</Badge> : <Badge tone="ok">salva</Badge>}
                  <Button variant="ghost" size="sm" onClick={discardChanges} disabled={!dirty || saveMutation.isPending}>
                    <RotateCcw aria-hidden="true" className="size-3.5" />
                    Descartar
                  </Button>
                </>
              }
            />

            <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 py-2">
              <span className="text-[10px] uppercase tracking-[0.14em] text-faint">Exemplos</span>
              {aclExamples.map((example) => (
                <Button
                  key={example.id}
                  variant="subtle"
                  size="sm"
                  onClick={() => loadExample(example.yaml)}
                  title={example.description}
                >
                  {example.title}
                </Button>
              ))}
              <span className="ml-auto inline-flex items-center gap-1.5 text-[10px] text-faint">
                {acl.updated_at ? `publicada ${formatDateTime(acl.updated_at)}` : "nunca publicada"}
              </span>
            </div>

            <div className="p-3">
              <label htmlFor="acl-editor" className="sr-only">
                Editor YAML da ACL
              </label>
              <textarea
                id="acl-editor"
                value={yamlText}
                onChange={(event) => handleYamlChange(event.target.value)}
                spellCheck={false}
                autoCorrect="off"
                autoCapitalize="off"
                aria-describedby="acl-editor-help"
                className="scroll-thin block h-[480px] w-full resize-y rounded-md border border-line-strong bg-[#060a0e] px-3 py-2.5 font-mono text-xs leading-5 text-ink outline-none transition-colors focus:border-accent focus:ring-2 focus:ring-accent/20"
              />
              <p id="acl-editor-help" className="mt-1.5 flex flex-wrap items-center justify-between gap-2 text-[10px] text-faint">
                <span>
                  Seletores: <code className="text-muted">*</code>, <code className="text-muted">name:foo</code>,{" "}
                  <code className="text-muted">tag:bar</code>, CIDR. Ações: accept / deny.
                </span>
                <CopyButton text={yamlText} label="Copiar YAML" variant="ghost" />
              </p>
            </div>

            {validation ? (
              <div
                className={cn(
                  "mx-3 mb-3 rounded-lg border px-3 py-2.5",
                  validation.warnings.length > 0
                    ? "border-warn/30 bg-warn/[0.05]"
                    : "border-ok/25 bg-ok/[0.05]",
                )}
              >
                <div className="flex flex-wrap items-center gap-2">
                  {validation.warnings.length > 0 ? (
                    <AlertTriangle aria-hidden="true" className="size-4 text-warn" />
                  ) : (
                    <CheckCircle2 aria-hidden="true" className="size-4 text-ok" />
                  )}
                  <span className="text-xs font-medium text-ink">
                    YAML válido · {validation.compiled.rules.length} regra(s) compiladas
                  </span>
                  <Badge tone={validation.compiled.default_allow ? "warn" : "ok"}>
                    {validation.compiled.default_allow ? "default allow" : "default deny"}
                  </Badge>
                  <Badge tone="neutral">{validation.warnings.length} warning(s)</Badge>
                </div>
                {validation.warnings.length > 0 ? (
                  <ul className="mt-2 space-y-1 pl-6 text-[11px] leading-5 text-warn">
                    {validation.warnings.map((warning, index) => (
                      <li key={`${warning}-${index}`} className="list-disc">
                        {warning}
                      </li>
                    ))}
                  </ul>
                ) : null}
              </div>
            ) : null}

            <div aria-live="polite">
              {validateMutation.isError ? (
                <p role="alert" className="mx-3 mb-3 rounded-md border border-danger/30 bg-danger/[0.06] px-3 py-2 text-xs leading-5 text-danger">
                  Validação falhou: {errorMessage(validateMutation.error)}
                </p>
              ) : null}
              {formError ? (
                <p role="alert" className="mx-3 mb-3 rounded-md border border-danger/30 bg-danger/[0.06] px-3 py-2 text-xs leading-5 text-danger">
                  {formError}
                </p>
              ) : null}
            </div>

            <CompiledTable compiled={acl.compiled} />
            <div className="border-t border-line px-4 py-2.5 text-[10px] text-faint">
              O compilado acima é a versão publicada. A prévia da validação ({compiledPreview.rules.length}{" "}
              regra(s)) é aplicada somente após salvar.
            </div>
          </Card>

          <Card>
            <CardHeader
              title="Como a ACL é aplicada"
              description="A mesma política vale para todos os agentes; o control compila e o mapa distribui."
              icon={<ShieldCheck aria-hidden="true" className="size-4" />}
            />
            <div className="grid grid-cols-1 gap-3 p-4 text-[11px] leading-5 text-muted sm:grid-cols-2">
              <div>
                <p className="font-medium text-ink">First-match, direcional</p>
                <p className="mt-0.5">
                  A primeira regra que casa decide. <code className="text-accent">accept A → B</code> não
                  libera B → A para fluxos novos; o conntrack cobre a resposta de um fluxo já aceito.
                </p>
              </div>
              <div>
                <p className="font-medium text-ink">Defesa nos dois lados</p>
                <p className="mt-0.5">
                  O agente aplica no egress antes de cifrar e no ingress depois de decifrar, incluindo
                  anti-spoof de <code className="text-accent">overlay_ip</code>.
                </p>
              </div>
              <div>
                <p className="font-medium text-ink">Default</p>
                <p className="mt-0.5">
                  Sem regras compiladas, o compilado fica aberto. Com pelo menos uma regra
                  compilada, o default é deny; uma regra explícita accept * → * reabre tudo.
                </p>
              </div>
              <div>
                <p className="font-medium text-ink">Sem espelho automático</p>
                <p className="mt-0.5">
                  O simulador e o compilado mostram exatamente o que os agentes recebem. Regras com
                  seletores inexistentes geram warning e são ignoradas.
                </p>
              </div>
            </div>
          </Card>
        </div>

        <div className="space-y-4">
          <Card>
            <CardHeader
              title="Simulador"
              description="Testa a ACL publicada, não o rascunho do editor."
              icon={<FlaskConical aria-hidden="true" className="size-4" />}
            />
            <form onSubmit={handleSimulate} className="space-y-3 p-4" noValidate>
              <Field label="Origem" htmlFor="sim-src" hint="Nome do nó ou IP de overlay.">
                <Input
                  id="sim-src"
                  value={simSrc}
                  onChange={(event) => setSimSrc(event.target.value)}
                  placeholder="mac-ana ou 10.66.0.3"
                  autoComplete="off"
                  spellCheck={false}
                  list="sim-nodes"
                  disabled={simulateMutation.isPending}
                  className="font-mono text-xs"
                />
              </Field>
              <Field label="Destino" htmlFor="sim-dst">
                <Input
                  id="sim-dst"
                  value={simDst}
                  onChange={(event) => setSimDst(event.target.value)}
                  placeholder="mac-bruno ou 10.66.0.4"
                  autoComplete="off"
                  spellCheck={false}
                  list="sim-nodes"
                  disabled={simulateMutation.isPending}
                  className="font-mono text-xs"
                />
              </Field>
              <div className="grid grid-cols-[120px_minmax(0,1fr)] gap-2">
                <Field label="Protocolo" htmlFor="sim-protocol">
                  <Select
                    id="sim-protocol"
                    value={simProtocol}
                    onChange={(event) => setSimProtocol(event.target.value)}
                    disabled={simulateMutation.isPending}
                  >
                    <option value="tcp">tcp</option>
                    <option value="udp">udp</option>
                    <option value="icmp">icmp</option>
                  </Select>
                </Field>
                <Field label="Porta" htmlFor="sim-port" hint={simProtocol === "icmp" ? "Ignorada para ICMP." : undefined}>
                  <Input
                    id="sim-port"
                    type="number"
                    min={1}
                    max={65535}
                    value={simPort}
                    onChange={(event) => setSimPort(event.target.value)}
                    disabled={simulateMutation.isPending || simProtocol === "icmp"}
                    className="font-mono text-xs"
                  />
                </Field>
              </div>
              <datalist id="sim-nodes">
                {nodeNames.map((nodeName, index) => (
                  <option key={nodeName} value={nodeName}>
                    {nodeIps[index]}
                  </option>
                ))}
              </datalist>

              {simError ? (
                <p role="alert" className="rounded-md border border-danger/30 bg-danger/[0.06] px-3 py-2 text-xs leading-5 text-danger">
                  {simError}
                </p>
              ) : null}
              {simulateMutation.isError ? (
                <p role="alert" className="rounded-md border border-danger/30 bg-danger/[0.06] px-3 py-2 text-xs leading-5 text-danger">
                  {errorMessage(simulateMutation.error)}
                </p>
              ) : null}

              <Button type="submit" variant="primary" className="w-full" loading={simulateMutation.isPending}>
                <Play aria-hidden="true" className="size-3.5" />
                Simular fluxo
              </Button>

              <div aria-live="polite">
                {simulateMutation.data ? (
                  <div
                    className={cn(
                      "rounded-lg border p-3",
                      simulateMutation.data.allowed
                        ? "border-ok/30 bg-ok/[0.06]"
                        : "border-danger/30 bg-danger/[0.06]",
                    )}
                  >
                    <p
                      className={cn(
                        "flex items-center gap-1.5 text-sm font-semibold",
                        simulateMutation.data.allowed ? "text-ok" : "text-danger",
                      )}
                    >
                      {simulateMutation.data.allowed ? (
                        <CheckCircle2 aria-hidden="true" className="size-4" />
                      ) : (
                        <AlertTriangle aria-hidden="true" className="size-4" />
                      )}
                      {simulateMutation.data.allowed ? "Permitido" : "Negado"}
                    </p>
                    <p className="mt-1 font-mono text-[10px] leading-4 text-muted">
                      {simulateMutation.data.rule_index != null
                        ? `regra ${simulateMutation.data.rule_index + 1} (índice ${simulateMutation.data.rule_index})`
                        : "nenhuma regra casou"}
                    </p>
                    <p className="mt-1 text-[11px] leading-4 text-muted">{simulateMutation.data.reason}</p>
                  </div>
                ) : null}
              </div>

              <p className="text-[10px] leading-4 text-faint">
                {nodes.length > 0
                  ? `${nodes.length} nó(s) disponíveis para nome. A simulação usa a ACL salva no control.`
                  : "Nenhum nó carregado; use IPs do overlay diretamente. A simulação usa a ACL salva."}
              </p>
            </form>
          </Card>

          <Card>
            <CardHeader
              title="Seletores e portas"
              description="A mesma sintaxe dos exemplos e da política salva."
              icon={<BookOpen aria-hidden="true" className="size-4" />}
            />
            <dl className="space-y-2.5 px-4 py-3 text-[11px] leading-5">
              <div>
                <dt className="font-mono text-accent">*</dt>
                <dd className="text-muted">todo o overlay.</dd>
              </div>
              <div>
                <dt className="font-mono text-accent">name:mac-ana</dt>
                <dd className="text-muted">um nó pelo nome exato.</dd>
              </div>
              <div>
                <dt className="font-mono text-accent">tag:operator</dt>
                <dd className="text-muted">todos os nós com a tag.</dd>
              </div>
              <div>
                <dt className="font-mono text-accent">10.66.0.0/24</dt>
                <dd className="text-muted">CIDR ou IP individual.</dd>
              </div>
              <div>
                <dt className="font-mono text-accent">ports: ["22", "8000-8100", "*"]</dt>
                <dd className="text-muted">portas de destino; ICMP ignora portas.</dd>
              </div>
            </dl>
            {nodesQuery.isError ? (
              <p className="border-t border-line px-4 py-2 text-[10px] text-warn">
                Não foi possível carregar os nós para o simulador.
              </p>
            ) : null}
          </Card>
        </div>
      </div>

      {validateMutation.isPending || saveMutation.isPending ? (
        <span className="sr-only" role="status">
          <Spinner className="inline" /> processando ACL
        </span>
      ) : null}
    </>
  );
}
