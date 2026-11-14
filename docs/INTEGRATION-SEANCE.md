# Integração com o Seance

O Seance continua dono do login, dos projetos, do agente WSS e do Baguette. O wired-magic
fornece **rede** e **metadados de nós**. Nenhum código Go do Seance foi alterado neste
trabalho; este documento define o contrato para a integração futura.

## 1. O que o Seance precisa saber

Somente: `name`, `overlay_ip`, `labels` e se o nó está online. O painel do Seance já casa o
agente WSS com o nó da malha por **nome** e calcula `baguetteUrl`; hoje ele lê isso do
`internal/mesh` local. A troca futura é ler da nossa API.

```http
GET /api/v1/integration/nodes
Authorization: Bearer wms_...
```

```json
{
  "nodes": [
    {
      "id": "0d4f...",
      "name": "mac-01",
      "hostname": "mac-rack-01.local",
      "overlay_ip": "10.66.0.12",
      "tags": ["mac"],
      "labels": {"baguette_port": "8421"},
      "online": true,
      "last_seen": "2026-11-12T22:30:00Z"
    }
  ],
  "server_time": "2026-11-12T22:30:01Z"
}
```

- `labels.baguette_port`: a porta anunciada pelo agente (`--expose 8421` e label
  `baguette_port=8421`). O Seance pode definir isso no join.
- `name` é o mesmo nome que o `seance-agent` envia no `hello` — o merge por nome continua.
- `online` combina `last_seen` (90 s) e presença no relay.

## 2. Criar o service token

Painel → **Integração** → criar token com escopo `integration:nodes:read`. Guarde no env do
Seance (ex.: `WM_INTEGRATION_TOKEN`). O token é mostrado uma vez; revogue se vazar.

## 3. Exemplo Go (cliente)

```go
func meshNodes(ctx context.Context, base, token string) ([]MeshNode, error) {
    req, _ := http.NewRequestWithContext(ctx, http.MethodGet,
        base+"/api/v1/integration/nodes?online=true", nil)
    req.Header.Set("Authorization", "Bearer "+token)
    resp, err := http.DefaultClient.Do(req)
    if err != nil {
        return nil, err
    }
    defer resp.Body.Close()
    if resp.StatusCode != http.StatusOK {
        return nil, fmt.Errorf("mesh: %s", resp.Status)
    }
    var out struct {
        Nodes []MeshNode `json:"nodes"`
    }
    if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
        return nil, err
    }
    return out.Nodes, nil
}
```

Use cache curto (5–15 s) e trate falha do control como "malha indisponível", não como queda
do agente. O fluxo de build continua no WSS e não depende desta API.

## 4. Expor o Baguette (loopback)

O Baguette escuta em `127.0.0.1:8421`; WireGuard não faz um serviço loopback aparecer no
overlay. A extensão **Seance** automatiza montagem, share, labels e expose:

```sh
# no Mac, depois de instalar o binário do wm:
sudo wm extensions enable seance
```

O manifesto (`extensions/private/seance/extension.toml`) roda o hook de instalação, inicia
`seance-agent`, publica `baguette-<nó>` e cria um listener `10.66.0.12:8421` →
`127.0.0.1:8421`, sujeito à ACL. A ativação também pode ser feita na TUI (`aba Extensões`,
`Enter`) ou no painel.

Fallback manual, sem a extensão:

```sh
sudo wm join https://mesh.exemplo.com --token wmj_... --name mac-01 \
  --tag mac --label baguette_port=8421 --expose 8421
```

Isso cria o mesmo listener, mas não instala nem supervisiona o agente do Seance. Nos dois
casos o painel/Seance usam `http://10.66.0.12:8421` como endpoint de **operador** (nunca
exponha ao navegador do aluno; o Seance deve proxear server-side).

Sem expose, `network_mode: host` no Seance não resolve o loopback no Mac; o correto é expor a
porta no nó.

## 5. Container do Seance na VPS

O host da VPS é o nó `.1`; o container precisa de rota para `10.66.0.0/24`:

- **host network**: `network_mode: host` no `app` (e PocketBase em loopback).
- **bridge + SNAT**: no host,
  `iptables -t nat -A POSTROUTING -s 172.16.0.0/12 -d 10.66.0.0/24 -o tun0 -j MASQUERADE`
  com `net.ipv4.ip_forward=1`. Ajuste a faixa do bridge e o nome do TUN.

Teste do container: `curl http://10.66.0.12:8421/simulators.json`.

## 6. Limites/decisões

- Não há login de aluno no wired-magic. O Seance é a identidade.
- Não há terminal web. O aluno continua no fluxo do Seance; a malha serve ao servidor.
- A API é read-only. Provisionar/revogar continua no painel do wired-magic (ou via API admin
  com sessão, fora do escopo do Go).
- O `internal/mesh` atual do Seance pode ser removido em uma decisão futura (D022/D023) depois
  que o painel Go consumir esta API; até lá, os dois modelos podem coexistir, mas evite dois
  alocadores de IP para os mesmos Macs.
- Se o Seance continuar emitindo configs WireGuard, não as aplique junto com o agente: duas
  interfaces/rotas para o mesmo `10.66.0.0/24` conflitam.

## 7. Checklist de integração

- [ ] Service token criado e guardado fora do front do aluno.
- [ ] `GET /integration/nodes?online=true` responde e casa nomes com o hub WSS.
- [ ] `wm extensions enable seance` nos Macs (ou fallback `--label` + `--expose`);
      `labels.baguette_port` e o share `baguette-<nó>` aparecem no mapa.
- [ ] URL do Baguette usada só server-side.
- [ ] Container alcança `10.66.0.x` (teste com curl).
- [ ] Falha do control tratada como degradação, não como queda do build.
- [ ] ACL restrita: `site → mac:8421,22,icmp`; simule no painel.
