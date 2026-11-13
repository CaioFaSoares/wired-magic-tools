# Segurança

## Modelo de confiança

| Componente | Vê | Não vê | Compromisso |
|---|---|---|---|
| Control | metadados, chaves públicas, IPs, tokens (hash), ACL | tráfego, chaves privadas | pode revogar, redirecionar endpoints e negar serviço; não decifra |
| Relay | conexões por nó, tamanho/ritmo, ciphertext WG | conteúdo, portas internas | pode negar/atrasar; não decifra nem forja WG |
| Nó | próprio tráfego e o que a ACL permite | chaves privadas de outros | pode tentar spoof/abuso dentro das regras |
| Admin | tudo no painel (metadados) | chaves privadas | pode revogar, criar tokens, mudar ACL |

O relay e o control são **cegos ao conteúdo** por construção (WireGuard ponta a ponta). O
control é um ponto central de disponibilidade e de decisão de política: protegê-lo é
proteger a malha.

## Credenciais

- **Join token** (`wmj_...`): TTL, usos limitados, tags/labels administrativos. Uso único por
  padrão. Consumo e criação do nó/token são atômicos; falha de criação não gasta uso.
  Tags livres do cliente não concedem autoridade; labels do convite prevalecem sobre as
  do cliente. Vaza? Revogue em Tokens; uso já esgotado não recria o nó.
- **Token de nó** (`wmn_...`): opaco, hash SHA-256 no SQLite; vai em `Authorization: Bearer`.
  Revogar o nó apaga os tokens (`revoked_at`) e remove a conexão do relay.
- **Relay token**: Ed25519 assinado, audiência fixa, TTL 1 h. O relay rejeita outro host/exp.
- **Admin**: argon2id; sessão de 12 h em cookie HttpOnly/SameSite=Lax; CSRF obrigatório em
  mutações; rate limit de login por IP.
- **Service token** (`wms_...`): escopo `integration:nodes:read`; só leitura de nós.
- Chaves privadas WG ficam no nó (`agent-state.json`, 0600). O control nunca as recebe.

## Superfície de rede

- Control/relay: 443/tcp (Caddy ou rustls); STUN 3478/udp. Não exponha o SQLite nem o socket
  local.
- Nós: apenas saída. O agente escuta UDP 41641 e, se configurado, portas `--expose` no IP
  overlay. As portas expostas passam pela ACL e projetam só para `127.0.0.1`.
- API local: Unix socket 0660; não há TCP local. Rode a TUI com um usuário do grupo do socket
  (ou root).

## ACL

- Aplicada na origem **e** no destino, evitando spoof de IP de origem.
- `default_allow=true` quando a política não tem regras. Com qualquer regra, o default é deny.
- Tabela de fluxos permite respostas de conexões TCP/UDP estabelecidas por 5 min; é conntrack
  simplificado, não um firewall completo.
- A política é recompilada no enrollment, saída e mutações administrativas de nome/tag/status.
- Nome é uma identidade de segurança quando aparece em seletor `name:`. Enrollment com
  nome já referenciado pela política exige aprovação administrativa (fica pending mesmo
  sem aprovação global). Autorrenomeação para/de nome referenciado é recusada; use edição
  administrativa. Ao escrever uma política nova, confira a identidade/registro do nó
  existente, não só o nome que ele escolheu. Prefira tags atribuídas pelo administrador.
- Um mapa novo revalida fluxos existentes contra a ACL; tráfego contínuo não mantém uma
  autorização removida. A direção autorizadora original é preservada para respostas.
- **Default aberto dentro da malha é conveniência de POC.** Para produção, use o template
  restrito e revise `src`/`dst`/portas. O simulador do painel mostra a regra que casou.

## Exposição de loopback

`--expose 8421` abre `10.66.0.x:8421` e encaminha para `127.0.0.1:8421`. Qualquer peer que a
ACL permitir alcança o serviço. Exponha só o necessário, prefira portas específicas e mantenha
o serviço local sem autenticação apenas se a ACL for a fronteira. O Baguette do Seance deve
ser exposto assim se continuar em loopback.

## Privilégios

- O agente precisa de root para `/dev/net/tun` e rotas. O código tenta minimizar: não executa
  shell além de `ip`/`ifconfig`/`route`, não abre portas públicas e não lê arquivos de outros
  usuários.
- O control pode rodar sem root (porta > 1024 atrás de proxy). Se usar 443 direto, precisa de
  root/capability.
- O SQLite e a chave de assinatura ficam no data dir 0700. Backup é sensível.

## Auditoria e resposta

- Eventos de login, criação/revogação de token, aprovação/revogação/edição de nó e ACL vão
  para `audit_events` (ator, IP, alvo, detalhe).
- Revogação: nó sai do mapa, tokens são invalidados e a sessão de relay é cancelada.
  Geração de sessão impede que reader antigo/clonado continue enviando após revoke ou
  substituição; frames já enfileirados antes disso podem chegar ao destinatário.
  Peers descartam a chave no próximo mapa recebido/aplicado. Não há prazo global garantido
  durante partição: peers isolados do control podem manter estado antigo. Disponibilidade
  offline não equivale a revogação instantânea; não há leases offline implementados.
- Não há expiração automática de chave WG; revogar exige reenroll.
- Rotação da chave de assinatura do control ainda é manual (trocar a setting e reiniciar).

## Hardening recomendado antes de produção

1. TLS real (Caddy/Let's Encrypt) e HSTS no proxy; `Secure` cookie já é automático em https.
2. ACL restrita por tag; remover o default aberto.
3. Senha de admin forte; considerar SSO/2FA (não incluso).
4. Rate limit no proxy e backup do SQLite.
5. Monitorar `last_seen`, falhas de login e tamanho do `audit_events`.
6. Revisar `--expose`; manter Baguette e outros serviços sensíveis atrás de ACL e, de
   preferência, de autenticação própria.
7. Rodar `cargo deny`/`pnpm audit` e revisar dependências antes de release.
8. Tratar manifestos de extensão como units de root: hooks `install`/`activate`/
   `deactivate` rodam sem sandbox com o privilégio do agente. Só instale manifestos
   revisados; a ativação por nó (TUI/token do nó) só escolhe entre manifestos já
   instalados pelo admin.
9. Revisar `NetworkMap.exposes`/adapter shares: um manifesto ativo publica portas
   loopback no overlay. Mantenha ACL restrita e não use adapters para serviços que
   deveriam continuar exclusivos do host.

## Lacunas conhecidas

- Relay token expira em 1 h, mas uma conexão já aberta não é derrubada exatamente na hora;
  a revogação de nó derruba via hub.
- Sem rate limit distribuído; o limitador é em memória de um control. Login/enrollment
  usam o IP do socket TCP, não `X-Forwarded-For`/`X-Real-IP` informados pelo cliente.
  Atrás de reverse proxy todos os clientes compartilham o bucket do proxy; aplique também
  limite por cliente no proxy. Não há allowlist de trusted proxies nesta versão.
  Chamadas embutidas sem ConnectInfo compartilham o bucket `unknown` (falha conservadora).
  Cada limiter mantém no máximo 20.000 buckets e chaves de até 128 bytes; saturação nega
  novas origens até expirar espaço, sem resetar buckets ativos. Mutex com falha nega.
  Isso limita memória, não substitui limites de conexão/CPU no proxy público.
- Sem mTLS entre agente e control; a autenticação é token sobre TLS.
- Sem auditoria de leitura de metadados nem de sessões de relay.
- A API de integração expõe IPs/nomes a quem tiver o service token; trate-o como credencial.
- `wm host` em dev sem TLS não deve ser exposto na internet.
- Hooks de extensão não têm sandbox, seccomp, cgroup ou limite de saída de rede; o
  timeout limita duração, não o que o script pode fazer. A ativação é decisão do
  membro do nó, mas o manifesto é decisão do admin — os dois controles são
  necessários.
- Exposes de adapter são TCP e ficam abertos no IP do overlay enquanto a extensão
  está ativa; a ACL do data plane é a única barreira de rede. Shares de adapter
  são globais ao control e não têm ACL por share nesta versão.
