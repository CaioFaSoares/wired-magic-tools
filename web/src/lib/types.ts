// Tipos espelhando `crates/wm-proto/src/lib.rs` (contrato JSON da API v1).
// Datas são strings RFC 3339; IPs e CIDRs são strings na serialização do Rust.

export type IsoDate = string;
export type Uuid = string;
export type NodeId = string;
export type LabelMap = Record<string, string>;

export type NodeStatus = "pending" | "active" | "disabled";
export type NatType = "unknown" | "cone" | "symmetric";
export type PathKind = "none" | "direct" | "relay";
export type EndpointKind = "stun" | "lan" | "previous" | "manual";
export type AclAction = "accept" | "deny";

export interface EndpointCandidate {
  addr: string;
  kind: EndpointKind;
}

export interface PeerInfo {
  id: NodeId;
  name: string;
  hostname: string;
  overlay_ip: string;
  wg_public_key: string;
  tags: string[];
  labels: LabelMap;
  endpoints: EndpointCandidate[];
  online: boolean;
  last_seen: IsoDate | null;
}

export interface PeerStats {
  peer_id: NodeId;
  path: PathKind;
  rtt_ms: number | null;
  last_handshake_secs: number | null;
  rx_bytes: number;
  tx_bytes: number;
  relay_online: boolean;
  endpoint: string | null;
  last_error: string | null;
}

export interface NodeView extends PeerInfo {
  status: NodeStatus;
  created_at: IsoDate;
  approved_at: IsoDate | null;
  disabled_at: IsoDate | null;
  nat_type: NatType;
  relay_connected: boolean;
  reported_peers: PeerStats[];
}

export interface UpdateNodeRequest {
  name?: string;
  tags?: string[];
  labels?: LabelMap;
  status?: NodeStatus;
}

export interface AdminUser {
  id: Uuid;
  username: string;
}

export interface AdminSession {
  user: AdminUser;
  csrf_token: string;
}

export interface LoginRequest {
  username: string;
  password: string;
}

export interface LoginResponse {
  user: AdminUser;
  csrf_token: string;
}

export interface CreateJoinTokenRequest {
  name: string;
  ttl_secs: number;
  max_uses: number;
  tags: string[];
  labels: LabelMap;
}

export interface JoinTokenView {
  id: Uuid;
  name: string;
  prefix: string;
  tags: string[];
  labels: LabelMap;
  max_uses: number;
  uses: number;
  expires_at: IsoDate;
  created_at: IsoDate;
  revoked_at: IsoDate | null;
  /** Presente somente na resposta de criação. */
  token?: string;
}

export interface PortRange {
  start: number;
  end: number;
}

export interface CompiledRule {
  action: AclAction;
  src: string;
  dst: string;
  protocol: number | null;
  ports: PortRange[];
  comment?: string | null;
}

export interface CompiledAcl {
  version: number;
  default_allow: boolean;
  rules: CompiledRule[];
}

export interface AclRule {
  action: AclAction;
  src: string[];
  dst: string[];
  protocol?: string | null;
  ports?: string[];
  comment?: string | null;
}

export interface AclPolicy {
  version: number;
  rules: AclRule[];
}

export interface AclView {
  policy: AclPolicy;
  compiled: CompiledAcl;
  updated_at: IsoDate | null;
}

export interface ValidateAclResponse {
  ok: boolean;
  policy: AclPolicy;
  compiled: CompiledAcl;
  warnings: string[];
}

export interface SimulateRequest {
  src: string;
  dst: string;
  protocol?: string | null;
  port?: number | null;
}

export interface SimulateResponse {
  allowed: boolean;
  rule_index: number | null;
  reason: string;
}

export interface NetworkInfo {
  name: string;
  overlay_cidr: string;
  dns_zone: string;
  mtu: number;
  server_address: string;
}

export interface VersionInfo {
  version: string;
  protocol: number;
  control_pubkey: string;
  network: NetworkInfo;
  relay: string | null;
  stun: string[];
}

export interface Overview {
  network: NetworkInfo;
  version: number;
  nodes_total: number;
  nodes_online: number;
  nodes_pending: number;
  relay_connected: number;
  direct_pairs: number;
  relay_pairs: number;
  control_uptime_secs: number;
  admin: AdminUser;
}

export interface AuditEventView {
  id: number;
  ts: IsoDate;
  actor: string;
  action: string;
  target: string;
  detail: string | null;
  ip: string | null;
}

export interface ServiceTokenView {
  id: Uuid;
  name: string;
  scopes: string[];
  created_at: IsoDate;
  last_used_at: IsoDate | null;
  /** Presente somente na resposta de criação. */
  token?: string;
}

export interface CreateServiceTokenRequest {
  name: string;
  scopes: string[];
}

export interface SettingsView {
  network: NetworkInfo;
  approval_required: boolean;
  relay_url: string | null;
  stun: string[];
  protocol: number;
  version: string;
}

export interface IntegrationNode {
  id: NodeId;
  name: string;
  hostname: string;
  overlay_ip: string;
  tags: string[];
  labels: LabelMap;
  online: boolean;
  last_seen: IsoDate | null;
}

export interface IntegrationNodesResponse {
  nodes: IntegrationNode[];
  server_time: IsoDate;
}

// Envelope de erro: { "error": { "code", "message", "details"? } }
export interface ApiErrorData {
  code: string;
  message: string;
  details?: unknown;
}

export interface ApiErrorBody {
  error: ApiErrorData;
}

// Eventos SSE de `/api/v1/admin/events` (tag "type", content "data").
export type ControlEvent =
  | { type: "node_updated"; data: NodeView }
  | { type: "node_removed"; data: { id: NodeId } }
  | { type: "join_token_created"; data: JoinTokenView }
  | { type: "join_token_revoked"; data: { id: Uuid } }
  | { type: "acl_updated"; data: { version: number } }
  | { type: "relay_updated"; data: unknown }
  | { type: "audit"; data: AuditEventView }
  | { type: "presence"; data: { node_id: NodeId; online: boolean } }
  | { type: "extension_status"; data: { node_id: NodeId; extension_id: string; state: ExtensionState } };

export interface ApiOk {
  ok: boolean;
}

// ---------------------------------------------------------------------------
// Extensões e cloudflared
// ---------------------------------------------------------------------------

export type ExtensionState = "stopped" | "starting" | "running" | "error";
export type ExtensionDriver = "none" | "exec" | "cloudflared";
export type CloudflaredMode = "quick" | "named";

export interface ExtensionStatus {
  id: string;
  name: string;
  driver: string;
  state: ExtensionState;
  message?: string | null;
  public_urls: string[];
  details?: unknown;
  updated_at: IsoDate;
}

export interface ExtensionView {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  homepage?: string | null;
  builtin: boolean;
  node_driver: ExtensionDriver;
  ui: string;
  config_example?: unknown;
  nodes_configured: number;
  nodes_running: number;
  warning?: string | null;
}

export interface ExtensionNodeView {
  node_id: NodeId;
  node_name: string;
  overlay_ip: string;
  online: boolean;
  enabled: boolean;
  active: boolean;
  activation_revision: number;
  revision: number;
  config: unknown;
  status?: ExtensionStatus | null;
  updated_at?: IsoDate | null;
}

export interface ExtensionDetail extends ExtensionView {
  nodes: ExtensionNodeView[];
}

export interface ReloadExtensionsResponse {
  manifests: ExtensionView[];
  warnings: string[];
}

export interface ExtensionNodeConfigInput {
  enabled: boolean;
  config: unknown;
}

export interface CloudflaredRouteLive {
  state: string;
  public_url?: string | null;
  message?: string | null;
}

export interface CloudflaredRouteView {
  id: Uuid;
  project: string;
  node_id: NodeId;
  node_name: string;
  node_online: boolean;
  local_url: string;
  mode: CloudflaredMode;
  hostname?: string | null;
  enabled: boolean;
  created_at: IsoDate;
  updated_at: IsoDate;
  status?: CloudflaredRouteLive | null;
}

export interface CloudflaredNodeView {
  node_id: NodeId;
  node_name: string;
  hostname: string;
  overlay_ip: string;
  online: boolean;
  active: boolean;
  activation_revision: number;
  binary: string;
  tunnel_id?: string | null;
  has_credentials: boolean;
  revision: number;
  updated_at?: IsoDate | null;
  status?: ExtensionStatus | null;
  quick_routes: number;
  named_routes: number;
}

export interface CloudflaredOverview {
  extension: ExtensionView;
  nodes: CloudflaredNodeView[];
  routes: CloudflaredRouteView[];
}

export interface CloudflaredRouteInput {
  project: string;
  node_id: NodeId;
  local_url: string;
  mode: CloudflaredMode;
  hostname?: string | null;
  enabled: boolean;
}

export interface CloudflaredNodeSettingsInput {
  binary?: string;
  tunnel_id?: string;
  credentials_json?: string;
}
