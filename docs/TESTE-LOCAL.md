# Smoke local do binário real

## Verificação integrada do candidato

Na raiz, com dependências já instaladas (Node/pnpm, Rust e cache Cargo local, Python 3.9+):

```sh
bash scripts/verify-local.sh
```

Executa typecheck/build Web, todos os testes Rust offline, contratos E2E normais e Python
`-O`, build debug e smoke HTTP real. Qualquer etapa reprovada encerra com erro. Guarda logs
privados por etapa e `manifest.json` com timestamp UTC, plataforma e SHA-256 das fontes,
assets e binário em `evidence/local/run.*`. Não substitui fmt/lint/auditoria de dependências;
não baixa pacotes, não executa Docker, TUN nem instala serviços. O commit pode ser nulo
quando o repositório ainda não tem commits; os hashes identificam o snapshot testado.
Mudanças posteriores invalidam correspondência ao snapshot (não o resultado histórico).

## Executar sem privilégios

Requer Python 3.9+ (somente biblioteca padrão) e o binário debug executável. Não usa root, sudo, TUN, Docker ou dependências Python externas.

Na raiz do repositório:

```sh
# Se necessário, primeiro compile o candidato; o smoke não compila automaticamente.
cargo build -p wired-magic
python3 scripts/smoke-local.py
```

Para um artefato diferente ou máquina lenta:

```sh
python3 scripts/smoke-local.py --binary target/debug/wm --startup-timeout 40
```

`--startup-timeout` aceita até 120 segundos; cada requisição HTTP tem timeout de 5 segundos. Saída 0 significa todas as verificações aprovadas; saída 1 indica falha. Binário inexistente/não executável falha com orientação de build. O script avisa se fontes Rust/manifests/assets de `web/dist` têm mtime posterior ao executável: um PASS nesse caso valida **somente aquele artefato**, não prova que ele representa o checkout atual. Alterações em fontes frontend precisam primeiro gerar `web/dist`, antes de reconstruir o executável que embute o painel.

## Isolamento e segurança

- Executa exclusivamente `wm serve --config ...`, nunca `host`, `join`, agente ou comandos de rede privilegiados.
- Seleciona porta TCP livre em `127.0.0.1`; configura HTTP/public URL loopback. A reserva é liberada antes do bind pelo processo: existe uma pequena corrida; colisão deve causar falha, não justificar expor outra interface.
- Desativa STUN (cujo default de produção pode escutar em todas as interfaces) e relay; configura também o endereço STUN como loopback porta zero.
- Remove variáveis herdadas `WM_*`, `RUST_LOG` e `RUST_BACKTRACE`, para impedir overrides acidentais; não usa proxies HTTP do ambiente.
- Cria diretório temporário privado, configuração 0600 e senha admin aleatória por execução. Credenciais não entram nos argumentos do processo e não são impressas. Tokens, respostas HTTP e logs do servidor são omitidos; não habilite trace/debug para investigar uma execução com credenciais reais.
- Gerencia o subprocesso com `finally`: terminate, wait limitado, kill/wait como fallback. Trata SIGINT/SIGTERM e remove somente o temporário criado por esta execução, após conferir caminho resolvido, identidade inode/device e dono. Não toca em dados existentes. SIGKILL, queda do Python/SO ou falha irrecuperável de filesystem não permitem garantir cleanup; nesse caso revise manualmente processos e diretórios `wm-smoke-*` privados antes de removê-los.

## O que verifica

1. Health HTTP real; login negativo, acesso admin sem sessão e mapa com token inválido.
2. Login admin positivo, cookie/sessão e CSRF; mutações sem CSRF/com CSRF incorreto são rejeitadas sem criar convite.
3. Convite admin de uso único; nomes/chaves inválidos não consomem uso; enrollment pendente não recebe mapa até aprovação.
4. Reuso de convite consumido é negado; conflito de nome e **violação UNIQUE de chave WireGuard** não gastam o segundo convite. Uma matrícula válida posterior com esse convite funciona; usos e lista de nós são conferidos, sem nós fantasmas.
5. Mapa com nome próprio e peer esperados; tags solicitadas pelo cliente não excedem a autoridade do convite.
6. ACL persistida restringe default; simulação permite TCP 8421 e nega 8422. YAML inválido é rejeitado sem mudar a ACL compilada.
7. Página raiz e deep links retornam SPA sem cache; assets locais referenciados pelo HTML respondem com conteúdo não HTML; asset/API inexistentes dão 404. Ausência do painel/assets é falha, não um PASS silencioso.
8. Logout invalida a sessão anterior.

## Limites e resultados

Este é um smoke **HTTP do control plane com o processo real**, não teste do dataplane WireGuard, TUN, DNS no sistema, tráfego direto, relay WebSocket, STUN, navegador/execução JavaScript, TLS ou integração real com Seance. As chaves públicas são bytes aleatórios bem formados, sem interface WireGuard e sem pares conectados; simular ACL não comprova enforcement de pacotes. Não cobre rollback de crash nem concorrência de matrícula.

Na preparação deste teste, o binário antigo falhou na etapa de enrollment; após rebuild pelo responsável, o smoke passou, incluindo conflito SQL/rollback, ACL e assets. Ainda houve aviso de fontes mais recentes durante trabalho paralelo: para evidência do candidato final, reconstrua após finalizar as mudanças e execute novamente os comandos acima. Nenhum token, senha ou estado persistente é necessário para reproduzir.
