import type {
  AclView,
  AdminUser,
  ApiErrorBody,
  ApiOk,
  AuditEventView,
  CloudflaredNodeSettingsInput,
  CloudflaredOverview,
  CloudflaredRouteInput,
  CloudflaredRouteView,
  CreateJoinTokenRequest,
  CreateServiceTokenRequest,
  ExtensionDetail,
  ExtensionNodeConfigInput,
  ExtensionNodeView,
  ExtensionView,
  IntegrationNodesResponse,
  JoinTokenView,
  AdminSession,
  LoginRequest,
  LoginResponse,
  NodeRequirements,
  NodeView,
  ReloadExtensionsResponse,
  Overview,
  ServiceTokenView,
  SettingsView,
  SimulateRequest,
  SimulateResponse,
  UpdateNodeRequest,
  ValidateAclResponse,
  VersionInfo,
} from "./types";

const API_BASE = "/api/v1";
const CSRF_STORAGE_KEY = "wm.csrf";

let csrfToken: string | null = null;

interface ApiHandlers {
  onUnauthorized?: () => void;
  onCsrfError?: () => void;
}

let handlers: ApiHandlers = {};

/** Registra callbacks globais de sessão. Devolve a função de limpeza. */
export function setApiHandlers(next: ApiHandlers): () => void {
  handlers = next;
  return () => {
    if (handlers.onUnauthorized === next.onUnauthorized && handlers.onCsrfError === next.onCsrfError) {
      handlers = {};
    }
  };
}

export function getCsrfToken(): string | null {
  return csrfToken;
}

export function loadStoredCsrf(): string | null {
  try {
    const stored = sessionStorage.getItem(CSRF_STORAGE_KEY);
    csrfToken = stored;
    return stored;
  } catch {
    csrfToken = null;
    return null;
  }
}

export function setCsrfToken(token: string | null): void {
  csrfToken = token;
  try {
    if (token) sessionStorage.setItem(CSRF_STORAGE_KEY, token);
    else sessionStorage.removeItem(CSRF_STORAGE_KEY);
  } catch {
    // sessionStorage pode estar bloqueado por política do navegador.
  }
}

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details?: unknown;

  constructor(status: number, code: string, message: string, details?: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

interface RequestOptions {
  method?: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  body?: unknown;
  /** Mutating calls exigem X-WM-CSRF por padrão. */
  needsCsrf?: boolean;
  bearer?: string;
  /** Não dispara o handler global de 401 (login, /me, teste de integração). */
  skipAuthHandlers?: boolean;
  signal?: AbortSignal;
}

function isApiErrorBody(value: unknown): value is ApiErrorBody {
  if (typeof value !== "object" || value === null || !("error" in value)) return false;
  const error = (value as { error: unknown }).error;
  return (
    typeof error === "object" &&
    error !== null &&
    typeof (error as { code?: unknown }).code === "string" &&
    typeof (error as { message?: unknown }).message === "string"
  );
}

function notifyCsrfError(): void {
  handlers.onCsrfError?.();
}

async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const method = options.method ?? "GET";
  const needsCsrf = options.needsCsrf ?? method !== "GET";
  const headers = new Headers({ Accept: "application/json" });

  if (options.body !== undefined) {
    headers.set("Content-Type", "application/json");
  }
  if (options.bearer) {
    headers.set("Authorization", `Bearer ${options.bearer}`);
  }
  if (needsCsrf) {
    if (!csrfToken) {
      notifyCsrfError();
      throw new ApiError(
        0,
        "csrf_unavailable",
        "Token CSRF indisponível nesta aba. Saia e entre novamente para continuar.",
      );
    }
    headers.set("X-WM-CSRF", csrfToken);
  }

  let response: Response;
  try {
    response = await fetch(`${API_BASE}${path}`, {
      method,
      headers,
      credentials: "same-origin",
      cache: "no-store",
      body: options.body === undefined ? undefined : JSON.stringify(options.body),
      signal: options.signal,
    });
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") throw error;
    throw new ApiError(
      0,
      "network_error",
      "Não foi possível falar com o control plane. Verifique a conexão e tente novamente.",
    );
  }

  const text = await response.text();
  let payload: unknown = null;
  if (text.length > 0) {
    try {
      payload = JSON.parse(text) as unknown;
    } catch {
      payload = null;
    }
  }

  if (!response.ok) {
    let code = `http_${response.status}`;
    let message = `A requisição falhou (HTTP ${response.status}).`;
    let details: unknown;
    if (isApiErrorBody(payload)) {
      code = payload.error.code;
      message = payload.error.message;
      details = payload.error.details;
    } else if (text.trim().length > 0 && !text.trimStart().startsWith("<")) {
      message = text.trim().slice(0, 400);
    }

    if (!options.skipAuthHandlers && response.status === 401) {
      handlers.onUnauthorized?.();
    }
    if (response.status === 403 && code.startsWith("csrf_")) {
      notifyCsrfError();
    }
    throw new ApiError(response.status, code, message, details);
  }

  return payload as T;
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

export const api = {
  /** Público. */
  meta(signal?: AbortSignal): Promise<VersionInfo> {
    return request<VersionInfo>("/meta", { skipAuthHandlers: true, signal });
  },

  login(body: LoginRequest): Promise<LoginResponse> {
    return request<LoginResponse>("/admin/login", {
      method: "POST",
      body,
      needsCsrf: false,
      skipAuthHandlers: true,
    });
  },

  logout(): Promise<ApiOk> {
    return request<ApiOk>("/admin/logout", { method: "POST" });
  },

  me(): Promise<AdminUser> {
    return request<AdminUser>("/admin/me", { skipAuthHandlers: true });
  },

  /** Devolve o usuário e o CSRF da sessão; usado para reabrir uma aba sem perder mutações. */
  session(): Promise<AdminSession> {
    return request<AdminSession>("/admin/session", { skipAuthHandlers: true });
  },

  overview(signal?: AbortSignal): Promise<Overview> {
    return request<Overview>("/admin/overview", { signal });
  },

  nodes(signal?: AbortSignal): Promise<NodeView[]> {
    return request<NodeView[]>("/admin/nodes", { signal });
  },

  updateNode(id: string, body: UpdateNodeRequest): Promise<NodeView> {
    return request<NodeView>(`/admin/nodes/${encodeURIComponent(id)}`, {
      method: "PATCH",
      body,
    });
  },

  setNodeRequirements(id: string, body: NodeRequirements): Promise<NodeView> {
    return request<NodeView>(`/admin/nodes/${encodeURIComponent(id)}/requirements`, {
      method: "PUT",
      body,
    });
  },

  approveNode(id: string): Promise<NodeView> {
    return request<NodeView>(`/admin/nodes/${encodeURIComponent(id)}/approve`, {
      method: "POST",
    });
  },

  revokeNode(id: string): Promise<ApiOk> {
    return request<ApiOk>(`/admin/nodes/${encodeURIComponent(id)}`, { method: "DELETE" });
  },

  joinTokens(signal?: AbortSignal): Promise<JoinTokenView[]> {
    return request<JoinTokenView[]>("/admin/join-tokens", { signal });
  },

  createJoinToken(body: CreateJoinTokenRequest): Promise<JoinTokenView> {
    return request<JoinTokenView>("/admin/join-tokens", { method: "POST", body });
  },

  revokeJoinToken(id: string): Promise<ApiOk> {
    return request<ApiOk>(`/admin/join-tokens/${encodeURIComponent(id)}`, { method: "DELETE" });
  },

  acl(signal?: AbortSignal): Promise<AclView> {
    return request<AclView>("/admin/acl", { signal });
  },

  putAcl(yaml: string): Promise<AclView> {
    return request<AclView>("/admin/acl", { method: "PUT", body: { yaml } });
  },

  validateAcl(yaml: string): Promise<ValidateAclResponse> {
    return request<ValidateAclResponse>("/admin/acl/validate", {
      method: "POST",
      body: { yaml },
    });
  },

  simulateAcl(body: SimulateRequest): Promise<SimulateResponse> {
    return request<SimulateResponse>("/admin/acl/simulate", { method: "POST", body });
  },

  audit(limit = 200, signal?: AbortSignal): Promise<AuditEventView[]> {
    return request<AuditEventView[]>(`/admin/audit?limit=${encodeURIComponent(String(limit))}`, {
      signal,
    });
  },

  serviceTokens(signal?: AbortSignal): Promise<ServiceTokenView[]> {
    return request<ServiceTokenView[]>("/admin/service-tokens", { signal });
  },

  createServiceToken(body: CreateServiceTokenRequest): Promise<ServiceTokenView> {
    return request<ServiceTokenView>("/admin/service-tokens", { method: "POST", body });
  },

  revokeServiceToken(id: string): Promise<ApiOk> {
    return request<ApiOk>(`/admin/service-tokens/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
  },

  settings(signal?: AbortSignal): Promise<SettingsView> {
    return request<SettingsView>("/admin/settings", { signal });
  },

  // Extensões ---------------------------------------------------------------

  extensions(signal?: AbortSignal): Promise<ExtensionView[]> {
    return request<ExtensionView[]>("/admin/extensions", { signal });
  },

  extension(id: string, signal?: AbortSignal): Promise<ExtensionDetail> {
    return request<ExtensionDetail>(`/admin/extensions/${encodeURIComponent(id)}`, { signal });
  },

  reloadExtensions(): Promise<ReloadExtensionsResponse> {
    return request<ReloadExtensionsResponse>("/admin/extensions/reload", { method: "POST", body: {} });
  },

  putExtensionNodeConfig(
    id: string,
    nodeId: string,
    body: ExtensionNodeConfigInput,
  ): Promise<ExtensionNodeView> {
    return request<ExtensionNodeView>(
      `/admin/extensions/${encodeURIComponent(id)}/nodes/${encodeURIComponent(nodeId)}/config`,
      { method: "PUT", body },
    );
  },

  setExtensionActivation(
    id: string,
    nodeId: string,
    active: boolean,
  ): Promise<ExtensionNodeView> {
    return request<ExtensionNodeView>(
      `/admin/extensions/${encodeURIComponent(id)}/nodes/${encodeURIComponent(nodeId)}/activation`,
      { method: "PUT", body: { active } },
    );
  },

  setExtensionRequest(
    id: string,
    nodeId: string,
    requested: boolean,
    message: string,
  ): Promise<ExtensionNodeView> {
    return request<ExtensionNodeView>(
      `/admin/extensions/${encodeURIComponent(id)}/nodes/${encodeURIComponent(nodeId)}/request`,
      { method: "PUT", body: { requested, message } },
    );
  },

  deleteExtensionNodeConfig(id: string, nodeId: string): Promise<ApiOk> {
    return request<ApiOk>(
      `/admin/extensions/${encodeURIComponent(id)}/nodes/${encodeURIComponent(nodeId)}/config`,
      { method: "DELETE" },
    );
  },

  cloudflaredOverview(signal?: AbortSignal): Promise<CloudflaredOverview> {
    return request<CloudflaredOverview>("/admin/extensions/cloudflared/overview", { signal });
  },

  createCloudflaredRoute(body: CloudflaredRouteInput): Promise<CloudflaredRouteView> {
    return request<CloudflaredRouteView>("/admin/extensions/cloudflared/routes", {
      method: "POST",
      body,
    });
  },

  updateCloudflaredRoute(id: string, body: CloudflaredRouteInput): Promise<CloudflaredRouteView> {
    return request<CloudflaredRouteView>(
      `/admin/extensions/cloudflared/routes/${encodeURIComponent(id)}`,
      { method: "PUT", body },
    );
  },

  deleteCloudflaredRoute(id: string): Promise<ApiOk> {
    return request<ApiOk>(`/admin/extensions/cloudflared/routes/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
  },

  updateCloudflaredNode(
    nodeId: string,
    body: CloudflaredNodeSettingsInput,
  ): Promise<CloudflaredOverview["nodes"][number]> {
    return request<CloudflaredOverview["nodes"][number]>(
      `/admin/extensions/cloudflared/nodes/${encodeURIComponent(nodeId)}`,
      { method: "PUT", body },
    );
  },

  restartCloudflaredNode(nodeId: string): Promise<ApiOk> {
    return request<ApiOk>(
      `/admin/extensions/cloudflared/nodes/${encodeURIComponent(nodeId)}/restart`,
      { method: "POST", body: {} },
    );
  },

  /** Chamada direta à API de integração, autenticada pelo token de serviço. */
  integrationNodes(serviceToken: string): Promise<IntegrationNodesResponse> {
    return request<IntegrationNodesResponse>("/integration/nodes", {
      bearer: serviceToken,
      skipAuthHandlers: true,
    });
  },
};
