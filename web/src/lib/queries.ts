import { useQuery, type QueryClient } from "@tanstack/react-query";
import { api } from "./api";

export const qk = {
  meta: ["meta"] as const,
  me: ["me"] as const,
  overview: ["overview"] as const,
  nodes: ["nodes"] as const,
  joinTokens: ["join-tokens"] as const,
  acl: ["acl"] as const,
  audit: (limit: number) => ["audit", limit] as const,
  serviceTokens: ["service-tokens"] as const,
  settings: ["settings"] as const,
  extensions: ["extensions"] as const,
  extension: (id: string) => ["extension", id] as const,
  cloudflared: ["cloudflared"] as const,
};

export function useMeta() {
  return useQuery({
    queryKey: qk.meta,
    queryFn: ({ signal }) => api.meta(signal),
    staleTime: 60_000,
    refetchInterval: 60_000,
    retry: 1,
  });
}

export function useOverview() {
  return useQuery({
    queryKey: qk.overview,
    queryFn: ({ signal }) => api.overview(signal),
    refetchInterval: 30_000,
  });
}

export function useNodes() {
  return useQuery({
    queryKey: qk.nodes,
    queryFn: ({ signal }) => api.nodes(signal),
    refetchInterval: 30_000,
  });
}

export function useJoinTokens() {
  return useQuery({
    queryKey: qk.joinTokens,
    queryFn: ({ signal }) => api.joinTokens(signal),
    refetchInterval: 30_000,
  });
}

export function useAcl() {
  return useQuery({
    queryKey: qk.acl,
    queryFn: ({ signal }) => api.acl(signal),
    staleTime: 15_000,
  });
}

export function useServiceTokens() {
  return useQuery({
    queryKey: qk.serviceTokens,
    queryFn: ({ signal }) => api.serviceTokens(signal),
    refetchInterval: 60_000,
  });
}

export function useSettings() {
  return useQuery({
    queryKey: qk.settings,
    queryFn: ({ signal }) => api.settings(signal),
    staleTime: 60_000,
  });
}

export function useExtensions() {
  return useQuery({
    queryKey: qk.extensions,
    queryFn: ({ signal }) => api.extensions(signal),
    refetchInterval: 20_000,
  });
}

export function useExtension(id: string) {
  return useQuery({
    queryKey: qk.extension(id),
    queryFn: ({ signal }) => api.extension(id, signal),
    enabled: Boolean(id),
    refetchInterval: 10_000,
  });
}

export function useCloudflaredOverview() {
  return useQuery({
    queryKey: qk.cloudflared,
    queryFn: ({ signal }) => api.cloudflaredOverview(signal),
    refetchInterval: 8_000,
  });
}

export function invalidateExtensionQueries(queryClient: QueryClient): void {
  void queryClient.invalidateQueries({ queryKey: qk.extensions });
  void queryClient.invalidateQueries({ queryKey: ["extension"] });
  void queryClient.invalidateQueries({ queryKey: qk.cloudflared });
  void queryClient.invalidateQueries({ queryKey: qk.overview });
}

export function useAudit(limit = 200) {
  return useQuery({
    queryKey: qk.audit(limit),
    queryFn: ({ signal }) => api.audit(limit, signal),
  });
}

export function invalidateNodeQueries(queryClient: QueryClient): void {
  void queryClient.invalidateQueries({ queryKey: qk.nodes });
  void queryClient.invalidateQueries({ queryKey: qk.overview });
}

export function invalidateTokenQueries(queryClient: QueryClient): void {
  void queryClient.invalidateQueries({ queryKey: qk.joinTokens });
  void queryClient.invalidateQueries({ queryKey: qk.audit(200) });
  void queryClient.invalidateQueries({ queryKey: ["audit"] });
}
