# Debug GUI

Painel web de performance do host, do processo `wm` e das extensões locais. O
servidor é o subcomando `wm debug_gui`; o padrão é a TUI `wm debug`.

## O que a ativação monta

| Recurso | Efeito |
| --- | --- |
| sidecar | `wm debug_gui --listen 127.0.0.1:8791 --data-dir <data-dir>` |
| wrappers | `/usr/local/bin/wm-debug-gui` e `/usr/local/bin/debug-discover` |
| label | `debug_gui=true`, `debug_port=8791` |
| expose | `10.66.0.x:8791` → `127.0.0.1:8791` no overlay (ACL) |
| share | `debug-gui-<nó>` (web, porta 8791) |

```sh
sudo wm extensions enable debug_gui
wm open debug-gui-<nó>
debug-discover --json
```

O `--data-dir` é embutido no wrapper para a descoberta varrer
`<data_dir>/extensions`. `debug-discover` é o atalho de `wm debug discover`;
ele lista extensões locais, marca novas/atualizadas e grava o estado em
`.debug-discovery.json`.

## O que mostra

CPU global e por núcleo, memória/swap, load, uptime, discos, interfaces de
rede, processo `wm` (PID/PPID/CPU/RSS/virtual/uptime/cmd), top processos e a
lista de extensões locais com status `nova`/`atualizada`/`ok`/`inválida`.

## Segurança

- **Sem autenticação.** Hostname, discos, interfaces e processos são sensíveis;
  a ACL da malha é o controle de acesso. Não publique na internet.
- **Somente leitura.** Nenhum endpoint de escrita.
- **Coleta local.** `sysinfo` lê o próprio nó; nada é enviado ao control plane.
- **Mesh-only.** Escuta loopback e só é exposto no overlay.

## Endpoints

| Método | Rota | Efeito |
| --- | --- | --- |
| GET | `/` | painel web |
| GET | `/healthz` | `{ "ok": true }` |
| GET | `/api/info` | título, nó, versão e intervalo |
| GET | `/api/snapshot` | host, CPU, memória, discos, rede, wm, processos e extensões |
| GET | `/api/extensions` | última descoberta local |

A TUI `wm debug` usa o mesmo sampler local, mas não precisa deste servidor.
