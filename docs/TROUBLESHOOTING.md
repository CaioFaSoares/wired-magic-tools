# Troubleshooting

## 1. Coleta rápida

```sh
wm doctor
wm status --json
wm peers --json
journalctl -u wired-magic-agent -n 200 --no-pager   # Linux
tail -n 200 /var/log/wired-magic.log                # macOS launchd
```

Inclua versão (`wm version`), `wm status --json`, SO e o início/fim do erro.

## 2. O agente não sobe

| Checagem | Comando | Correção |
|---|---|---|
| root | `id -u` | rode com sudo/serviço root |
| TUN | `ls -l /dev/net/tun` (Linux) | carregue `tun` ou use host com suporte |
| porta UDP | `lsof -iUDP:41641` | pare o outro processo ou use `--udp-port` |
| estado | `ls -l <data>/agent-state.json` | refaça `wm join` |
| socket | `ls -l <data>/agent.sock` | remova socket órfão com o agente parado |
| rota | `ip route get 10.66.0.1` / `route -n get 10.66.0.1` | reinicie o agente; veja logs de rota |
| control | `curl <url>/healthz` | confira DNS/TLS/firewall |

## 3. Peers não aparecem

- `wm status` mostra `map_version` e `control_connected`? Se não, o control/URL/token está
  errado.
- O nó está `pending`? Aprove no painel (se `approval_required`) ou verifique a configuração.
- Peers só aparecem se estão `active` e o mapa foi recebido. O long-poll pode levar até 25 s;
  o primeiro mapa é imediato.
- `wm peers` mostra endpoints vazios? O discovery ainda não publicou; aguarde 20 s. Em redes
  sem STUN, os IPs LAN ainda são publicados.
- Firewall do laboratório bloqueando UDP: o peer cai para `relay`, mas os dois precisam
  alcançar o relay (443/tcp) e o control.

## 4. Caminho sempre relay

- UDP 41641 bloqueado ou NAT simétrico; é o comportamento esperado.
- Verifique `relay_connected` e `relay_online` por peer.
- Force um teste determinístico: `wm join ... --force-relay` (ou `wm up --force-relay`).
- Se o relay está off: o Caddy repassa `/relay`? O token de relay expirou? Reinicie o agente
  para pegar um mapa novo.

## 5. Probe falha

- O serviço local está rodando? `curl 127.0.0.1:8421` no próprio nó.
- O serviço escuta **só** loopback? Adicione `--expose 8421` e reinicie o agente.
- ACL restrita? Use o simulador no painel (`site` → `mac-01`, tcp, 8421). Lembre que a ACL é
  aplicada na origem e no destino.
- O peer está online? `wm peers`.
- Teste o caminho: `wm ping mac-01` (se ICMP permitido). Se ping passa e probe não, o problema
  é o serviço/porta, não a malha.

## 6. Ping falha

- ICMP pode estar bloqueado pela ACL; adicione uma regra `protocol: icmp` ou use `probe`.
- A regra de retorno é coberta pela tabela de fluxos ICMP do agente; se a política mudou,
  aguarde o novo mapa.
- Firewall do SO pode bloquear ICMP no TUN; teste `probe` para isolar.

## 7. Painel não abre / login falha

- `curl https://<host>/healthz` e `/api/v1/meta`.
- Caddy passou o `/relay` com WebSocket? O upgrade precisa de HTTP/1.1; o Caddy padrão suporta.
- Cookie: precisa do mesmo domínio e HTTPS; `Secure` é setado automaticamente em https.
- 401 no login: senha errada ou admin recriado; veja o log do primeiro start.
- 403 em mutações: falta o header `X-WM-CSRF` (recarregue o login).

## 8. Control/SQLite

- `database is locked`: um backup/consulta externa está segurando o WAL; pare o serviço para
  copiar.
- Corrupção: restaure o backup **com o serviço parado**. A chave de assinatura está no SQLite;
  sem ela, tokens de relay em voo falham até novo mapa.
- Trocar de VPS: copie o data dir inteiro; mantenha o mesmo `public_url`/domínio. Se mudar o
  domínio, os tokens de relay mudam de audiência e os agentes pegam novos no mapa.

## 9. macOS

- Sem root, utun/rotas falham; use LaunchDaemon.
- Conflito com Tailscale: usamos `10.66.0.0/24` para não colidir com `100.64.0.0/10`.
- Interface utun com número variável: o agente descobre o nome real; `wm status` mostra.
- Sleep/wake: o agente re-STUNa e reconecta; peers podem levar alguns segundos.
- Firewall pedindo aprovação: permita o binário; o TUN é local.

## 10. Reverter/limpar

```sh
sudo wm down --data-dir <dir>
sudo wm service uninstall
sudo rm -rf <dir>          # apaga estado/chaves (irreversível)
```

No painel, revogue o nó antes de descartar a máquina, para remover a chave do mapa dos peers.

## 11. TUI, agente em segundo plano e sessão

- **`q` fechou a tela, mas o agente continua rodando.** É o comportamento esperado. Ao sair, a
  TUI imprime o pid, o comando de reabertura e como parar. Para encerrar a malha neste nó use
  `X` na aba **Agente**, `:down` no console ou `sudo wm down --data-dir <dir>`.
- **Log do agente iniciado pela TUI:** `<dir>/agent.log` (rotaciona em 8 MiB para
  `agent.log.1`). Se o agente veio do systemd/launchd, o log continua sendo
  `journalctl -u wired-magic-agent` ou `/var/log/wired-magic-agent.log`.
- **O agente não sobe pela TUI:** veja a aba **Agente** e o `agent.log`; as causas comuns são
  falta de root (a TUI mostra o aviso), outro agente usando o mesmo data-dir/socket
  (`agent.lock`) ou TUN indisponível. O rodapé também mostra o motivo quando o processo sai.
- **Auto-start desligado:** depois de parar pelo `X`, abrir a TUI não inicia mais o agente.
  Use `s` na aba Início/Agente para iniciar e religar, ou `a` para alternar sem iniciar.
  `wm tui --no-auto-start` desliga só naquela sessão.
- **Reabrir retoma a aba, a seleção e o histórico:** o estado fica em `<dir>/tui-state.json` e é
  salvo no máximo a cada ~10 s (1 s após eventos), além de ao sair; um `kill -9` pode perder até
  esse intervalo. O arquivo é 0600 e pode ser removido com a TUI fechada para começar do zero.
- **Dados antigos na tela:** se o agente está parado, a TUI mostra o último retrato salvo
  (peers/shares) com o aviso de "última sessão". Isso não é uma conexão ativa; `s` inicia.
- **Console não executa comandos de shell:** `:` aceita apenas subcomandos `wm`; aspas agrupam
  argumentos, mas `|`, `>`, `&&` e variáveis não são interpretados. Use o terminal para shell.
- **Jobs em segundo plano:** `:host`, `:serve` e `:gui` escrevem em `<dir>/host.log`,
  `serve.log` e `gui.log`. A aba **Serviços** lista os pids; `x` para o selecionado. Fechar a
  TUI não encerra esses jobs — pare-os pela TUI ou com `kill`.
