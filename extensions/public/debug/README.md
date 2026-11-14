# Debug público

Extensão pública que serve um painel de performance do host e do processo `wm`
do nó. O servidor é o subcomando `wm debug` — nenhum binário extra é baixado.

## O que a ativação monta

| Recurso | Efeito |
| --- | --- |
| sidecar | `wm debug --listen 127.0.0.1:8791` (supervisionado pelo agente) |
| wrapper | `/usr/local/bin/wm-debug`, regravado a cada ativação com o `wm` do nó |
| label | `debug=true`, `debug_port=8791` |
| expose | `10.66.0.x:8791` → `127.0.0.1:8791` no overlay (sujeito à ACL) |
| share | `debug-<nó>` (web, porta 8791) |

```sh
sudo wm extensions enable debug
wm open debug-<nó>         # de outro nó da malha
```

O painel fica em `http://<overlay_ip>:8791`. Ele **não** abre porta pública, não
usa cloudflared e não exige que outras portas do nó estejam abertas: o acesso é
feito pelo mesmo túnel da malha, que já transporta os shares e o control.

## O que ele mostra

- **Host:** hostname, SO, kernel, arquitetura e uptime.
- **CPU:** uso global, marca, núcleos físicos/lógicos, frequência e barras por
  núcleo.
- **Memória:** total, usada, disponível, barra de uso e swap.
- **Load average:** 1, 5 e 15 minutos.
- **Discos:** mount, filesystem, uso e espaço livre (atualizado a cada 5 s).
- **Rede:** RX/TX por segundo e totais por interface.
- **Processo `wm`:** PID, PPID, estado, CPU, RSS, memória virtual, uptime e linha
  de comando. O coletor usa o processo pai do sidecar (o agente `wm`) e, quando
  roda manualmente, procura o `wm up`/`join`/`host` de vida mais longa.
- **Top processos:** 8 maiores por CPU (nome, CPU, RSS e estado; sem linha de
  comando, para não vazar segredos de argumentos).

O hook de ativação prefere o binário do próprio agente (`WM_AGENT_BIN`) e,
depois, caminhos usuais como `/usr/local/bin/wm` e `/opt/homebrew/bin/wm`.

## Segurança

- **Sem autenticação.** Qualquer nó que alcance a porta vê os dados. A ACL da
  malha e o share são o controle de acesso. Não publique esta porta na internet.
- **Dados sensíveis.** Hostname, layout de discos, nomes de interfaces e
  processos podem revelar informação da máquina. Ative só em nós confiáveis.
- **Somente leitura.** O servidor não aceita mutações; não há endpoints de
  escrita.
- **Coleta local.** `sysinfo` lê o sistema no próprio nó; nada é enviado ao
  control plane. O histórico é apenas a última amostra em memória.

## Endpoints

| Método | Rota | Efeito |
| --- | --- | --- |
| GET | `/` | painel estático (HTML/CSS/JS embutidos no `wm`) |
| GET | `/healthz` | `{ "ok": true }` |
| GET | `/api/info` | título, nó, versão e intervalo de amostragem |
| GET | `/api/snapshot` | última amostra completa em JSON |

O JSON de `/api/snapshot` é estável o suficiente para scripts:

```json
{
  "generated_at": "2026-11-14T00:00:00Z",
  "host": {"hostname": "...", "uptime_secs": 123},
  "cpu": {"usage_percent": 12.5, "per_core": [10.0, 15.0]},
  "memory": {"total_bytes": 0, "used_bytes": 0},
  "load": {"one": 0.1, "five": 0.1, "fifteen": 0.1},
  "disks": [],
  "networks": [],
  "wm": {"pid": 123, "cpu_percent": 0.4, "memory_bytes": 0},
  "processes": []
}
```

## Personalizar

```json
{
  "command": "/usr/local/bin/wm-debug",
  "args": ["--listen", "127.0.0.1:8791"],
  "restart": "always"
}
```

Opções do servidor: `--listen`, `--title` e `--node`. A porta publicada pelo
adapter é fixa em 8791; se mudar `--listen`, ajuste `[adapter]` no manifesto e
reative a extensão em cada nó.
