# Extensões do wired-magic

Esta pasta é a fonte de verdade das extensões embutidas. O control plane copia
os arquivos para `<data_dir>/extensions/<id>/` no primeiro boot (sem sobrescrever
edições existentes). Os scripts de hook embutidos são a exceção: são arquivos
gerenciados e atualizados no boot; para customizar, copie a extensão para outro
id. O agente também semeia os assets embutidos no data dir do
nó, para que os hooks encontrem os scripts mesmo em nós remotos. Extensões
declarativas adicionais também podem ser criadas diretamente no data dir.

| id | driver | o que faz |
| --- | --- | --- |
| `cloudflared` | `cloudflared` | rotas de projeto para quick/named tunnel; ativável por nó |
| `seance` | `exec` | instala/roda o agente do Seance, expõe 8421 e publica `baguette-<nó>` |

## Ativação

Extensões com componente no nó são **por nó** e começam desativadas. Ative na
aba **Extensões** da TUI (`wm tui`, tecla `Enter`), no painel web ou pela API:

```sh
wm extensions                # lista o catálogo deste nó
wm extensions enable seance  # ativa no nó atual
wm extensions disable seance
```

Ativar `seance` faz o agente rodar `install.sh` → `activate.sh` → sidecar
`seance-agent`, além de aplicar labels, share e expose declarados no adapter.

## Criar uma extensão

Copie `seance/` ou crie um diretório com `extension.toml` em
`<data_dir>/extensions` (o control lê essa pasta no boot). O contrato completo está em
[`docs/extensions.md`](../docs/extensions.md) e
[`docs/hooks-adapters.md`](../docs/hooks-adapters.md). O mapa carrega o comando
dos hooks, não os arquivos: para customizar scripts, instale-os no nó (ou use um
comando inline).
