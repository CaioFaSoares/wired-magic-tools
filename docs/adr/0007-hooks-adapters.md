# ADR 0007 — Ativação por nó, hooks de ciclo de vida e adapters declarativos

- **Status:** Aceito; implementação inicial
- **Data:** 2026-11-13
- **Decisores:** dono + implementação
- **Relacionado:** [ADR 0006](0006-extension-runtime.md),
  [extensions.md](../extensions.md), [hooks-adapters.md](../hooks-adapters.md)

## Contexto

O runtime de extensões (ADR 0006) já leva estado desejado no mapa e traz status
no report, com cloudflared e `exec`. Faltavam três coisas para o uso real:

1. a extensão existir no menu TUI e poder ser **ativada por nó**, em vez de
   rodar sempre que configurada;
2. uma forma de o nó executar **passos de montagem** (instalar/parar/limpar) sem
   embutir cada produto no binário;
3. uma forma de um produto como o **Seance** declarar o que precisa na malha
   (share, porta exposta, labels) sem o operador fazer tudo à mão.

## Decisão

1. **Ativação é por nó e persistida.**
   Tabela `extension_activations(extension_id,node_id,enabled,revision,...)`.
   O nó (token de nó) e o admin podem alternar. A ausência de linha mantém o
   comportamento anterior para não quebrar deploy existente:
   cloudflared com rota/túnel e exec com config contam como ativos.
   Criar uma rota cloudflared ativa a extensão.
2. **Catálogo no agente.**
   `GET /api/v1/node/extensions` devolve manifestos + ativação + status;
   `PUT /api/v1/node/extensions/{id}` alterna. A TUI lê o catálogo pelo socket
   local e ganhou a aba **Extensões** com `Enter/t`.
3. **Default config.**
   Manifesto pode declarar `default_config`; ativar uma extensão `exec` sem
   config materializa esse JSON validado. Isso torna “ativar” suficiente para
   um produto cujo manifesto já conhece o comando.
4. **Hooks no nó.**
   Manifesto declara `hooks.install/activate/deactivate` com command/args/env/
   timeout. `install` é uma vez por versão; `activate` antes do driver;
   `deactivate` depois. Execução sem shell, serializada por extensão, com
   `WM_*` e placeholders. Falha de install/activate vira status `error`; falha
   de deactivate é log.
5. **Adapters declarativos no control.**
   Manifesto declara labels, shares e exposes. O host materializa shares e
   labels enquanto ativo (idempotente, a cada 30 s e na ativação), com tabela de
   propriedade para não apagar share do usuário. Exposes entram no
   `NetworkMap.exposes` e o agente reconcilia listeners dinamicamente.
6. **Pasta de extensões no repositório.**
   `extensions/` é a fonte de verdade dos bundlados (cloudflared e Seance),
   embutida no binário e semeada em `<data_dir>/extensions` sem sobrescrever
   edições. Scripts de hook são semeados junto.
7. **Seance como receita.**
   `extensions/seance/` usa `default_config` + hooks de install/activate +
   adapter (labels `seance`/`baguette_port`, share `baguette-<nó>`, expose 8421).
   O operador só precisa ativar; o instalador real é o script/hook do manifesto.

## Alternativas

| Alternativa | Por que não |
| --- | --- |
| Sempre ativo, sem toggle | impõe sidecars e exposição a todos os nós; o operador não controla o blast radius |
| Ativação global (admin only) | a TUI do membro não consegue montar o nó; ativação local é o ponto do fluxo |
| Hooks em shell embutidos no Rust | engessa o produto; manifesto + script é auditável e editável |
| Hooks como `exec` com config normal | mistura processo longo com ação de lifecycle; não há “uma vez” nem deactivate |
| Adapter como código Rust para Seance | o Seance não deve exigir release do wm; declarativo cobre labels/share/expose |
| Expor via socat/nc instalado pelo hook | mais superfície, sem reconciliação, sem ACL; o data plane já tem o forward |
| `default_active = true` para tudo | upgrade ativaria sidecars inesperados; default false + fallback de compatibilidade |

## Consequências

- **Compatibilidade:** dados antigos continuam ativos; novos nós começam
  inativos para extensões node-side.
- **Segurança:** hooks rodam como root, ativação é uma ação de membro do nó (mas
  só sobre manifestos que o admin instalou). O modelo é “manifesto confiável”.
- **Convergência:** ativação e config bumpam a versão do mapa; hooks/status
  fluem pelo report. Nó revogado para sidecars e limpa exposes de extensão.
- **Custo:** catálogo é mais uma chamada HTTP por segundo na TUI; aceitável para
  o número de extensões do projeto. Adapter sync é O(nós × extensões) a cada
  30 s.
- **UI:** a TUI ganhou uma aba; o painel web mostra ativação por nó no
  cloudflared e no editor genérico.
- **Limites:** hooks não têm retry automático; exposes são TCP; share do adapter
  é corrigido pelo sync se editado.

## Critérios de aceite

- Cloudflared aparece inativo em nó novo e ativa por TUI/API; criar rota ativa.
- Ativar Seance cria label, share e expose; desativar desfaz sem tocar em share
  homônimo do usuário.
- `install` roda uma vez por versão; `activate`/`deactivate` rodam na ordem e
  com timeout; erro aparece no status.
- Nó revogado/pendente para os sidecars e remove exposes de extensão.
- `cargo test --workspace`, `pnpm typecheck` e `pnpm build` passam.
