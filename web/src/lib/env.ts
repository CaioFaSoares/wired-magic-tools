/** Modo mock é opt-in em tempo de build/dev (`VITE_MOCK=1`). */
export const isMockMode: boolean = import.meta.env.VITE_MOCK === "1";

export const API_BASE = "/api/v1";

export function normalizeBaseUrl(value: string): string {
  return value.trim().replace(/\/+$/, "");
}

/**
 * URL usada nos comandos `wm join`. Em produção o painel é servido pelo
 * próprio control plane, então a origem atual é a resposta correta. Em dev,
 * o Vite roda em outra porta e o proxy aponta para 8080.
 */
export function defaultControlUrl(): string {
  const configured = import.meta.env.VITE_CONTROL_URL?.trim();
  if (configured) return normalizeBaseUrl(configured);
  if (import.meta.env.DEV) return "http://127.0.0.1:8080";
  return normalizeBaseUrl(window.location.origin);
}
