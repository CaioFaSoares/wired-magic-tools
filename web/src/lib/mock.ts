/**
 * Mock opt-in para desenvolvimento sem backend (`VITE_MOCK=1`).
 * Não é importado nem instalado em builds normais: o caminho real usa a API.
 * O estado é em memória e reinicia a cada reload; credenciais: admin/admin.
 */
import { parse } from "yaml";
import type {
  AclAction,
  AclPolicy,
  AclView,
  AdminUser,
  AuditEventView,
  CompiledAcl,
  CompiledRule,
  CreateJoinTokenRequest,
  CreateServiceTokenRequest,
  IntegrationNode,
  IntegrationNodesResponse,
  JoinTokenView,
  LoginResponse,
  NetworkInfo,
  NodeView,
  Overview,
  PeerStats,
  PortRange,
  ServiceTokenView,
  SettingsView,
  SimulateRequest,
  SimulateResponse,
  UpdateNodeRequest,
  ValidateAclResponse,
  VersionInfo,
} from "./types";

interface MockHttpErrorInit {
  status: number;
  code: string;
  message: string;
}

class MockHttpError extends Error {
  readonly status: number;
  readonly code: string;

  constructor({ status, code, message }: MockHttpErrorInit) {
    super(message);
    this.name = "MockHttpError";
    this.status = status;
    this.code = code;
  }
}

type JsonRecord = Record<string, unknown>;

function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStatus(value: unknown): value is NodeView["status"] {
  return value === "active" || value === "pending" || value === "disabled";
}

function stringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string");
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json; charset=utf-8" },
  });
}

function fail(status: number, code: string, message: string): Response {
  return json({ error: { code, message } }, status);
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

// ---------------------------------------------------------------------------
// Estado em memória
// ---------------------------------------------------------------------------

const network: NetworkInfo = {
  name: "wired-magic",
  overlay_cidr: "10.66.0.0/24",
  dns_zone: "wm.internal",
  mtu: 1280,
  server_address: "10.66.0.1",
};

const now = Date.now();
function iso(deltaSeconds: number): string {
  return new Date(now + deltaSeconds * 1000).toISOString();
}

function peer(id: string, path: PeerStats["path"], rtt: number | null, overrides: Partial<PeerStats> = {}): PeerStats {
  return {
    peer_id: id,
    path,
    rtt_ms: rtt,
    last_handshake_secs: 5,
    rx_bytes: 1_240_000,
    tx_bytes: 980_000,
    relay_online: path === "relay",
    endpoint: path === "direct" ? "203.0.113.20:41641" : null,
    last_error: null,
    ...overrides,
  };
}

const NODE_IDS = {
  host: "11111111-1111-4111-8111-111111111111",
  ana: "22222222-2222-4222-8222-222222222222",
  carla: "33333333-3333-4333-8333-333333333333",
  bruno: "44444444-4444-4444-8444-444444444444",
  old: "55555555-5555-4555-8555-555555555555",
};

let nodes: NodeView[] = [
  {
    id: NODE_IDS.host,
    name: "host-vps",
    hostname: "mesh-control",
    overlay_ip: "10.66.0.2",
    wg_public_key: "bW9jayB3aXJlZ3VhcmQga2V5IGhvc3QgKDMp",
    tags: ["operator", "site"],
    labels: { role: "control", region: "br" },
    endpoints: [{ addr: "203.0.113.10:41641", kind: "stun" }],
    online: true,
    last_seen: iso(-4),
    status: "active",
    created_at: iso(-11 * 86_400),
    approved_at: iso(-11 * 86_400),
    disabled_at: null,
    nat_type: "cone",
    relay_connected: false,
    reported_peers: [peer(NODE_IDS.ana, "direct", 6), peer(NODE_IDS.carla, "relay", 37)],
  },
  {
    id: NODE_IDS.ana,
    name: "mac-ana",
    hostname: "MacBook-Pro-de-Ana.local",
    overlay_ip: "10.66.0.3",
    wg_public_key: "bW9jayB3aXJlZ3VhcmQga2V5IGFuYSAoMyk=",
    tags: ["mac", "seance"],
    labels: { seance: "true", baguette_port: "8421" },
    endpoints: [
      { addr: "192.168.0.24:41641", kind: "lan" },
      { addr: "198.51.100.7:41641", kind: "stun" },
    ],
    online: true,
    last_seen: iso(-14),
    status: "active",
    created_at: iso(-3 * 86_400),
    approved_at: iso(-3 * 86_400),
    disabled_at: null,
    nat_type: "cone",
    relay_connected: false,
    reported_peers: [peer(NODE_IDS.host, "direct", 8)],
  },
  {
    id: NODE_IDS.carla,
    name: "mac-carla",
    hostname: "Mac-mini-Carla.local",
    overlay_ip: "10.66.0.4",
    wg_public_key: "bW9jayB3aXJlZ3VhcmQga2V5IGNhcmxhICgzKQ==",
    tags: ["mac"],
    labels: { seance: "true", baguette_port: "8422" },
    endpoints: [{ addr: "203.0.113.77:51234", kind: "stun" }],
    online: true,
    last_seen: iso(-19),
    status: "active",
    created_at: iso(-2 * 86_400),
    approved_at: iso(-2 * 86_400),
    disabled_at: null,
    nat_type: "symmetric",
    relay_connected: true,
    reported_peers: [peer(NODE_IDS.host, "relay", 41, { endpoint: null })],
  },
  {
    id: NODE_IDS.bruno,
    name: "mac-bruno",
    hostname: "MacBook-Air-de-Bruno.local",
    overlay_ip: "10.66.0.5",
    wg_public_key: "bW9jayB3aXJlZ3VhcmQga2V5IGJydW5vICgzKQ==",
    tags: ["mac"],
    labels: { baguette_port: "8421" },
    endpoints: [{ addr: "203.0.113.91:50001", kind: "stun" }],
    online: false,
    last_seen: iso(-3600),
    status: "pending",
    created_at: iso(-3500),
    approved_at: null,
    disabled_at: null,
    nat_type: "symmetric",
    relay_connected: false,
    reported_peers: [],
  },
  {
    id: NODE_IDS.old,
    name: "mac-antigo",
    hostname: "MacBook-2015.local",
    overlay_ip: "10.66.0.6",
    wg_public_key: "bW9jayB3aXJlZ3VhcmQga2V5IG9sZCAoMyk=",
    tags: ["mac"],
    labels: {},
    endpoints: [],
    online: false,
    last_seen: iso(-9 * 86_400),
    status: "disabled",
    created_at: iso(-30 * 86_400),
    approved_at: iso(-29 * 86_400),
    disabled_at: iso(-8 * 86_400),
    nat_type: "unknown",
    relay_connected: false,
    reported_peers: [],
  },
];

const initialPolicy: AclPolicy = {
  version: 3,
  rules: [
    {
      action: "accept",
      src: ["tag:operator"],
      dst: ["tag:mac"],
      protocol: "tcp",
      ports: ["8421"],
      comment: "Baguette do Seance",
    },
    {
      action: "accept",
      src: ["tag:operator"],
      dst: ["tag:mac"],
      protocol: "tcp",
      ports: ["22"],
      comment: "SSH de diagnóstico",
    },
    {
      action: "deny",
      src: ["*"],
      dst: ["tag:mac"],
      protocol: "*",
      ports: ["*"],
      comment: "fecha o resto para os Macs",
    },
  ],
};

let joinTokens: JoinTokenView[] = [
  {
    id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    name: "convite-demo",
    prefix: "wmj_mockAbc",
    tags: ["mac", "seance"],
    labels: { seance: "true" },
    max_uses: 5,
    uses: 1,
    expires_at: iso(6 * 86_400),
    created_at: iso(-86_400),
    revoked_at: null,
  },
];

let serviceTokens: ServiceTokenView[] = [];
const serviceSecrets = new Map<string, string>();

const audits: AuditEventView[] = [];
let nextAuditId = 1;
function addAudit(actor: string, action: string, target: string, detail: string | null, ip: string | null = "127.0.0.1"): void {
  audits.unshift({
    id: nextAuditId++,
    ts: new Date().toISOString(),
    actor,
    action,
    target,
    detail,
    ip,
  });
  if (audits.length > 500) audits.length = 500;
}

addAudit("admin:admin", "acl.update", "policy", "rules=12", "127.0.0.1");
addAudit("admin:admin", "join_token.create", "convite-demo", "convite-demo", "127.0.0.1");
addAudit("admin:admin", "node.approve", "mac-ana", null, "127.0.0.1");
addAudit("enroll:198.51.100.7", "node.enroll", "mac-bruno", "status=Pending", "198.51.100.7");

let currentPolicy: AclPolicy = structuredClone(initialPolicy);
let compiledAcl: CompiledAcl;
let aclUpdatedAt: string | null = iso(-2 * 86_400);

let authenticated = false;
let csrf = "";

const admin: AdminUser = {
  id: "99999999-9999-4999-8999-999999999999",
  username: "admin",
};

// ---------------------------------------------------------------------------
// ACL: compilação simplificada, espelhando `wm-control/src/acl.rs`
// ---------------------------------------------------------------------------

function ipv4ToInt(ip: string): number | null {
  const parts = ip.split(".");
  if (parts.length !== 4) return null;
  let value = 0;
  for (const part of parts) {
    if (!/^\d{1,3}$/.test(part)) return null;
    const octet = Number(part);
    if (octet > 255) return null;
    value = ((value << 8) | octet) >>> 0;
  }
  return value;
}

function ipInCidr(ip: string, cidr: string): boolean {
  const [base, prefixRaw] = cidr.split("/");
  const prefix = prefixRaw === undefined ? 32 : Number(prefixRaw);
  const ipInt = ipv4ToInt(ip);
  const baseInt = ipv4ToInt(base);
  if (ipInt == null || baseInt == null || !Number.isInteger(prefix) || prefix < 0 || prefix > 32) {
    return false;
  }
  if (prefix === 0) return true;
  const mask = (0xffffffff << (32 - prefix)) >>> 0;
  return (ipInt & mask) === (baseInt & mask);
}

function hostCidr(ip: string): string {
  return `${ip}/32`;
}

function parseProtocol(raw: string | null | undefined): number | null {
  if (raw == null) return null;
  const value = raw.trim().toLowerCase();
  if (!value || value === "*" || value === "any") return null;
  if (value === "tcp") return 6;
  if (value === "udp") return 17;
  if (value === "icmp") return 1;
  const number = Number(value);
  if (Number.isInteger(number) && number >= 0 && number <= 255) return number;
  throw new MockHttpError({ status: 400, code: "invalid_acl", message: `protocolo inválido '${raw}'` });
}

function parsePorts(raw: string[] | undefined): PortRange[] {
  const ports: PortRange[] = [];
  for (const item of raw ?? []) {
    const value = item.trim();
    if (!value || value === "*") continue;
    if (value.includes("-")) {
      const [startRaw, endRaw] = value.split("-");
      const start = Number(startRaw);
      const end = Number(endRaw);
      if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end > 65535 || start > end) {
        throw new MockHttpError({ status: 400, code: "invalid_acl", message: `porta inválida '${value}'` });
      }
      ports.push({ start, end });
    } else {
      const port = Number(value);
      if (!Number.isInteger(port) || port < 0 || port > 65535) {
        throw new MockHttpError({ status: 400, code: "invalid_acl", message: `porta inválida '${value}'` });
      }
      ports.push({ start: port, end: port });
    }
  }
  return ports;
}

function expandSelectors(
  selectors: string[],
  side: "src" | "dst",
  ruleNumber: number,
  warnings: string[],
): string[] {
  if (selectors.length === 0) {
    throw new MockHttpError({ status: 400, code: "invalid_acl", message: `regra ${ruleNumber}: ${side} vazio` });
  }
  const out: string[] = [];
  for (const raw of selectors) {
    const selector = raw.trim();
    if (!selector || selector === "*" || selector.toLowerCase() === "any") {
      out.push(network.overlay_cidr);
      continue;
    }
    if (selector.startsWith("name:")) {
      const name = selector.slice(5).trim();
      const node = nodes.find((item) => item.name === name);
      if (node) out.push(hostCidr(node.overlay_ip));
      else warnings.push(`regra ${ruleNumber}: nome '${name}' não existe; seletor ignorado`);
      continue;
    }
    if (selector.startsWith("tag:")) {
      const tag = selector.slice(4).trim();
      const tagged = nodes.filter((node) => node.tags.includes(tag));
      if (tagged.length === 0) {
        warnings.push(`regra ${ruleNumber}: nenhum nó com a tag '${tag}'; seletor ignorado`);
      }
      for (const node of tagged) out.push(hostCidr(node.overlay_ip));
      continue;
    }
    if (selector.includes("/")) {
      const [base, prefix] = selector.split("/");
      if (ipv4ToInt(base) == null || !Number.isInteger(Number(prefix)) || Number(prefix) > 32) {
        throw new MockHttpError({ status: 400, code: "invalid_acl", message: `regra ${ruleNumber}: CIDR inválido '${selector}'` });
      }
      out.push(selector);
      continue;
    }
    if (ipv4ToInt(selector) != null) {
      out.push(hostCidr(selector));
      continue;
    }
    throw new MockHttpError({
      status: 400,
      code: "invalid_acl",
      message: `regra ${ruleNumber}: seletor '${selector}' inválido (use *, name:, tag: ou CIDR)`,
    });
  }
  if (out.length === 0) {
    warnings.push(`regra ${ruleNumber}: nenhum seletor resolvido em ${side}; regra ignorada`);
  }
  return out;
}

function compilePolicy(policy: AclPolicy): { compiled: CompiledAcl; warnings: string[] } {
  const warnings: string[] = [];
  const rules: CompiledRule[] = [];
  policy.rules.forEach((rule, index) => {
    const ruleNumber = index + 1;
    const srcs = expandSelectors(rule.src, "src", ruleNumber, warnings);
    const dsts = expandSelectors(rule.dst, "dst", ruleNumber, warnings);
    const protocol = parseProtocol(rule.protocol);
    const ports = parsePorts(rule.ports);
    for (const src of srcs) {
      for (const dst of dsts) {
        rules.push({
          action: rule.action,
          src,
          dst,
          protocol,
          ports,
          comment: rule.comment ?? null,
        });
      }
    }
  });
  return {
    warnings,
    compiled: {
      version: policy.version,
      default_allow: rules.length === 0,
      rules,
    },
  };
}

function parsePolicyYaml(yamlText: string): AclPolicy {
  let doc: unknown;
  try {
    doc = parse(yamlText);
  } catch (error) {
    throw new MockHttpError({
      status: 400,
      code: "invalid_acl_yaml",
      message: `YAML inválido: ${error instanceof Error ? error.message : "erro de parse"}`,
    });
  }
  if (!isRecord(doc)) {
    throw new MockHttpError({ status: 400, code: "invalid_acl_yaml", message: "YAML vazio ou não é um objeto" });
  }
  const version = typeof doc.version === "number" && Number.isInteger(doc.version) ? doc.version : 1;
  const rawRules = Array.isArray(doc.rules) ? doc.rules : [];
  const rules = rawRules.map((raw, index) => {
    if (!isRecord(raw)) {
      throw new MockHttpError({ status: 400, code: "invalid_acl", message: `regra ${index + 1}: formato inválido` });
    }
    const action = raw.action;
    if (action !== "accept" && action !== "deny") {
      throw new MockHttpError({
        status: 400,
        code: "invalid_acl",
        message: `regra ${index + 1}: action deve ser accept ou deny`,
      });
    }
    const src = stringArray(raw.src);
    const dst = stringArray(raw.dst);
    const protocol = typeof raw.protocol === "string" ? raw.protocol : raw.protocol == null ? null : String(raw.protocol);
    const ports = stringArray(raw.ports);
    const comment = typeof raw.comment === "string" ? raw.comment : null;
    return { action: action as AclAction, src, dst, protocol, ports, comment };
  });
  return { version, rules };
}

const initialCompilation = compilePolicy(initialPolicy);
compiledAcl = initialCompilation.compiled;

function simulate(req: SimulateRequest): SimulateResponse {
  const resolve = (value: string): string | null => {
    const trimmed = value.trim();
    if (ipv4ToInt(trimmed) != null) return trimmed;
    if (trimmed.includes("/")) {
      const [base] = trimmed.split("/");
      return ipv4ToInt(base) != null ? base : null;
    }
    return nodes.find((node) => node.name === trimmed)?.overlay_ip ?? null;
  };
  const src = resolve(req.src);
  const dst = resolve(req.dst);
  if (!src) throw new MockHttpError({ status: 400, code: "simulate_failed", message: `origem '${req.src}' não encontrada` });
  if (!dst) throw new MockHttpError({ status: 400, code: "simulate_failed", message: `destino '${req.dst}' não encontrado` });
  const protocol = parseProtocol(req.protocol) ?? 6;
  const port = req.port ?? null;
  if ((protocol === 6 || protocol === 17) && port == null) {
    throw new MockHttpError({ status: 400, code: "simulate_failed", message: "informe a porta para tcp/udp" });
  }
  for (let index = 0; index < compiledAcl.rules.length; index += 1) {
    const rule = compiledAcl.rules[index];
    if (!ipInCidr(src, rule.src) || !ipInCidr(dst, rule.dst)) continue;
    if (rule.protocol != null && rule.protocol !== protocol) continue;
    if (protocol !== 1 && rule.ports.length > 0) {
      if (port == null || !rule.ports.some((range) => port >= range.start && port <= range.end)) continue;
    }
    return {
      allowed: rule.action === "accept",
      rule_index: index,
      reason: `regra ${index + 1} (${rule.action === "accept" ? "Accept" : "Deny"}, protocolo ${
        rule.protocol == null ? "any" : rule.protocol
      }, portas ${rule.ports.length === 0 ? "any" : rule.ports.map((range) => range.start === range.end ? String(range.start) : `${range.start}-${range.end}`).join(",")})`,
    };
  }
  return {
    allowed: compiledAcl.default_allow,
    rule_index: null,
    reason: compiledAcl.default_allow
      ? "nenhuma regra casou; política aberta (default allow)"
      : "nenhuma regra casou; deny implícito",
  };
}

// ---------------------------------------------------------------------------
// Derivações
// ---------------------------------------------------------------------------

function buildOverview(): Overview {
  let online = 0;
  let pending = 0;
  let directPeers = 0;
  let relayPeers = 0;
  for (const node of nodes) {
    if (node.online || node.relay_connected) online += 1;
    if (node.status === "pending") pending += 1;
    for (const peerStat of node.reported_peers) {
      if (peerStat.path === "direct") directPeers += 1;
      if (peerStat.path === "relay") relayPeers += 1;
    }
  }
  return {
    network,
    version: 12,
    nodes_total: nodes.length,
    nodes_online: online,
    nodes_pending: pending,
    relay_connected: nodes.filter((node) => node.relay_connected).length,
    direct_pairs: Math.floor(directPeers / 2),
    relay_pairs: Math.floor(relayPeers / 2),
    control_uptime_secs: 3 * 86_400 + 4 * 3600 + 23 * 60,
    admin,
  };
}

function buildMeta(): VersionInfo {
  return {
    version: "0.1.0",
    protocol: 1,
    control_pubkey: "wm_mock_ed25519_control_public_key_000000000000",
    network,
    relay: `${window.location.origin}/relay`,
    stun: ["stun:127.0.0.1:3478"],
  };
}

function buildSettings(): SettingsView {
  return {
    network,
    approval_required: true,
    relay_url: `${window.location.origin}/relay`,
    stun: ["stun:127.0.0.1:3478"],
    protocol: 1,
    version: "0.1.0",
  };
}

function integrationNodes(): IntegrationNodesResponse {
  const active: IntegrationNode[] = nodes
    .filter((node) => node.status === "active")
    .map((node) => ({
      id: node.id,
      name: node.name,
      hostname: node.hostname,
      overlay_ip: node.overlay_ip,
      tags: node.tags,
      labels: node.labels,
      online: node.online || node.relay_connected,
      last_seen: node.last_seen,
    }));
  return { nodes: active, server_time: new Date().toISOString() };
}

// ---------------------------------------------------------------------------
// Roteamento do mock
// ---------------------------------------------------------------------------

function requireCsrf(headers: Headers): void {
  const provided = headers.get("x-wm-csrf");
  if (!provided) {
    throw new MockHttpError({ status: 403, code: "csrf_missing", message: "cabeçalho X-WM-CSRF ausente" });
  }
  if (provided !== csrf) {
    throw new MockHttpError({ status: 403, code: "csrf_invalid", message: "CSRF inválido" });
  }
}

function readBody(init: RequestInit | undefined): JsonRecord {
  if (typeof init?.body !== "string") return {};
  try {
    const parsed: unknown = JSON.parse(init.body);
    return isRecord(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

function nodeByName(name: string): NodeView | undefined {
  return nodes.find((node) => node.name === name);
}

async function handleApi(url: URL, init: RequestInit | undefined): Promise<Response> {
  const method = (init?.method ?? "GET").toUpperCase();
  const path = url.pathname.replace(/^\/api\/v1/, "");
  const headers = new Headers(init?.headers);

  // Endpoints públicos.
  if (path === "/meta" && method === "GET") return json(buildMeta());

  if (path === "/integration/nodes" && method === "GET") {
    const authorization = headers.get("authorization");
    const token = authorization?.startsWith("Bearer ") ? authorization.slice(7) : null;
    if (!token || !serviceSecrets.has(token)) {
      return fail(401, "service_token_invalid", "token inválido");
    }
    return json(integrationNodes());
  }

  if (path === "/admin/login" && method === "POST") {
    const body = readBody(init);
    const username = typeof body.username === "string" ? body.username.trim() : "";
    const password = typeof body.password === "string" ? body.password : "";
    if (username !== "admin" || password !== "admin") {
      addAudit(`login:127.0.0.1`, "admin.login_failed", username, null, "127.0.0.1");
      return fail(401, "invalid_credentials", "usuário ou senha inválidos");
    }
    authenticated = true;
    csrf = `wmcsrf_mock_${Math.random().toString(36).slice(2)}`;
    addAudit("admin:admin", "admin.login", "admin", null, "127.0.0.1");
    const response: LoginResponse = { user: admin, csrf_token: csrf };
    return json(response);
  }

  if (!authenticated) return fail(401, "not_authenticated", "sessão ausente");

  if (path === "/admin/logout" && method === "POST") {
    requireCsrf(headers);
    authenticated = false;
    csrf = "";
    return json({ ok: true });
  }

  if (path === "/admin/me" && method === "GET") return json(admin);
  if (path === "/admin/session" && method === "GET") {
    return json({ user: admin, csrf_token: csrf });
  }
  if (path === "/admin/overview" && method === "GET") return json(buildOverview());
  if (path === "/admin/nodes" && method === "GET") return json(nodes);
  if (path === "/admin/join-tokens" && method === "GET") return json(joinTokens);
  if (path === "/admin/acl" && method === "GET") {
    const view: AclView = {
      policy: currentPolicy,
      compiled: compiledAcl,
      updated_at: aclUpdatedAt,
    };
    return json(view);
  }
  if (path === "/admin/acl/validate" && method === "POST") {
    requireCsrf(headers);
    const body = readBody(init);
    const yamlText = typeof body.yaml === "string" ? body.yaml : "";
    const policy = parsePolicyYaml(yamlText);
    const { compiled, warnings } = compilePolicy(policy);
    const response: ValidateAclResponse = { ok: true, policy, compiled, warnings };
    return json(response);
  }
  if (path === "/admin/acl/simulate" && method === "POST") {
    requireCsrf(headers);
    const body = readBody(init);
    const request: SimulateRequest = {
      src: typeof body.src === "string" ? body.src : "",
      dst: typeof body.dst === "string" ? body.dst : "",
      protocol: typeof body.protocol === "string" ? body.protocol : null,
      port: typeof body.port === "number" ? body.port : null,
    };
    return json(simulate(request));
  }
  if (path === "/admin/audit" && method === "GET") {
    const limitRaw = Number(url.searchParams.get("limit") ?? "200");
    const limit = Number.isFinite(limitRaw) ? Math.min(1000, Math.max(1, Math.floor(limitRaw))) : 200;
    return json(audits.slice(0, limit));
  }
  if (path === "/admin/service-tokens" && method === "GET") return json(serviceTokens);
  if (path === "/admin/settings" && method === "GET") return json(buildSettings());

  if (path === "/admin/acl" && method === "PUT") {
    requireCsrf(headers);
    const body = readBody(init);
    const yamlText = typeof body.yaml === "string" ? body.yaml : "";
    const parsed = parsePolicyYaml(yamlText);
    const { compiled } = compilePolicy(parsed);
    const policy: AclPolicy = {
      ...parsed,
      version: Math.max(parsed.version, compiledAcl.version + 1),
    };
    currentPolicy = policy;
    compiledAcl = { ...compiled, version: policy.version };
    aclUpdatedAt = new Date().toISOString();
    addAudit("admin:admin", "acl.update", "policy", `rules=${compiled.rules.length}`, "127.0.0.1");
    const view: AclView = { policy, compiled: compiledAcl, updated_at: aclUpdatedAt };
    return json(view);
  }

  if (path.startsWith("/admin/nodes/")) {
    requireCsrf(headers);
    const rest = path.slice("/admin/nodes/".length);
    const [id, action] = rest.split("/");
    const index = nodes.findIndex((node) => node.id === id);
    if (index < 0) return fail(404, "node_not_found", "nó não encontrado");

    if (action === "approve" && method === "POST") {
      const node = nodes[index];
      const updated: NodeView = {
        ...node,
        status: "active",
        approved_at: new Date().toISOString(),
        disabled_at: null,
      };
      nodes = nodes.map((item) => (item.id === id ? updated : item));
      addAudit("admin:admin", "node.approve", node.name, null, "127.0.0.1");
      return json(updated);
    }

    if (!action && method === "PATCH") {
      const body = readBody(init);
      const current = nodes[index];
      const patch = body as Partial<UpdateNodeRequest>;
      const nextName = typeof patch.name === "string" ? patch.name.trim() : current.name;
      if (!/^[A-Za-z0-9._-]{1,63}$/.test(nextName) || nextName.startsWith(".")) {
        return fail(400, "invalid_name", "nome inválido");
      }
      if (nextName !== current.name && nodeByName(nextName)) {
        return fail(409, "name_taken", "nome já está em uso");
      }
      const tags = Array.isArray(patch.tags) ? stringArray(patch.tags) : current.tags;
      const labels = isRecord(patch.labels)
        ? Object.fromEntries(
            Object.entries(patch.labels).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
          )
        : current.labels;
      const status = isStatus(patch.status) ? patch.status : current.status;
      const updated: NodeView = {
        ...current,
        name: nextName,
        tags,
        labels,
        status,
        approved_at: status === "active" ? current.approved_at ?? new Date().toISOString() : current.approved_at,
        disabled_at: status === "disabled" ? new Date().toISOString() : current.disabled_at,
      };
      nodes = nodes.map((item) => (item.id === id ? updated : item));
      addAudit("admin:admin", "node.update", current.name, JSON.stringify(patch), "127.0.0.1");
      return json(updated);
    }

    if (!action && method === "DELETE") {
      const node = nodes[index];
      const updated: NodeView = {
        ...node,
        status: "disabled",
        disabled_at: new Date().toISOString(),
      };
      nodes = nodes.map((item) => (item.id === id ? updated : item));
      addAudit("admin:admin", "node.revoke", node.name, null, "127.0.0.1");
      return json({ ok: true });
    }
  }

  if (path.startsWith("/admin/join-tokens/") && method === "DELETE") {
    requireCsrf(headers);
    const id = path.slice("/admin/join-tokens/".length);
    const token = joinTokens.find((item) => item.id === id);
    if (!token) return fail(404, "join_token_not_found", "token não encontrado");
    joinTokens = joinTokens.map((item) =>
      item.id === id ? { ...item, revoked_at: new Date().toISOString() } : item,
    );
    addAudit("admin:admin", "join_token.revoke", id, null, "127.0.0.1");
    return json({ ok: true });
  }

  if (path === "/admin/join-tokens" && method === "POST") {
    requireCsrf(headers);
    const body = readBody(init);
    const request = body as Partial<CreateJoinTokenRequest>;
    const name = typeof request.name === "string" ? request.name.trim() : "";
    if (!name || name.length > 64) return fail(400, "invalid_name", "nome do token inválido");
    const ttl = typeof request.ttl_secs === "number" ? request.ttl_secs : 86_400;
    const maxUses = typeof request.max_uses === "number" ? request.max_uses : 1;
    const tags = stringArray(request.tags);
    const labels = isRecord(request.labels)
      ? Object.fromEntries(
          Object.entries(request.labels).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
        )
      : {};
    const id = crypto.randomUUID();
    const rawToken = `wmj_mock_${crypto.randomUUID().replace(/-/g, "")}`;
    const view: JoinTokenView = {
      id,
      name,
      prefix: rawToken.slice(0, 12),
      tags,
      labels,
      max_uses: Math.min(1000, Math.max(1, Math.floor(maxUses))),
      uses: 0,
      expires_at: new Date(Date.now() + Math.min(30 * 86_400, Math.max(60, ttl)) * 1000).toISOString(),
      created_at: new Date().toISOString(),
      revoked_at: null,
      token: rawToken,
    };
    joinTokens = [{ ...view, token: undefined }, ...joinTokens];
    addAudit("admin:admin", "join_token.create", id, name, "127.0.0.1");
    return json(view);
  }

  if (path === "/admin/service-tokens" && method === "POST") {
    requireCsrf(headers);
    const body = readBody(init);
    const request = body as Partial<CreateServiceTokenRequest>;
    const name = typeof request.name === "string" ? request.name.trim() : "";
    if (!name) return fail(400, "invalid_name", "nome do token inválido");
    const rawToken = `wms_mock_${crypto.randomUUID().replace(/-/g, "")}`;
    const view: ServiceTokenView = {
      id: crypto.randomUUID(),
      name,
      scopes: ["integration:nodes:read"],
      created_at: new Date().toISOString(),
      last_used_at: null,
      token: rawToken,
    };
    serviceTokens = [{ ...view, token: undefined }, ...serviceTokens];
    serviceSecrets.set(rawToken, view.id);
    addAudit("admin:admin", "service_token.create", view.id, name, "127.0.0.1");
    return json(view);
  }

  if (path.startsWith("/admin/service-tokens/") && method === "DELETE") {
    requireCsrf(headers);
    const id = path.slice("/admin/service-tokens/".length);
    const token = serviceTokens.find((item) => item.id === id);
    if (!token) return fail(404, "service_token_not_found", "token não encontrado");
    serviceTokens = serviceTokens.filter((item) => item.id !== id);
    for (const [secret, tokenId] of serviceSecrets) {
      if (tokenId === id) serviceSecrets.delete(secret);
    }
    addAudit("admin:admin", "service_token.revoke", id, null, "127.0.0.1");
    return json({ ok: true });
  }

  return fail(404, "not_found", "rota não encontrada");
}

let installed = false;

export function installMockApi(): void {
  if (installed) return;
  installed = true;
  const originalFetch = window.fetch.bind(window);

  window.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const rawUrl =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    let url: URL;
    try {
      url = new URL(rawUrl, window.location.origin);
    } catch {
      return originalFetch(input, init);
    }
    if (!url.pathname.startsWith("/api/v1/")) return originalFetch(input, init);

    await delay(90 + Math.floor(Math.random() * 120));
    try {
      return await handleApi(url, init);
    } catch (error) {
      if (error instanceof MockHttpError) return fail(error.status, error.code, error.message);
      console.error("[wired-magic mock]", error);
      return fail(500, "internal", "erro interno do mock");
    }
  };

  console.info(
    "[wired-magic] modo mock ativo (VITE_MOCK=1) — credenciais admin/admin; estado em memória, sem backend.",
  );
}
