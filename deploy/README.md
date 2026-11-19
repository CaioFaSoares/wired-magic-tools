# Deploy do wired-magic

> Para hospedar com **Coolify** em um único compose, use [`Dockerfile`](../Dockerfile) e
> [`docker-compose.yml`](../docker-compose.yml) na raiz e siga
> [DEPLOY-COOLIFY](../docs/DEPLOY-COOLIFY.md). As receitas abaixo são para instalação nativa
> (systemd/launchd/Caddy) e para quem precisa de rede host ou SNAT.

## Topologia recomendada

```text
Internet
  │ 443/tcp  (Caddy → wm host em 127.0.0.1:8080)
  │ 3478/udp (STUN do wm host)
  ▼
VPS ── wm host ── 10.66.0.1 ── TUN ── overlay
  │
  ├── Mac do rack ── wm join (utun/utunN, 10.66.0.x)
  └── container do Seance na mesma VPS
```

## 1. VPS

Instale o binário em `/usr/local/bin/wm` e:

```sh
sudo mkdir -p /var/lib/wired-magic/control
sudo wm host \
  --public-url https://mesh.exemplo.com \
  --listen 127.0.0.1:8080 \
  --data-dir /var/lib/wired-magic
```

- Antes do primeiro start, configure `WM_BOOTSTRAP_USERNAME` e `WM_BOOTSTRAP_PASSWORD`
  (ou `WM_BOOTSTRAP_PASSWORD_FILE`). Não há senha gerada nos logs. Pela TUI remota,
  entre com essa credencial temporária e crie admin definitivo + usuário cotidiano:
  `wm admin --server https://mesh.exemplo.com tui`. O bootstrap é invalidado;
  só então o host inscreve o gateway. Veja [primeiro acesso](../docs/remote-admin.md).
- Coloque o Caddy na frente (Caddyfile incluso) para TLS/ACME e WebSocket do relay.
- Libere **443/tcp** e **3478/udp** no firewall. `41641/udp` é saída/inbound opcional dos nós.
- Se quiser TLS direto, use `--tls-cert`/`--tls-key` e escute em `0.0.0.0:443`.

Systemd (host):

```sh
sudo cp deploy/systemd/wired-magic-host.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now wired-magic-host
```

Ajuste a URL/porta no unit antes. Para bootstrap no systemd, crie
`/etc/wired-magic/bootstrap.env` com propriedade root e modo 0600, contendo as
variáveis temporárias acima. O unit carrega esse arquivo se existir. Remova os
segredos após concluir o setup; preserve o diretório de dados nos reinícios.

## 2. Macs do rack

```sh
sudo wm join https://mesh.exemplo.com --token wmj_... --name mac-01 --tag mac
sudo wm service install --mode agent
```

Requisitos: root (utun + rotas) e saída UDP/TCP liberada. O Mac precisa de `sshd`/serviço local só se o Seance for falar com ele via TCP; o overlay em si não abre portas.

Verificação:

```sh
sudo wm status
sudo wm peers
wm ping site
wm probe mac-02 8421
```

## 3. Seance na mesma VPS

O host da VPS é o nó `10.66.0.1`. O container do Seance precisa alcançar `10.66.0.0/24`. Duas receitas:

**A. host network (mais simples)**

```yaml
services:
  app:
    network_mode: host
```

Ajuste o PocketBase para `127.0.0.1` e as portas publicadas.

**B. bridge + SNAT (mantém o compose)**

No host, depois do `wm host` (interface do TUN descoberta em `wm status`; exemplo `tun0`):

```sh
sudo sysctl -w net.ipv4.ip_forward=1
sudo iptables -t nat -A POSTROUTING -s 172.16.0.0/12 -d 10.66.0.0/24 -o tun0 -j MASQUERADE
```

O container usa a rota default para o gateway Docker; o host encaminha para o TUN e o SNAT faz o nó aparecer como `.1` para a ACL. Se o seu bridge usa outra faixa, ajuste `-s`.

O container então chama `http://10.66.0.12:8421` (Baguette) ou o IP do Mac que o Seance usar.

## 4. Convites e painel

- Administração padrão: TUI/CLI. O [painel React](../docs/control-panel.md) é opcional,
  instalado e ativado por admin depois do setup, sem rebuild do servidor.
- Convite: crie em **Tokens** ou use o token impresso pelo `wm host` (10 usos por padrão).
- ACL: comece com o template aberto e endureça em **ACL**. O simulador mostra a regra que casou.
- Integração: crie um service token em **Integração** e configure o Seance com `Authorization: Bearer wms_...`.

## 5. Backup e upgrade

- Backup: `/var/lib/wired-magic/control/control.db` + a chave de assinatura ficam no SQLite; copie o diretório inteiro com o serviço parado.
- Upgrade: substitua `/usr/local/bin/wm`, reinicie o serviço. Chaves e tokens são preservados.
- Logs: `journalctl -u wired-magic-host -f` (Linux) ou `/var/log/wired-magic.log` (launchd).

## 6. E2E local

```sh
scripts/e2e-docker.sh
```

Sobe control+relay+STUN e três nós Linux em containers privilegiados, testa caminho direto, relay, ping/probe e a API de integração.
