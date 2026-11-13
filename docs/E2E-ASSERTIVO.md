# E2E Docker assertivo — T01/T02

Implementação preparada; **integração privilegiada e aceite G3 ainda pendentes**. Nenhum Docker/TUN foi executado nesta alteração. Não modifica Rust nem os READMEs existentes.

## Validação sem privilégios

```sh
bash -n scripts/e2e-docker.sh
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_e2e_assert.py
```

Os seis testes usam fixtures válidas/sabotadas: nome errado/duplicado/ausente, direto trocado por relay e vice-versa, probe desconectado, porta errada, falso booleano, política permitindo indevidamente, erro diferente de timeout, status desconectado, JSON inválido e processo que falha/expira. Provam os contratos do verificador, **não** transporte real.

## Execução futura, somente com autorização para Docker privilegiado

Pré-requisitos: binário Linux compatível em `target-linux/release/wm`, Docker com TUN/privileged, Python 3 no host. O Dockerfile instala curl, Python e iproute2. Usa o contexto Docker já configurado; não altera configuração do Docker nem usa nomes/volumes existentes.

```sh
bash scripts/e2e-docker.sh
# Cada execução abaixo deve terminar com exit != 0 e sem mensagem de sucesso:
E2E_SABOTAGE=route bash scripts/e2e-docker.sh
E2E_SABOTAGE=relay bash scripts/e2e-docker.sh
E2E_SABOTAGE=policy bash scripts/e2e-docker.sh
```

- `route`: adiciona blackhole /32 no container site para o IP de a1 descoberto no JSON; a resposta HTTP exata deve falhar.
- `relay`: desconecta apenas a2, force-relay, da rede descartável; probe conectado deve falhar.
- `policy`: omite a instalação da ACL restritiva; timeout negativo deve falhar porque a conexão continua permitida pela política inicial explícita do teste.

Sabotagem é um caso negativo deliberado, nunca um passe. Infraestrutura que falhar antes da sabotagem **não constitui** prova T02; conferir a fase reportada e executar primeiro o caso normal. O teste instala uma política inicial explícita (site/mac → mac TCP 8421) e confirma a2 → a1 conectado antes de restringi-la; não depende do default de produção.

## Contratos e isolamento

UUID por run para rede, imagem e containers; sem remoção preventiva, volumes compartilhados ou portas fixas. Antes de remover recurso Docker, confere nome exato no namespace e label de propriedade; antes de remover diretório, confere caminho absoluto resolvido do mktemp. Cleanup também ocorre em falha/INT/TERM. Não há KEEP: recursos sempre são descartados. Falha de cleanup é erro. Encerramento abrupto (SIGKILL, daemon indisponível) pode deixar recursos com label `wm.e2e.run`; investigar manualmente, nunca remover recursos por prefixo indiscriminadamente.

Cada comando tem limite de 30s (build: 300s), curl tem connect/max timeout. Convergência tenta até 60 vezes com intervalo de 0,5s; falha final é obrigatória. O limite total inclui os comandos (pode exceder 30s). Probe negativo requer JSON de sucesso do comando, peer/porta/caminho corretos, `connected: false` e `error: timeout`; erro de CLI/JSON/rede externa não conta como negação ACL. Asserts usam exceções explícitas, não `assert` do Python que desaparece sob `-O`.

São verificados nomes individuais a1/direct e a2/relay, status dos três agentes, probes, resposta HTTP com marcador exclusivo do run nos dois destinos, ACL compilada não vazia, acesso permitido após ACL e direto sobrevivendo à parada de control/relay. IPs HTTP vêm do JSON, não da ordem de enrollment. Force-relay não é teste de NAT real. Não substitui browser, ping/ICMP, integração Seance, jogo/UDP, reconexão ou aceite de campo.

## Credenciais e evidência

`umask 077`, tracing desligado e diretório/cookies temporários exclusivos. Senha aleatória; headers CSRF e payload login em arquivos privados. Tokens de enrollment continuam nos argumentos Docker/CLI (visíveis ao administrador do daemon), nunca impressos pelo script. stdout/stderr de processos que falham são retidos pelo helper e não publicados. Logs brutos de falha vão somente ao diretório privado e são apagados; saída sanitizada é uma lista permitida de contagens (linhas/ERROR/WARN/panic), sem conteúdo arbitrário, tokens, chaves ou cookies. Isso favorece confidencialidade e limita diagnóstico; não há pacote de logs brutos persistido.

Registrar externamente revisão/hash do binário, plataforma, comando, run ID, exit code e saída sanitizada quando a execução real for autorizada. Sucesso só aparece após todos os asserts; cleanup ainda pode transformar o exit final em falha. O plano de prontidão não foi marcado aprovado.
