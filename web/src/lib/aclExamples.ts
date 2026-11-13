import { stringify } from "yaml";
import type { AclPolicy } from "./types";

export function policyToYaml(policy: AclPolicy): string {
  return stringify(policy, { lineWidth: 0, indent: 2 });
}

export interface AclExample {
  id: string;
  title: string;
  description: string;
  yaml: string;
}

export const aclExamples: AclExample[] = [
  {
    id: "aberta",
    title: "Aberta",
    description: "Libera todo o tráfego dentro do overlay. Útil só para bootstrap e diagnóstico.",
    yaml: `# Aberta: uma regra explícita de accept para todo o overlay.
# O default do compilador passa a ser deny, mas a regra casa primeiro.
version: 1
rules:
  - action: accept
    src: ["*"]
    dst: ["*"]
    protocol: "*"
    ports: ["*"]
    comment: "aberta para bootstrap"
`,
  },
  {
    id: "taua",
    title: "Tauá restrita",
    description:
      "Operadores alcançam Macs em 8421 (Baguette) e 22 (SSH); o resto do tráfego para Macs é negado.",
    yaml: `# Tauá restrita: ajuste as tags aos nós reais da malha.
# name:um-no e tag:mac continuam válidos como seletores.
version: 1
rules:
  - action: accept
    src: ["tag:operator"]
    dst: ["tag:mac"]
    protocol: tcp
    ports: ["8421"]
    comment: "Baguette do Seance"
  - action: accept
    src: ["tag:operator"]
    dst: ["tag:mac"]
    protocol: tcp
    ports: ["22"]
    comment: "SSH de diagnóstico"
  - action: deny
    src: ["*"]
    dst: ["tag:mac"]
    protocol: "*"
    ports: ["*"]
    comment: "fecha o resto para os Macs"
`,
  },
];
