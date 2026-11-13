import { ApiError } from "./api";

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.status === 0) return error.message;
    if (error.status === 401) return "Sua sessão expirou. Entre novamente para continuar.";
    if (error.status === 403 && error.code.startsWith("csrf_")) {
      return "A proteção CSRF recusou a ação. Recarregue a página e entre novamente.";
    }
    if (error.status === 429) return error.message || "Muitas tentativas. Aguarde e tente de novo.";
    return error.message;
  }
  if (error instanceof Error && error.message) return error.message;
  return "Ocorreu um erro inesperado.";
}
