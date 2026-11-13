# Runbook — cenário Tauá

**Objetivo:** alunos usam o site do Seance (na VPS) para acionar builds nos Macs do rack,
que estão atrás do NAT/firewall do laboratório. O wired-magic fornece a rede entre os nós; o
Seance continua dono do login e do fluxo pedagógico.

```text
aluno ──HTTPS──▶ Seance (VPS, nó 10.66.0.1)
                     │ overlay + expose 8421
                     ▼
                Mac do rack (10.66.0.12, Baguette em 127.0.0.1:8421)
```

## 0. O que você precisa

- VPS com domínio, 443/tcp e 3478/udp liberados.
- Binário `wm` (Linux/macOS) e Caddy (recomendado).
- Macs com root para utun/rotas e o Baguette escutando em loopback (porta 8421).
- O Seance já rodando na VPS (ou o host do Seance sendo a própria VPS).

## 1. Subir o host (uma vez)

```sh
sudo wm host --public-url https://mesh.exemplo.com --listen 127.0.0.1:8080 \
  --data-dir /var/lib/wired-magic
```

- Anote a senha do admin impressa e o convite `wmj_...`.
- Confirme: `curl https://mesh.exemplo.com/healthz`.
- O host vira `10.66.0.1` (gateway). Caddy faz TLS/WS para 127.0.0.1:8080.
- Firewall: 443/tcp e 3478/udp. O laboratório **não** precisa de port forward.

## 2. Seance alcançar o overlay

O host da VPS é o nó `.1`. O container do Seance precisa de rota para `10.66.0.0/24`:

**A. Host network** (mais simples) no serviço `app`.
**B. Bridge + SNAT**:

```sh
sudo sysctl -w net.ipv4.ip_forward=1
# descubra o TUN com: wm status --json | jq -r .tun_name
sudo iptables -t nat -A POSTROUTING -s 172.16.0.0/12 -d 10.66.0.0/24 -o tun0 -j MASQUERADE
```

Teste do container: `curl http://10.66.0.12:8421/simulators.json` (com o Mac já na malha e
Baguette exposto).

## 3. Entrar cada Mac

```sh
# instale o binário em /usr/local/bin/wm
sudo wm join https://mesh.exemplo.com --token wmj_... \
  --name mac-01 --tag mac --expose 8421
sudo wm service install --mode agent
```

- `--expose 8421` publica `10.66.0.12:8421` → `127.0.0.1:8421` (Baguette). Sem isso, o
  Baguette em loopback não é alcançável pela malha.
- O agente fica em foreground após o join; o service install o mantém reiniciando.
- Repita para `mac-02`, `mac-03`, … (cada um com nome único).

## 4. Verificar (antes de envolver aluno)

Na VPS ou num Mac:

```sh
sudo wm status
sudo wm peers
wm probe mac-01 8421        # TCP através do overlay
wm ping mac-01              # ICMP
curl -v http://10.66.0.12:8421/   # do host da VPS/Seance
```

No painel `https://mesh.exemplo.com`:

- Overview: nós online, caminhos direct/relay.
- Nodes: `mac-01` ativo, IP .12, online.
- ACL: veja o template e o simulador.

## 5. Endurecer a ACL

No painel, **ACL** → template restrito. Exemplo:

```yaml
version: 1
rules:
  - action: accept
    src: ["tag:site"]
    dst: ["tag:mac"]
    protocol: tcp
    ports: ["8421", "22"]
  - action: accept
    src: ["tag:site"]
    dst: ["tag:mac"]
    protocol: icmp
```

- Só o nó `site` (VPS/Seance) fala com os Macs; Mac não fala com Mac.
- O `tag:site` precisa estar no nó da VPS (`--tag site` no `wm host` não tem flag; edite as
  tags do nó no painel ou use `--tag site` em um `wm join` dedicado).
- Teste: `wm probe mac-01 8421` do site funciona; de outro Mac dá timeout.
- O conntrack do agente libera as respostas TCP/UDP e o eco ICMP da conexão estabelecida.

## 6. Integrar com o Seance

1. Painel → Integração → criar service token.
2. No Seance, use `GET /api/v1/integration/nodes` com `Authorization: Bearer wms_...`.
3. Case `name` com o nome do nó e leia `labels.baguette_port`.
4. O endpoint do Baguette para operador passa a ser `http://10.66.0.x:8421`.

Detalhes: [`INTEGRATION-SEANCE.md`](INTEGRATION-SEANCE.md).

## 7. Falhas comuns

| Sintoma | Causa provável | Ação |
|---|---|---|
| `wm join` 401 | token expirado/usado | crie outro token no painel |
| agente não sobe | sem root / sem `/dev/net/tun` | `sudo`, instale o serviço, `wm doctor` |
| peers offline | control inacessível/URL errada | `wm status`, `curl .../healthz`, `wm doctor` |
| probe timeout | ACL, serviço local parado ou `--expose` ausente | simulador de ACL, `wm probe` local, reinicie com `--expose` |
| Baguette inacessível | escuta só 127.0.0.1 | `--expose 8421` e serviço local rodando |
| caminho relay sempre | UDP bloqueado/NAT simétrico | esperado; o relay mantém a função |
| control caiu e túnel segue | comportamento esperado | reconecta sozinho; não reinicie os peers |
| TLS/cookie | proxy sem TLS ou hostname divergente | use HTTPS e o mesmo domínio |

## 8. Aceite do cenário

- [ ] `wm host` na VPS, admin e convite funcionando.
- [ ] 2 Macs (ou containers) com `wm join --expose 8421` e IPs distintos.
- [ ] `wm peers` mostra pelo menos um `direct` e um `relay` (force-relay para testar).
- [ ] `wm probe mac-01 8421` do site conecta; do Mac vizinho é negado pela ACL restrita.
- [ ] `curl http://10.66.0.12:8421/` do host do Seance retorna o serviço.
- [ ] derrubar o control não interrompe um ping já estabelecido.
- [ ] `GET /api/v1/integration/nodes` retorna os Macs para o Seance.
