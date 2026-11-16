# Extensões: core → CLI → TUI → GUI opcional

## Contrato de autoria

O domínio não depende da interface. Novas extensões first-party implementam o
core, expõem operações não interativas na CLI e só então uma TUI. A GUI é
opcional. Um servidor é uma região de execução do core, não uma exigência de GUI.

O SDK `wm-ext` aceita `[components]` opcional para compatibilidade com manifestos
anteriores. Quando declarado, exige `core = true`, `cli = true`, regiões não
vazias (`local`, `node`, `server`) e TUI antes de GUI. `server_extension` aponta
para um pacote relacionado: não instala nem ativa dependências implicitamente.

```toml
[components]
core = true
cli = true
tui = true
gui = false
regions = ["local", "server"]
server_extension = "chat_gui"
```

Chat e debug organizam sua implementação em `crates/wm/src/extensions/<id>/`:
`core`, `cli`, `tui` e `gui`. Os módulos antigos são adaptadores de compatibilidade.
Os manifestos/assets de distribuição permanecem em `extensions/public/<id>`;
os IDs históricos `chat_gui` e `debug_gui` não são removidos de instalações.

## CLI sem terminal interativo

```sh
wm chat --url http://127.0.0.1:8790 info
wm chat --url http://127.0.0.1:8790 list --after 0
wm chat --author operador send 'Olá'
wm chat serve --listen 127.0.0.1:8790
wm debug snapshot
wm extensions discover --json --no-state
wm extensions context
wm extensions logs chat_gui --limit 100
wm extensions preferences
wm extensions favorite chat
wm extensions autoload chat
wm extensions configure chat url http://10.66.0.2:8790
wm extensions autoload chat --off
```

`wm chat` e `wm debug` preservam a abertura da TUI; os subcomandos `tui` a
selecionam explicitamente. `chat_gui`/`debug_gui` continuam compatíveis. A sala
continua sem autenticação própria: mantenha loopback/overlay e ACL adequadas.

Favorito, autoload e ativação de serviços são decisões independentes. As
preferências ficam no diretório do perfil em `extension-preferences.json`, com
permissões 0600 em Unix. Opções são apenas de apresentação, não armazene senhas
ou tokens nelas. Descobrir um manifesto não autoriza executar seus comandos.

## API do host e múltiplos servidores

A API local expõe `extension_context` (versão 1): um snapshot com `host`,
`nodes`, `shares` e `extensions`, sem credenciais de inscrição. O SDK exporta
`ExtensionContext` e `ServerContext { profile, context }`. A CLI imprime esse
contrato com `wm --data-dir <perfil> extensions context`.

Cada inscrição tem seu próprio socket `agent.sock`. Para trabalhar com múltiplos
servidores, consulte explicitamente cada perfil e preserve sua identidade;
não una peers apenas pelo nome, pois redes diferentes podem repetir nomes.
Esse contrato não concede acesso a outros perfis ou servidores remotos.
Para administração autenticada de um control plane remoto, use a
[CLI/TUI administrativa](remote-admin.md): ela exige login admin no servidor
escolhido e não reutiliza o token de inscrição do nó.

Hooks recebem `WM_API_SOCKET`, `WM_API_VERSION` e `WM_CONTEXT_METHOD`, além do
contexto existente `WM_NODE_ID`, `WM_NODE_NAME`, `WM_CONTROL_URL`,
`WM_EXTENSION_DIR` e `WM_EXTENSION_CONFIG_FILE`. No socket Unix, envie um
`LocalRequest` JSON delimitado por newline, por exemplo:

```json
{"id":1,"method":"extension_context","params":null}
```

A resposta é um `LocalResponse`, também delimitado por newline. O socket segue
as permissões do agente. Hooks locais são código confiado pelo operador, não
um sandbox de capacidades; não instale código de origem desconhecida.

## Reutilização da TUI

O contrato `TuiDocument` permite que a extensão produza apenas título, seções,
linhas e tabelas JSON (`mode = "snapshot"`), reutilizando o renderer do WM.
`mode = "pane"` empresta um PTY embutido para interfaces próprias;
`mode = "exec"` mantém a opção explícita de terminal de tela cheia.
Manifestos TUI são validados antes de serem aceitos pelo loader.
Os três modos usam o mesmo resolvedor de `{wm}`, `{extension_dir}` e
`{option:chave}`, sem interpolação de shell. A opção `cwd` é relativa ao diretório
da extensão, quando disponível, ou ao diretório do perfil. Opções definidas pelo
operador são código/configuração confiada, não uma fronteira de sandbox.

## Navegação, favoritos e descoberta

Na aba **Extensões**, `h`/`l` alternam **Servidor**, **Locais**, **Descoberta** e
**Settings**. Servidor mostra o catálogo conhecido pelo agente daquele perfil;
não se trata de uma sessão de administrador remoto. `e` edita favorito, autoload
e opções locais; `L` abre estado/erros e solicita logs retidos. A descoberta
(`r` na seção Descoberta) lê manifestos sem instalar nem iniciar serviços.

Favoritos restauram abas por ID, independentemente da ordenação do catálogo.
Autoload carrega as interfaces Snapshot/Pane selecionadas de forma preguiçosa e sem
bloquear o event loop; nunca abre Exec fullscreen automaticamente nem ativa
sidecars. Mudar o spec/opções encerra a apresentação antiga e invalida suas
respostas pendentes, inclusive eventos de PTYs substituídos.

## Logs por extensão

`wm --data-dir <perfil> extensions logs <id> --limit 100` consulta o RPC local
read-only `extension_logs`, com parâmetros `{ "id": "chat_gui", "limit": 100 }`.
A resposta contém `id`, `entries` (`timestamp`, `source`, `message`) e limites de
retenção. A CLI aceita até 256 registros por consulta; o RPC aceita até 500.
Capturam-se eventos de ciclo de vida, stdout/stderr dos sidecars Exec/cloudflared
e saída dos hooks. O buffer é próprio de cada agente/perfil e fica apenas em
memória: 15 minutos, no máximo 500 registros por extensão, 128 IDs e 4096
registros totais, com 2048 bytes por mensagem. Reiniciar o agente perde os logs;
um ID sem dados retidos retorna erro, sem inventar histórico.

Linhas de saída excessivas são descartadas e ambas as pipes são drenadas sem
bloquear o processo. Controles ANSI são removidos; marcadores de credenciais,
códigos WM e saída de configuração reconhecível são ocultados antes do recorte.
Isso é defesa em profundidade, não detecção universal de segredos: extensões
não devem escrever credenciais em stdout/stderr. Esses logs são locais; o painel
admin remoto não transmite os arquivos de log de todos os nós.
