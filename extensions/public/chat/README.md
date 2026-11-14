# Chat público

Extensão pública que sobe uma sala de chat web em cada nó ativado. O servidor é
o subcomando `wm chat` — a extensão não baixa um segundo binário.

## O que a ativação monta

| Recurso | Efeito |
| --- | --- |
| sidecar | `wm chat --listen 127.0.0.1:8790` (supervisionado pelo agente) |
| wrapper | `/usr/local/bin/wm-chat`, regravado a cada ativação com o `wm` do nó |
| label | `chat=true`, `chat_port=8790` |
| expose | `10.66.0.x:8790` → `127.0.0.1:8790` no overlay (sujeito à ACL) |
| share | `chat-<nó>` (web, porta 8790) |

Ative o chat **no nó que vai hospedar a sala** (normalmente o servidor). Os
outros nós não precisam ativar a extensão: eles apenas abrem o share.

```sh
# no nó servidor
sudo wm extensions enable chat

# em qualquer outro nó da malha
wm open chat-<nó-servidor>
```

O chat é **mesh-only**: o servidor escuta só em loopback e o adapter expõe a
porta apenas no overlay. Um nó com portas públicas fechadas continua acessando
o chat do nó servidor pelo túnel da malha, porque o acesso não depende de abrir
porta no host nem no roteador. Não publique esta porta no cloudflared; se um dia
isso for necessário, trate como uma decisão separada de exposição pública.

O hook de ativação prefere o binário do próprio agente (`WM_AGENT_BIN`) e,
depois, caminhos usuais como `/usr/local/bin/wm` e `/opt/homebrew/bin/wm`.

## Segurança

- **Sem autenticação por padrão.** Qualquer nó que alcance a porta do servidor
  lê e escreve. A ACL da malha e o share são o controle de acesso.
- **Sem persistência.** Até 500 mensagens em memória; reiniciar o sidecar limpa a
  sala. Não use como registro/auditoria.
- **Conteúdo tratado como texto.** O servidor recusa caracteres de controle e o
  cliente renderiza com `textContent`; não há HTML de usuário.
- **Limites.** Nome até 32 caracteres, mensagem até 2.000, corpo HTTP até 64 KiB.
- **Escuta loopback.** O manifesto nunca abre `0.0.0.0`; a única exposição é a
  porta do adapter no overlay, sujeita à ACL da malha.

## Endpoints

| Método | Rota | Efeito |
| --- | --- | --- |
| GET | `/` | UI estática (HTML/CSS/JS embutidos no `wm`) |
| GET | `/healthz` | `{ "ok": true }` |
| GET | `/api/info` | título, nó, epoch, limites |
| GET | `/api/messages?after=N` | mensagens com `id > N` e cursor |
| POST | `/api/messages` | `{ "author": "Ana", "text": "oi" }` → 201 |

O campo `epoch` identifica a instância do sidecar. Se o processo reiniciar, o
cliente detecta o novo epoch e recarrega a sala em vez de esperar ids antigos.

## Personalizar

O `default_config` aceita os mesmos campos do driver `exec`:

```json
{
  "command": "/usr/local/bin/wm-chat",
  "args": ["--listen", "127.0.0.1:8790"],
  "restart": "always"
}
```

Opções do servidor: `--listen`, `--title` e `--node`. Para outro título, edite o
`config` do nó no painel (o wrapper continua sendo o mesmo). A porta publicada
pelo adapter é fixa em 8790; se mudar `--listen`, ajuste `[adapter]` no
manifesto e reative a extensão em cada nó.
