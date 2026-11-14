# TUI macOS: UI de usuário e agente root

## Fluxo incremental implementado

Execute `wm tui` **como usuário normal**, sem `sudo`. O fluxo desktop atende um nó **root já inscrito** no diretório indicado por `--data-dir` (padrão: `/usr/local/var/wired-magic`). A UI grava seus próprios profiles, preferências e eventos em `~/Library/Application Support/wired-magic/tui`, não no diretório privado do daemon.

- Se o bridge autorizado já estiver disponível, a TUI conecta automaticamente. Um agente root existente pode ser conectado sem reiniciá-lo e sem alterar permissões do estado ou socket privado.
- Se não houver bridge, a TUI pede que você pressione **s**. Essa ação solicita autorização nativa para executar uma instalação one-shot: copia a versão atual do executável para um helper root-owned, guarda root/UID autorizados em configuração root-owned e inicia o bridge. Não há instalação silenciosa no startup.
- O diálogo nativo pode pedir **senha de administrador**. Touch ID e um novo diálogo em toda operação **não são garantidos**; o macOS controla disponibilidade e cache da autorização.
- Autorizar esse passo delega ao UID indicado operações de ciclo de vida e uma lista limitada da API do nó. O bridge só inicia o agente para o root aprovado pelo administrador; não aceita comandos CLI/shell arbitrários.
- **q** fecha apenas a UI. O agente e bridge continuam em segundo plano. Reabrir `wm tui` reconecta.
- Parar usa `shutdown` pela API autenticada. Um daemon legado sem esse método retorna erro explícito: nenhum PID arbitrário é sinalizado como fallback.
- Cancelar/recusar autorização produz um aviso; a UI continua utilizável. A autorização roda em worker, não na thread dos eventos de terminal.

## Fronteiras de segurança

O helper e configuração ficam em `/Library/PrivilegedHelperTools/wired-magic-desktop`, privados de root. O daemon mantém seu diretório privado 0700, estado secreto 0600 e socket interno root. Não há chmod/chown recursivo ou cópia do segredo para a UI.

O bridge expõe apenas `/Library/Application Support/wired-magic-desktop-UID/agent.sock`: diretório root-owned, socket 0600 para o UID autorizado. O servidor verifica credenciais do peer UID; o cliente verifica peer root. O proxy verifica peer root no socket privado. A configuração não pode ser trocada silenciosamente para outro root/UID; isso exige intervenção administrativa explícita.

Metadados retornados à UI são uma projeção explícita: nome, rede, IP/CIDR, identidade pública e origem do control. Tokens, chave privada, credenciais/query/fragmento do URL não são serializados. Métodos de execução/ativação de extensões e um proxy genérico de CLI não são expostos. Há limites de tamanho, concorrência e tempo no IPC.

Caminhos aprovados precisam ser root-owned em todos os ancestrais, sem escrita de grupo/outros ou symlinks não resolvidos. ACLs estendidas são rejeitadas conservadoramente, inclusive deny-only, em vez de serem removidas. Arquivos sensíveis existentes também são verificados. A autorização inicial aprova executar o binário atual uma vez como administrador; processos root persistentes executam sua cópia root-owned, não o binário de desenvolvimento/user-writable.

O lock de máquina mantém o inode legado em `/var/run/wired-magic/agent.lock` para impedir dois agentes, inclusive entre versões. Só nesse prefixo de compatibilidade, o grupo sistema `daemon` também integra a fronteira de confiança: macOS usa `/private/var/run` root:daemon 0775. O UID delegado não pode ter esse grupo primário ou suplementar. Diretório e arquivo são abertos por descritores com `O_NOFOLLOW`, verificados por `fstat`, sem ACL estendida, e o lock é mantido por toda a execução. Essa exceção nunca vale para código, estado ou socket IPC; não protege contra comprometimento de root ou do grupo sistema `daemon`.

## Limitações concretas

- **Não é um instalador completo de nó novo.** Enrollment inicial root, migração de estado e criação do diretório root não estão implementados neste fluxo. O formulário de join do perfil desktop informa essa limitação e não eleva silenciosamente.
- **Um root/UID aprovado por instalação.** Os profiles extras continuam no registry do usuário e não recebem elevação automática. Não há gestão privilegiada multiusuário/multiperfil.
- Um `/usr/local` ou outro ancestral pertencente ao usuário/Homebrew, gravável por grupo, ou com ACL estendida será **recusado**, mesmo que o diretório final seja root 0700. Não há migração automática ou afrouxamento de permissões para tornar esse caso aparentemente funcional. Um administrador precisa preparar uma localização confiável e o nó já inscrito.
- Para uma instalação administrativamente preparada, uma localização recomendada é `/Library/Application Support/wired-magic/node`, com todos os ancestrais root-owned/não graváveis/sem ACL estendida e o diretório do nó 0700. Depois de o administrador preparar e inscrever esse nó pela CLI existente (o enrollment inicial ainda pode exigir uma sessão root/`sudo` one-time), execute como usuário: `wm --data-dir '/Library/Application Support/wired-magic/node' tui`. Não mova estado ativo nem altere permissões recursivamente; esta etapa não automatiza migração ou substitui o enrollment privilegiado. Portanto não há promessa de instalação geral zero-sudo nesta entrega incremental.
- `/Library/PrivilegedHelperTools` precisa existir e ser confiável. Reconfiguração de root/UID e remoção do helper ainda são tarefas administrativas, não operações da TUI.
- Agentes iniciados pelo helper usam os defaults do agente e as exposições já persistidas; ajustes avançados da UI não são aplicados à configuração privilegiada nesta etapa.
- Agentes iniciados pelo helper **não executam extensões/hooks**. Um daemon legado conectado mantém sua política pré-existente, mas o bridge não permite ativar ou executar extensões.
- Um daemon legado pode ser conectado, porém só versões com API `shutdown` podem ser paradas por esse fluxo. Não há migração/restart automático para contornar isso.
- O bridge é um processo destacado, não uma instalação launchd: não há promessa de inicialização automática após reboot, assinatura/notarização ou upgrade transacional do helper. Após reboot, **s** pode requerer novamente autorização explícita para a cópia atual.

## Validação local realizada

Em 14/11/2026, com autorização explícita do usuário, o diálogo nativo executou a instalação da cópia privilegiada. O teste real confirmou autenticação do bridge e resposta do agente existente como UID 501 sem sudo, resistência a conexões fechadas antes da autenticação, duas aberturas da TUI sem privilégios, saída com `q` e reconexão mantendo o agente disponível. O comando instalado foi trocado atomicamente, com backup do executável anterior, para o mesmo build local validado. Esse build mantém a identificação 0.1.3 do checkout; não é uma release nova do Homebrew.

A suíte passou com 141 testes e 2 ignorados. As primeiras tentativas E2E revelaram particularidades de ACL ausente, permissões do runtime macOS e fechamento prematuro de probes, agora corrigidas. Não foram realizados testes destrutivos de parada/reinício do daemon existente, reboot ou cancelamento manual do diálogo; Touch ID continua sem garantia.
