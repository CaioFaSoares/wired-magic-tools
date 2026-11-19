# Deploy na VPS

> Para o caminho com **Docker Compose + Coolify**, use
> [DEPLOY-COOLIFY](DEPLOY-COOLIFY.md). Este documento cobre a instalação nativa com systemd e
> Caddy.

Ver também [`deploy/README.md`](../deploy/README.md) (recipes de Caddy, systemd, launchd,
Docker/Seance e SNAT). Este documento é o passo a passo operacional.

## Pré-requisitos

- VPS Linux com IP público, domínio apontando para ela, portas **443/tcp** e **3478/udp**
  liberadas.
- Root (TUN, rotas, systemd) e o binário `wm` em `/usr/local/bin`.
- Caddy (ou outro proxy TLS) para ACME/HTTPS e WebSocket.

## 1. Binário

```sh
# build local (Linux):
CARGO_BUILD_JOBS=1 cargo build --locked --release -p wired-magic
sudo install -m755 target/release/wm /usr/local/bin/wm
```

## 2. Host

Antes do primeiro start, grave uma senha temporária forte (12–1024 bytes) em
`/etc/wired-magic/bootstrap.secret`, arquivo root-owned modo 0600. Passe a env ao
processo root explicitamente (`sudo` normalmente filtra variáveis do seu shell).
O servidor não precisa de Node nem de build React.

```sh
sudo mkdir -p /var/lib/wired-magic
sudo env WM_BOOTSTRAP_USERNAME=setup \
  WM_BOOTSTRAP_PASSWORD_FILE=/etc/wired-magic/bootstrap.secret \
  wm host \
  --public-url https://mesh.exemplo.com \
  --listen 127.0.0.1:8080 \
  --data-dir /var/lib/wired-magic
```

- Após configurar o proxy abaixo, abra `wm admin --server https://mesh.exemplo.com tui`
  no seu cliente. Entre como `setup` com a senha temporária e crie admin + usuário
  cotidiano. [Fluxo e limites](remote-admin.md).
- Não há senha impressa. Setup consome o bootstrap permanentemente; remova sua env
  e o arquivo secreto após concluir. Não use bootstrap para tentar redefinir senhas.
- Só depois disso o host se inscreve como `10.66.0.1` (gateway) e imprime o convite.
- O host também conecta ao relay e ao STUN. Nada precisa escutar em 443 no processo; o Caddy
  faz TLS e repassa HTTP/WS para 127.0.0.1:8080.

Caddy (mínimo):

```caddy
mesh.exemplo.com {
  encode zstd gzip
  reverse_proxy 127.0.0.1:8080 {
    transport http {
      read_timeout 90s
      write_timeout 90s
    }
  }
}
```

Teste:

```sh
curl https://mesh.exemplo.com/healthz
curl https://mesh.exemplo.com/api/v1/meta
```

## 3. Systemd

Para iniciar direto pelo serviço, configure `/etc/wired-magic/bootstrap.env`
(root-owned, modo 0600) com `WM_BOOTSTRAP_USERNAME` e
`WM_BOOTSTRAP_PASSWORD_FILE=/etc/wired-magic/bootstrap.secret`. A unit lê esse
arquivo opcional. Após o setup, remova as variáveis bootstrap; preserve o volume.
Não execute o start manual e o serviço simultaneamente nas mesmas portas/dados.

```sh
sudo cp deploy/systemd/wired-magic-host.service /etc/systemd/system/
# ajuste ExecStart (URL/porta/data dir)
sudo systemctl daemon-reload
sudo systemctl enable --now wired-magic-host
journalctl -u wired-magic-host -f
```

## 4. Nós

```sh
sudo wm join https://mesh.exemplo.com --token wmj_... --name mac-01 --tag mac --expose 8421
sudo wm service install --mode agent
```

- `--expose 8421` é necessário se o serviço real escuta em `127.0.0.1:8421` (Baguette).
- Sem `--expose`, só serviços ligados ao IP overlay ou à LAN são alcançáveis.
- Verifique: `wm status`, `wm peers`, `wm probe mac-01 8421`, `wm ping site`.

## 5. Container do Seance

O host da VPS é o nó `.1`. Para o container alcançar `10.66.0.0/24`:

**A. network_mode: host** no serviço `app` (e ajuste o PocketBase para loopback).
**B. bridge + SNAT** (ver `deploy/README.md`): `ip_forward=1` + MASQUERADE para o TUN.

O container chama `http://10.66.0.12:8421` (com `--expose 8421` no Mac) ou o IP/serviço
que precisar. O login do aluno continua no Seance; o wired-magic não tem portal de aluno.

## 6. Convites, ACL e integração

- Administração padrão: `wm admin --server https://mesh.exemplo.com tui` e CLI admin.
- Painel web: `https://mesh.exemplo.com` somente após instalar/ativar a
  [extensão opcional](control-panel.md). Não é requisito de deployment.
- Tokens: crie por nó/grupo, com TTL curto; evite um token eterno de 100 usos.
- ACL: comece restrito. Exemplo:

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

- Integração: crie um service token e use `GET /api/v1/integration/nodes` no Seance.
  Guia: [`INTEGRATION-SEANCE.md`](INTEGRATION-SEANCE.md).

## 7. Operação

- Backup: pare o serviço e copie `/var/lib/wired-magic/control` (SQLite + chaves). Guarde com
  a mesma sensibilidade de uma chave privada.
- Upgrade: substitua o binário e reinicie; estado e tokens permanecem.
- Métricas/logs: `journalctl`, `/api/v1/admin/events` (SSE), `/healthz`, `wm doctor`.
- Ameaças e hardening: [`SECURITY.md`](SECURITY.md).
- Cenário Tauá: [`RUNBOOK-TAUA.md`](RUNBOOK-TAUA.md).
