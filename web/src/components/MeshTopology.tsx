import { useId } from "react";
import { isNodeOnline, nodeStatusLabel, pathLabel, topologyPath } from "../lib/nodes";
import type { NodeView } from "../lib/types";

interface MeshTopologyProps {
  nodes: NodeView[];
  relayEnabled: boolean;
  maxNodes?: number;
  className?: string;
  caption?: string;
}

interface Point {
  x: number;
  y: number;
}

const WIDTH = 880;
const HEIGHT = 380;
const CENTER: Point = { x: 440, y: 190 };
const RADIUS_X = 280;
const RADIUS_Y = 118;

function truncate(value: string, max: number): string {
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

function layoutNodes(count: number): Point[] {
  if (count <= 0) return [];
  if (count === 1) return [{ x: CENTER.x + RADIUS_X, y: CENTER.y }];
  if (count === 2) {
    return [
      { x: CENTER.x - RADIUS_X, y: CENTER.y },
      { x: CENTER.x + RADIUS_X, y: CENTER.y },
    ];
  }
  return Array.from({ length: count }, (_, index) => {
    const angle = (index / count) * Math.PI * 2 - Math.PI / 2;
    return {
      x: CENTER.x + Math.cos(angle) * RADIUS_X,
      y: CENTER.y + Math.sin(angle) * RADIUS_Y,
    };
  });
}

function lineGeometry(point: Point) {
  const dx = point.x - CENTER.x;
  const dy = point.y - CENTER.y;
  const distance = Math.hypot(dx, dy) || 1;
  const ux = dx / distance;
  const uy = dy / distance;
  return {
    x1: CENTER.x + ux * 44,
    y1: CENTER.y + uy * 44,
    x2: point.x - ux * 20,
    y2: point.y - uy * 20,
  };
}

export function MeshTopology({ nodes, relayEnabled, maxNodes = 10, className, caption }: MeshTopologyProps) {
  const gradientId = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const visible = nodes
    .filter((node) => node.status !== "disabled")
    .sort((a, b) => {
      const online = Number(isNodeOnline(b)) - Number(isNodeOnline(a));
      if (online !== 0) return online;
      return a.name.localeCompare(b.name, "pt-BR");
    })
    .slice(0, maxNodes);
  const points = layoutNodes(visible.length);
  const hiddenCount = nodes.filter((node) => node.status !== "disabled").length - visible.length;

  if (visible.length === 0) {
    return (
      <div
        className={`flex min-h-44 flex-col items-center justify-center rounded-lg border border-dashed border-line-strong/70 bg-white/[0.015] px-4 py-8 text-center ${className ?? ""}`}
      >
        <p className="text-sm font-medium text-ink">Sem nós para desenhar</p>
        <p className="mt-1 max-w-sm text-xs leading-5 text-muted">
          Quando um nó entrar na malha, ele aparece aqui com o caminho reportado para os peers.
        </p>
      </div>
    );
  }

  const ariaLabel = `${caption ?? "Topologia da malha"}: ${visible.length} ${visible.length === 1 ? "nó" : "nós"} ao redor do control plane.`;

  return (
    <figure className={className}>
      <div className="hidden sm:block">
      <svg
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        role="img"
        aria-label={ariaLabel}
        className="h-auto w-full"
      >
        <defs>
          <radialGradient id={`${gradientId}-hub`} cx="35%" cy="30%" r="80%">
            <stop offset="0%" stopColor="#123441" />
            <stop offset="100%" stopColor="#081016" />
          </radialGradient>
          <radialGradient id={`${gradientId}-halo`} cx="50%" cy="50%" r="50%">
            <stop offset="0%" stopColor="#5eead4" stopOpacity="0.13" />
            <stop offset="100%" stopColor="#5eead4" stopOpacity="0" />
          </radialGradient>
        </defs>

        <circle cx={CENTER.x} cy={CENTER.y} r="150" fill={`url(#${gradientId}-halo)`} />
        <circle
          cx={CENTER.x}
          cy={CENTER.y}
          r="98"
          fill="none"
          stroke="#1e2b39"
          strokeWidth="1"
          strokeDasharray="2 6"
        />

        {visible.map((node, index) => {
          const point = points[index];
          const path = topologyPath(node);
          const online = isNodeOnline(node);
          const geometry = lineGeometry(point);
          const color = !online ? "#3b4c5e" : path === "direct" ? "#5eead4" : path === "relay" ? "#fbbf24" : "#52667a";
          const dash = !online ? "2 5" : path === "relay" ? "6 5" : path === "unknown" ? "2 5" : undefined;
          const strokeOpacity = !online ? 0.45 : path === "unknown" ? 0.65 : 0.9;

          const nearCenterX = Math.abs(point.x - CENTER.x) < 110;
          const anchor = nearCenterX ? "middle" : point.x < CENTER.x ? "end" : "start";
          const labelX = nearCenterX ? point.x : point.x < CENTER.x ? point.x - 26 : point.x + 26;
          const isTop = point.y < CENTER.y - 50;
          const labelY = nearCenterX ? (isTop ? point.y + 28 : point.y - 26) : point.y - 3;
          const ipY = nearCenterX ? labelY + 13 : point.y + 11;

          return (
            <g key={node.id}>
              <title>
                {`${node.name} · ${online ? "online" : "offline"} · ${path === "unknown" ? "sem relatório de caminho" : pathLabel[path]} · ${node.overlay_ip}`}
              </title>
              <line
                x1={geometry.x1}
                y1={geometry.y1}
                x2={geometry.x2}
                y2={geometry.y2}
                stroke={color}
                strokeWidth={path === "direct" ? 1.8 : 1.4}
                strokeDasharray={dash}
                strokeOpacity={strokeOpacity}
                strokeLinecap="round"
              />
              <circle cx={point.x} cy={point.y} r="13" fill="#080f15" stroke={color} strokeWidth="1.6" />
              <circle
                cx={point.x}
                cy={point.y}
                r="4"
                fill={online ? color : "#3b4c5e"}
                fillOpacity={online ? 1 : 0.7}
              />
              {node.status === "pending" ? (
                <circle cx={point.x + 11} cy={point.y - 11} r="4" fill="#fbbf24" stroke="#080f15" strokeWidth="1.5" />
              ) : null}
              <text
                x={labelX}
                y={labelY}
                textAnchor={anchor}
                fill={online ? "#dce6f0" : "#93a6ba"}
                fontSize="11"
                fontWeight="600"
              >
                {truncate(node.name, 22)}
                {node.status === "pending" ? " · pendente" : ""}
              </text>
              <text
                x={labelX}
                y={ipY}
                textAnchor={anchor}
                fill="#71859a"
                fontSize="9"
                fontFamily="ui-monospace, SFMono-Regular, Menlo, monospace"
              >
                {node.overlay_ip}
                {!online ? " · offline" : path === "unknown" ? " · s/ relatório" : ` · ${pathLabel[path]}`}
              </text>
            </g>
          );
        })}

        <circle cx={CENTER.x} cy={CENTER.y} r="42" fill={`url(#${gradientId}-hub)`} stroke="#2c3e51" strokeWidth="1.2" />
        <circle cx={CENTER.x} cy={CENTER.y} r="36" fill="none" stroke="#5eead4" strokeOpacity="0.25" strokeWidth="1" />
        <text x={CENTER.x} y={CENTER.y - 2} textAnchor="middle" fill="#dce6f0" fontSize="11" fontWeight="600">
          control
        </text>
        <text x={CENTER.x} y={CENTER.y + 11} textAnchor="middle" fill="#71859a" fontSize="8.5">
          plane
        </text>
        <text x={CENTER.x} y={CENTER.y + 59} textAnchor="middle" fill="#71859a" fontSize="9">
          relay WSS {relayEnabled ? "habilitado" : "desligado"}
        </text>
      </svg>

      <figcaption className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-2 text-[11px] text-muted">
        <span className="inline-flex items-center gap-1.5">
          <span aria-hidden="true" className="h-px w-6 bg-accent" />
          caminho direto reportado
        </span>
        <span className="inline-flex items-center gap-1.5">
          <span aria-hidden="true" className="w-6 border-t border-dashed border-warn" />
          via relay
        </span>
        <span className="inline-flex items-center gap-1.5">
          <span aria-hidden="true" className="w-6 border-t border-dotted border-faint" />
          sem relatório / offline
        </span>
        <span className="ml-auto text-faint">
          {visible.length} de {nodes.filter((node) => node.status !== "disabled").length} nós
          {hiddenCount > 0 ? ` · +${hiddenCount} fora da vista` : ""}
        </span>
      </figcaption>
      </div>

      <ul className="space-y-1.5 sm:hidden">
        {visible.map((node) => {
          const path = topologyPath(node);
          const online = isNodeOnline(node);
          const color = !online ? "bg-faint" : path === "direct" ? "bg-accent" : path === "relay" ? "bg-warn" : "bg-faint";
          const label = !online ? "offline" : path === "unknown" ? "sem relatório" : pathLabel[path];
          return (
            <li
              key={node.id}
              className="flex items-center justify-between gap-3 rounded-md border border-line bg-white/[0.02] px-2.5 py-2"
            >
              <span className="flex min-w-0 items-center gap-2">
                <span aria-hidden="true" className={`size-1.5 shrink-0 rounded-full ${color}`} />
                <span className="min-w-0">
                  <span className="block truncate text-xs font-medium text-ink">
                    {node.name}
                    {node.status === "pending" ? " · pendente" : ""}
                  </span>
                  <span className="block font-mono text-[10px] text-faint">{node.overlay_ip}</span>
                </span>
              </span>
              <span className="shrink-0 font-mono text-[10px] text-muted">{label}</span>
            </li>
          );
        })}
      </ul>

      <p className="mt-2 text-[10px] text-faint sm:hidden">
        {visible.length} de {nodes.filter((node) => node.status !== "disabled").length} nós
        {hiddenCount > 0 ? ` · +${hiddenCount} fora da vista` : ""}
      </p>

      <ul className="sr-only">
        {visible.map((node) => (
          <li key={node.id}>
            {`${node.name}, ${node.overlay_ip}, ${isNodeOnline(node) ? "online" : "offline"}, ${
              node.status === "pending" ? "pendente de aprovação" : nodeStatusLabel[node.status]
            }, caminho ${topologyPath(node) === "unknown" ? "sem relatório" : pathLabel[topologyPath(node) === "direct" ? "direct" : "relay"]}.`}
          </li>
        ))}
      </ul>
    </figure>
  );
}
