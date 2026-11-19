# WM: conectividade pequena, extensões opcionais

**Status: proposta para validação; não representa a arquitetura inteira já implementada.** 16/11/2026.

Recorte implementado depois desta proposta: servidor headless por padrão,
[painel opcional do servidor](control-panel.md) e [setup inicial pela TUI](remote-admin.md)
com bootstrap por env, admin + usuário e invalidação permanente do bootstrap.
Isso não implementa social, ownership de nós, novos adapters ou otimizações/medições
idle descritas a seguir. [Arquitetura atual](ARCHITECTURE.md).

## 1. Decisão central

WM deve ser uma **malha privada programável de nós e serviços**, com administração e aplicações opcionais por cima. Não deve transformar cada nó em servidor de rede social, painel DevOps e executor de comandos por padrão.

Manter Rust, SQLite e o protocolo WireGuard atual. Evoluir o monólito em módulos com dependências explícitas; não introduzir microserviços, Kubernetes, outro banco, broker ou runtime de plugins no caminho crítico apenas para organizar o código.

Um comando simples pode compor vários papéis. Separação de responsabilidade não exige vários processos: `wm host` pode continuar sendo a conveniência control + relay + STUN + agente. O operador deve conseguir executar apenas os papéis necessários.

Referências: [revisão e evidências](REVIEW-2026-11.md), [arquitetura atual](ARCHITECTURE.md), [segurança atual](SECURITY.md).

## 2. Três planos, três limites

```text
                         CLIENTES
                 CLI / TUI / painel opcional
                              |
                API de domínio, sem lógica na UI
                              |
         +--------------------+-----------------------+
         |                    |                       |
     CONECTIVIDADE       ADMINISTRAÇÃO             APLICAÇÕES
   identidade de nó      inventário read-only      perfis humanos
   enrollment/revogação  jobs autorizados          avatares/amigos
   peers e políticas     capacidades locais        chat / DM
   mapas e serviços      Coolify / containers      extensões futuras
         |                    |                       |
         +------ contratos versionados e escopos -----+
                              |
                agente local / APIs dos serviços
                              |
                WireGuard direto; relay fallback
```

### Conectividade — sempre pequena

- Identidade de nó, enrollment, chaves, mapas, presença, ACL e descoberta de serviços.
- WireGuard, NAT traversal, TUN quando necessário, fallback por relay e listeners explícitos.
- Sem histórico de chat, varredura de processos, inventário Docker ou hooks arbitrários no caminho de pacotes.
- Control distribui metadados; relay transporta ciphertext. Quando coabitam no mesmo binário, métricas e limites continuam separados.

### Administração — opt-in por nó

- Descobrir recursos não autoriza administrá-los. Consultas de inventário e comandos mutantes têm escopos distintos.
- Primeiro inventário restrito, logs limitados e health; depois operações tipadas: reiniciar um recurso autorizado, obter status, criar uma exposição aprovada.
- Preferir API do Coolify a acesso ao socket Docker. Um token amplo do Coolify não vira magicamente token restrito: o adapter precisa impor allowlists e o token deve ter o menor privilégio disponível no provedor.
- Não montar socket Docker em todos os nós. Acesso a ele equivale, em muitas instalações, a controle do host.
- Terminal remoto e execução arbitrária são capacidades privilegiadas separadas, desativadas por padrão; não são requisito para observar containers.

### Aplicações — social e outros usos

- Identidade de pessoa/conta não é identidade de nó nem de administrador da malha.
- Uma conta pode usar vários dispositivos; participar de uma sala não concede acesso a bancos, debug ou administração.
- Perfis, avatares, amizades, salas e DMs têm armazenamento, retenção, quotas e autorização próprios. Podem ser um único serviço social opcional inicialmente, sem criar um serviço por entidade.
- Não promover o chat anônimo atual a sistema de contas/DMs. Ele pode permanecer demo interna com avisos ou ser substituído por uma extensão autenticada.
- Criptografia WireGuard protege o transporte entre nós; **não significa** que mensagens estejam cifradas ponta a ponta entre pessoas ou que o serviço social não as veja. E2EE de DMs seria uma decisão de produto/protocolo independente.

## 3. Dependências e organização do código

Direção desejada:

```text
contratos de conectividade <--- control / agente
contratos de extensões    <--- runtime / pacotes / adapters
APIs de domínio           <--- CLI / TUI / GUI
```

Reaproveitar as crates atuais antes de criar novas:

| Unidade atual | Organização proposta |
|---|---|
| `wm-proto` | separar módulos `identity`, `map`, `policy`, `service`, `events`; preservar reexports e formato wire |
| `wm-control` | módulos de domínio + repositórios SQLite + handlers HTTP finos; relay/STUN isolados internamente |
| `wm-core` | separar data plane, sincronização do control, serviço de listeners, runtime local de extensões e API local |
| `wm-ext` | contratos pequenos e validação de manifestos; mover catálogo/asset de extensões privadas para pacotes opcionais |
| `wm` | composição, parsing, bootstrap, CLI; chat/debug não devem ser dependências obrigatórias do servidor mínimo |
| `wm-tui` | estado/navegação/render separados de I/O; operações assíncronas; filas e PTYs limitados |
| frontend | consumir APIs, não duplicar regras de autorização ou lifecycle |

Não dividir arquivos apenas para atingir número de linhas ou agradar ao CodeHygiene. Extrair por responsabilidade, taxa de mudança e invariantes testáveis. Começar por mapa/versionamento, autorização, catálogo/runtime de extensões e configuração de deployment.

### Artefatos, sem multiplicar produtos

Propor builds headless de control e agente sem React, TUI, chat, debug e coleta sysinfo. Implementar features Cargo e/ou pequenos entrypoints que componham bibliotecas existentes; o CLI completo continua disponível para desktop.

A meta é **não compilar dependências opcionais**, não apenas não invocar suas funções em runtime. Um `--no-gui` que mantém todas as dependências incondicionais não resolve o build.

Imagens pré-compiladas, versionadas por digest, devem ser o caminho normal de instalação. Build local continua opção de desenvolvimento. CI produz Linux nas arquiteturas usadas, mede tamanho/tempo/RSS e publica checksums; não recompilar tudo em cada VPS pequena.

## 4. Contratos de serviços e exposição

Separar três conceitos hoje próximos:

1. **Serviço:** identidade estável, nó proprietário, nome, protocolo, porta e metadados. Anunciar não abre porta.
2. **Grant/política:** quem pode acessar ou administrar aquele serviço. Listar não autoriza conectar.
3. **Exposure:** listener/forward concreto com bind, target, origem e ciclo de vida explícitos.

Escopos mínimos:

- `mesh`: acessível apenas na malha e sob ACL;
- `local`: socket/loopback do operador;
- `public`: publicação explícita por gateway autorizado, com porta/protocolo/destino/quotas próprios.

Esses nomes são contratos propostos, não flags já disponíveis. Um `share` atual é catálogo, não fronteira de autorização. Não apresentar catálogo global como ACL por share.

Novos deployments públicos devem começar com default-deny e grants explícitos; a política vazia atual permite tudo. Migrar instalações existentes com aviso/preview e consentimento para não cortar acesso administrativo nem preservar abertura silenciosa em ambiente multiusuário.

Targets precisam reconhecer namespace de rede. `127.0.0.1` dentro de um sidecar não é o loopback do host ou do banco em outro container. Para alcançar uma rede privada Docker, o nó aprovado deve integrar aquela rede ou usar um conector local específico, com destinos/portas allowlisted. Não aceitar target arbitrário vindo de usuário social: isso criaria um proxy para a rede interna.

TCP e UDP são contratos separados. O forward local atual é TCP; preservar o suporte UDP do overlay não equivale a ter publicação pública de UDP pronta. Túneis de jogos que exigem UDP público precisam de listener UDP explícito, tabela de associações limitada, expiração, testes de MTU/NAT e quotas.

Limites obrigatórios: listeners por nó, conexões por serviço, bytes em filas, duração ociosa, taxa e tráfego por nó/locatário. O relay sob tráfego consome CPU e banda proporcionalmente: ele não poderá ser “invisível” em qualquer carga.

## 5. Casos de uso do pedido

| Caso | Composição recomendada | Fronteira importante |
|---|---|---|
| Servidor público de túneis/jogos | control + relay; gateway de publicação apenas se necessário | usuários não ganham automaticamente acesso ao overlay inteiro; distinguir TCP e UDP públicos |
| Computadores privados mediados por servidor | control central; agente de saída em cada máquina; direto preferido, relay fallback | sem abrir painel/API administrativa na máquina; revogação e políticas por nó/grupo |
| Compose que cria uma malha | papel host explícito, dados persistentes, URL pública deliberada | não iniciar agente/TUN/root se só precisa de control |
| Compose que entra em host existente | papel node explícito + enrollment restrito; segredo via arquivo | ingressar não deve criar host de fallback quando a conexão falhar |
| Banco/container sem porta publicada | agente/conector em rede privada aprovada, serviço allowlisted | acesso na malha não publica na internet; namespace e credenciais do banco permanecem relevantes |
| Grid de nós com portas seletivas | conectividade + registro de serviços + grants; adapter de jobs opcional | WM é substrato, não scheduler distribuído completo por padrão |
| Administração bare metal | adapter local de inventário e receitas autorizadas | root só para operações que exigem; social não acessa receitas |
| Coolify sem abrir sua UI | adapter que usa API do Coolify pelo conector aprovado | separar listar containers, ler métricas, logs e ações mutantes |
| Perfis, avatar, amigos, chat e DM | serviço social opcional + API autenticada | conta humana ≠ nó ≠ admin; uploads com limites e validação |

Uma pessoa conectada a servidores distintos deve conservar contextos distintos: `(server_id, user_id/node_id/service_id)`. Não unir recursos por nome ou ID de extensão sem namespace. Não introduzir federação entre hosts agora apenas porque o desktop trabalha com vários perfis.

## 6. Configuração de container

Manter um único loader tipado para CLI/env/config. O mesmo valor final deve ser validado e exposto de forma redigida em `doctor`, independentemente da origem.

Contrato **proposto** para Compose, sujeito a adaptação aos nomes atuais:

```yaml
# Exemplo de design; NÃO é configuração suportada hoje.
WM_MODE: node             # node | control | host
WM_CONTROL_URL: https://mesh.example
WM_JOIN_TOKEN_FILE: /run/secrets/wm_join_token
WM_NODE_NAME: banco-privado
WM_PROFILE_DIR: /var/lib/wm
# Serviços e targets em arquivo versionado, não shell/JSON secreto em argv.
```

- CLI explícita > env > arquivo > defaults, documentado e testado.
- Ler token de enrollment apenas no ingresso; estado/chaves de identidade persistentes e com permissões restritas.
- Recriar container não deve gerar novo nó a cada boot se o estado continua válido.
- Falha de matrícula é erro claro, nunca abertura de host público sem intenção.
- Logs e dumps não mostram tokens, argv bruto nem conteúdo dos arquivos secretos.
- TUN/NET_ADMIN e rede host são opt-in conforme o papel. Control puro não precisa deles.
- Limites de CPU/RAM e rotação de logs explícitos; healthchecks leves, separados de readiness quando possível.

## 7. Extensões: descoberta não é execução

Preservar o princípio [core → CLI → TUI → GUI opcional](extensions-core-first.md), sem exigir que todo componente esteja linkado no binário principal.

Contrato mínimo antes de um marketplace/ecossistema público:

- ID namespaced, versão de API, origem e conteúdo identificado por hash; dependências declaradas.
- Descobrir manifesto é read-only. Confiar/instalar é ação explícita; seleção de aba não deve converter conteúdo de um repositório não confiado em código executado.
- Capabilities declaradas: ler contexto, inventário, abrir listener mesh, publicar porta pública, ler segredo específico, executar receita. Validação deve ocorrer em cada operação do host.
- Não chamar manifesto de sandbox. Processo `exec` herda permissões do usuário enquanto não houver isolamento real. Social/código não confiado nunca deve executar como root do agente.
- Consentimento do admin do servidor **e** do operador do nó para efeitos locais privilegiados; consentimento vinculado ao conteúdo/capacidades, não só ao ID.
- Desired state versionado, atualização idempotente, status/revisão/erro, timeout e política de restart com backoff.
- Update por staging + validação + troca atômica + rollback; paths sempre relativos à raiz canônica do pacote/repositório.
- Ownership para labels, shares, listeners e processos: remover/editar extensão deve reconciliar também o que ela deixou de declarar. Não apagar recursos manuais de terceiros.
- Logs, filas, arquivos, conexões e jobs com limites por extensão; cancelamento e encerramento de processos descendentes.

Protocolos de jobs devem conter ID, ator autenticado, nó alvo, ação tipada, parâmetros validados, expiração, chave de idempotência e resposta limitada. Desconexão não deve permitir replay de comandos antigos nem execução sem prazo. Rollout pode começar com chamadas locais read-only; não é necessário construir uma plataforma de execução remota inteira para o primeiro adapter Coolify.

## 8. Eficiência e metas de aceitação

O objetivo “quase imperceptível” deve ser medido em **idle** e **carga definida**, separadamente para control, agente, relay e extensões. Nenhum número de RAM/CPU é prometido nesta proposta.

### Remover trabalho inútil antes de otimizar Rust

1. Parar polling remoto de catálogo por TUI a cada segundo: snapshot no agente, cache por revisão + invalidação; fallback com TTL/jitter e deduplicação de chamadas concorrentes.
2. Agregar leituras de extensões por nó/revisão, sem consultas de ativação repetidas por manifesto.
3. Coalescer presença/last-used em memória e persistir por intervalo, com flush/best effort documentado. Validação de token/revogação continua imediata e independente de timestamp.
4. Mapa inalterado não deve refazer todo o estado, republicar extensões e emitir handshakes sem motivo. Cache estrutural por revisão; tokens de relay separados/renovados antes do vencimento. Não reutilizar token de um nó para outro ao cachear.
5. Reconciliar adapters por mudança de desired state; varredura periódica lenta só como reparo.
6. Sessões de peers sob demanda e expiração por ociosidade; keepalive NAT configurável. Hoje o agente tenta manter relações com todos os peers mesmo sem tráfego útil: cache de UI sozinho não resolve esse custo de malha.
7. Separar topologia de telemetria: relatórios periódicos não precisam regravar a lista completa e estatísticas de cada peer quando nada relevante mudou. Limitar cardinalidade e frequência de métricas.
8. Debug/sysinfo apenas se solicitado, com frequência e escopo limitados; nada de `refresh_all()` no núcleo idle.
9. Filas limitadas em bytes e itens, orçamento de trabalho por frame e timeouts explícitos; backpressure no plano de dados não pode bloquear toda a malha por causa de um peer.

### Matriz de medição proposta

| Cenário | Medir | Critério de sucesso |
|---|---|---|
| control, 1/10/100 nós idle, sem UIs/extensões | CPU, RSS/PSS, wakeups, queries/s, writes/s, WAL/log bytes, conexões/tasks | registrar baseline; custo previsível por nó; sem busy loop |
| abrir TUI/painel e fechar | deltas de requests, CPU, tarefas e conexões | nenhuma consulta de catálogo a cada segundo fora da necessidade; retorno ao baseline |
| 24 h com reconexões e PTY ruidoso | RSS, profundidade/bytes de filas, FDs, tarefas | estabiliza após warmup; UI responsiva; sem crescimento contínuo |
| direto vs relay TCP/UDP | throughput, p50/p95 RTT/jitter, drops, CPU por tráfego | quotas/backpressure funcionam; peer lento não paralisa os demais |
| revogar nó/sessão durante long-poll/SSE/relay | credenciais e efeitos posteriores | sem mapa/token novo após revogação; stream expirada não recebe segredos |
| container privado e reinício | identidade, listeners, target, DNS | reencontra mesmo nó; porta só no escopo aprovado; falhas não abrem publicação |
| build frio/quente headless vs completo | tempo, pico RSS, cache hits, tamanho | instalação normal usa imagem pronta; dependências opcionais ausentes do headless |

Definir budgets numéricos após medir máquina e carga de referência. Para limitar risco em VPS, começar por imagens prontas, paralelismo de build reduzido quando inevitável e limites operacionais conservadores, sem confundir limite de container com RSS esperado.

## 9. Migração incremental

### Etapa 0 — segurança e correção (antes da reorganização)

Corrigir vazamento de argv, versão do mapa, reautorização de long-poll/SSE, aplicação de exposes e limpeza de adapters. Rever confiança de extensões descobertas e limitar filas de PTY. Cada correção começa com teste de regressão; não mudar formato wire desnecessariamente.

**Cuidado de ordem:** aplicar `map.exposes` passará a tornar portas declaradas realmente alcançáveis. Corrigir/redigir debug e estabelecer política/consentimento antes de habilitar exposições; não “consertar conectividade” abrindo uma superfície vulnerável.

### Etapa 1 — idle previsível

Instrumentar baseline; retirar polling/N+1/escritas de presença redundantes. Testar invalidação, revogação, cache cold/start e concorrência. Reduzir trabalho por mapa inalterado sem quebrar renovação de token/presença.

### Etapa 2 — deploy mínimo

Build headless, imagem pronta para Linux, perfis de Compose control/node/host, secrets por arquivo, dados persistentes e testes de bootstrap/restart. CLI completa e comandos existentes continuam funcionando.

### Etapa 3 — interfaces de domínio e extensões

Extrair contratos/módulos e mover implementação de aplicações para pacotes opcionais. SDK/capabilities/trust/update transacional. Adaptadores antigos mantidos com depreciação explícita, sem renomear IDs públicos de uma vez.

### Etapa 4 — infra e jogos

Registro de serviços/grants, targets em redes de containers, adapter Coolify read-only, depois ações tipadas. Publicação pública TCP/UDP e quotas conforme testes de jogos; não depender de “share web” para semântica de túnel.

### Etapa 5 — social opcional

Identidade humana, autorização/retention e contrato social separado. Implementar perfis/avatares, depois amizades, salas e DMs. Testes negativos garantem que permissões sociais nunca concedem autoridade de infraestrutura.

Cada etapa precisa de PR pequeno, teste reproduzível, comparação de recursos e rollback. Não reescrever tudo enquanto as invariantes atuais continuam sem teste.

## 10. O que validar antes da implementação ampla

Recomendação padrão:

1. WM é conectividade + serviços; social e DevOps são opcionais.
2. Control puro e node são deployments independentes; host continua atalho.
3. Isolamento inicial por rede/host; multi-tenancy pública exige modelagem e testes próprios, não apenas múltiplas contas no mesmo admin atual.
4. Coolify começa read-only; execução privilegiada somente em receitas consentidas.
5. Primeiro segurança/correção + idle; depois artefatos headless; depois novas funcionalidades.

Pontos de produto ainda abertos: servidor público hospeda uma única comunidade confiada ou vários locatários não confiados? Jogos precisam de portas públicas para clientes sem agente WM? DMs exigem E2EE entre pessoas? Essas decisões alteram política e protocolo, não impedem as correções das primeiras etapas.
