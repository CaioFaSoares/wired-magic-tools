# Administração remota: core, CLI e TUI

Um nó pode administrar um control plane remoto quando o operador tem uma conta
**admin nesse servidor**. A inscrição do nó na VPN não concede poderes de admin.
A interface local usa a API administrativa existente, sessão e CSRF; não transmite
teclas para um shell no servidor nem compartilha sua sessão de terminal.

## Transporte e escopo

- Informe a URL do servidor explicitamente. Cada sessão pertence a essa URL.
- HTTPS é o padrão para conexões remotas. HTTP loopback é permitido; HTTP na
  malha requer `--allow-insecure-http-overlay` explícito e transmite os segredos
  sem TLS (confie também na proteção da malha).
- Redirecionamentos não são seguidos; cookies não são encaminhados a outro host.
- O servidor verifica autenticação em cada operação e CSRF nas mutações.
- Ser admin em A não permite administrar B. Sessões expiradas/revogadas exigem
  novo login. O token de inscrição do nó não substitui o login administrativo.

## TUI

```sh
wm admin --server https://vpn.example tui
```

Na TUI principal, o menu **Administrar servidor remoto (TUI)…** abre o mesmo
painel; ao sair, volta à interface local sem trocar a inscrição do nó.

O painel pede usuário e senha (mascarada), mantém a sessão só em memória e tem
páginas de visão geral, nós, ACL/acessos, extensões e status das operações locais.
HTTP na malha exige opt-in no formulário. ACL e configuração são editáveis no
próprio terminal; ações destrutivas pedem confirmação. Status não é um stream de
stdout/logs dos serviços remotos.

## CLI

```sh
# Use arquivo novo para cada login; não coloque senhas no histórico/argv.
# O programa que fornece a senha deve escrevê-la em stdin.
seu-gerenciador-de-segredos | wm admin --server https://vpn.example \
  --session-file ./admin-session.json login --password-stdin --username admin

wm admin --server https://vpn.example --session-file ./admin-session.json overview
wm admin --server https://vpn.example --session-file ./admin-session.json nodes
wm admin --server https://vpn.example --session-file ./admin-session.json approve UUID
wm admin --server https://vpn.example --session-file ./admin-session.json revoke UUID --yes
wm admin --server https://vpn.example --session-file ./admin-session.json acl
wm admin --server https://vpn.example --session-file ./admin-session.json acl --file politica.yaml
wm admin --server https://vpn.example --session-file ./admin-session.json extensions
wm admin --server https://vpn.example --session-file ./admin-session.json reload-extensions
wm admin --server https://vpn.example --session-file ./admin-session.json configure chat_gui UUID_DO_NO
wm admin --server https://vpn.example --session-file ./admin-session.json configure chat_gui UUID_DO_NO --file config.json
wm admin --server https://vpn.example --session-file ./admin-session.json activate chat_gui UUID_DO_NO
wm admin --server https://vpn.example --session-file ./admin-session.json logout
```

O arquivo de sessão contém credenciais, embora não contenha a senha. O login o
cria com 0600 em Unix e não sobrescreve um arquivo existente. Não comite, publique
nem compartilhe esse arquivo. Logout invalida a sessão no servidor; o arquivo
local deixa de ser útil. Leituras recusam arquivos inseguros ou de outro servidor.

ACL controla o tráfego da malha; aprovação/revogação de nós e revogação de tokens
de entrada controlam acesso à inscrição. A lista de tokens pode conter informação
sensível: evite redirecionar a saída para logs compartilhados.

## Extensões e instalação

O catálogo administrativo lista os manifestos carregados **no servidor escolhido**.
É possível recarregá-los e consultar/alterar configuração e ativação por nó pela
API existente. Pedidos do servidor continuam separados da decisão de ativação
local conforme o contrato do agente.

Instalar um pacote novo no disco do servidor ainda exige o fluxo de distribuição
do operador nesse servidor. Não foi introduzido um endpoint de shell interativo
ou instalação arbitrária no control plane.

**Admin é uma função altamente confiável.** O contrato preexistente de extensões
`exec` permite configurar executável, argumentos e diretório de trabalho. Essas
configurações podem executar código com os privilégios do agente no nó (inclusive
root); não são um sandbox. Só dê acesso administrativo a quem pode receber esse
poder, como faria com um administrador de serviços do sistema.

`configure --disabled --file ...` altera o registro de configuração; não substitui
`activate ... --off` para desativar a extensão. A extensão gerenciada cloudflared
usa sua API própria, não configuração genérica por este comando.
