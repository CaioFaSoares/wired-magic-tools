# Chat GUI

Interface web do chat. O servidor é o subcomando `wm chat_gui`; o cliente
padrão é a TUI `wm chat` (pacote `chat`).

## O que a ativação monta

| Recurso | Efeito |
| --- | --- |
| sidecar | `wm chat_gui --listen 127.0.0.1:8790` |
| wrapper | `/usr/local/bin/wm-chat-gui`, regravado a cada ativação |
| label | `chat_gui=true`, `chat_port=8790` |
| expose | `10.66.0.x:8790` → `127.0.0.1:8790` no overlay (ACL) |
| share | `chat-gui-<nó>` (web, porta 8790) |

Ative no nó que hospeda a sala; os outros nós só abrem o share:

```sh
sudo wm extensions enable chat_gui          # no servidor
wm open chat-gui-<nó-servidor>              # navegador
wm chat --url http://10.66.0.x:8790         # TUI em outro nó
```

O chat é **mesh-only**: escuta loopback e só é exposto no overlay. Um nó com
outras portas públicas fechadas continua acessando, porque o tráfego vai pelo
túnel da malha. Não publique a porta no cloudflared.

## Segurança

- **Sem autenticação por padrão.** Qualquer nó que alcance a porta lê e escreve;
  a ACL da malha e o share são o controle de acesso.
- **Sem persistência.** Até 500 mensagens em memória; reiniciar o sidecar limpa.
- **Conteúdo como texto.** Sem HTML de usuário; limites de 32/2.000 caracteres e
  corpo HTTP de 64 KiB.
- **Escuta loopback.** A única exposição é o adapter no overlay.

## Endpoints

| Método | Rota | Efeito |
| --- | --- | --- |
| GET | `/` | UI web |
| GET | `/healthz` | `{ "ok": true }` |
| GET | `/api/info` | título, nó, epoch e limites |
| GET | `/api/messages?after=N` | mensagens com `id > N` |
| POST | `/api/messages` | `{ "author": "Ana", "text": "oi" }` |

A TUI `wm chat` consome exatamente esses endpoints.
