const dateTimeFormatter = new Intl.DateTimeFormat("pt-BR", {
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

const dateTimeSecondsFormatter = new Intl.DateTimeFormat("pt-BR", {
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});

const dateFormatter = new Intl.DateTimeFormat("pt-BR", {
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
});

const numberFormatter = new Intl.NumberFormat("pt-BR");

export function parseDate(value: string | null | undefined): Date | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

export function formatDateTime(value: string | null | undefined): string {
  const date = parseDate(value);
  return date ? dateTimeFormatter.format(date) : "—";
}

export function formatDateTimeSeconds(value: string | null | undefined): string {
  const date = parseDate(value);
  return date ? dateTimeSecondsFormatter.format(date) : "—";
}

export function formatDate(value: string | null | undefined): string {
  const date = parseDate(value);
  return date ? dateFormatter.format(date) : "—";
}

export function formatRelative(value: string | null | undefined, now = Date.now()): string {
  const date = parseDate(value);
  if (!date) return "nunca";
  return formatRelativeFromSeconds(Math.round((date.getTime() - now) / 1000));
}

/** `deltaSeconds` positivo = futuro; negativo = passado. */
export function formatRelativeFromSeconds(deltaSeconds: number): string {
  const future = deltaSeconds > 0;
  const abs = Math.abs(deltaSeconds);
  if (abs < 45) return future ? "em instantes" : "agora";
  const prefix = future ? "em" : "há";
  if (abs < 90) return `${prefix} 1 min`;
  if (abs < 3600) return `${prefix} ${Math.round(abs / 60)} min`;
  if (abs < 86_400) return `${prefix} ${Math.round(abs / 3600)} h`;
  if (abs < 2_592_000) return `${prefix} ${Math.round(abs / 86_400)} d`;
  if (abs < 31_536_000) return `${prefix} ${Math.round(abs / 2_592_000)} mês${Math.round(abs / 2_592_000) > 1 ? "es" : ""}`;
  return `${prefix} ${Math.round(abs / 31_536_000)} ano${Math.round(abs / 31_536_000) > 1 ? "s" : ""}`;
}

export function formatDuration(totalSeconds: number | null | undefined): string {
  if (totalSeconds == null || Number.isNaN(totalSeconds)) return "—";
  const total = Math.max(0, Math.floor(totalSeconds));
  const days = Math.floor(total / 86_400);
  const hours = Math.floor((total % 86_400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, "0")}min`;
  if (minutes > 0) return `${minutes}min ${String(seconds).padStart(2, "0")}s`;
  return `${seconds}s`;
}

export function formatUptimeSince(totalSeconds: number): string {
  const date = new Date(Date.now() - Math.max(0, totalSeconds) * 1000);
  return dateTimeFormatter.format(date);
}

export function formatNumber(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return "—";
  return numberFormatter.format(value);
}

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || Number.isNaN(bytes)) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unitIndex]}`;
}

export function formatRtt(rttMs: number | null | undefined): string {
  if (rttMs == null) return "—";
  return `${formatNumber(rttMs)} ms`;
}

export function formatProtocol(protocol: number | null | undefined): string {
  if (protocol == null) return "qualquer";
  if (protocol === 1) return "icmp";
  if (protocol === 6) return "tcp";
  if (protocol === 17) return "udp";
  return `proto ${protocol}`;
}

export function formatPorts(ports: Array<{ start: number; end: number }>): string {
  if (ports.length === 0) return "*";
  return ports
    .map((range) => (range.start === range.end ? String(range.start) : `${range.start}-${range.end}`))
    .join(", ");
}

export function plural(count: number, singular: string, pluralForm: string): string {
  return count === 1 ? singular : pluralForm;
}

export function normalizeSearch(value: string): string {
  return value
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .trim();
}

export function shortenHash(value: string | null | undefined, head = 8, tail = 6): string {
  if (!value) return "—";
  if (value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}
