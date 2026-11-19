# Painel React opcional do servidor

O servidor padrão é **headless**: `cargo build -p wired-magic`, a imagem padrão e
a release macOS não executam Node/pnpm, não exigem `web/dist` e não embutem React.
API HTTP, relay WSS, STUN, CLI/TUI e UIs de outras extensões continuam separados.
A raiz `/` informa o modo headless; rotas do painel retornam 404 até a ativação.

`control_panel` é uma extensão **do servidor**, não uma extensão ativável por nó.
Ela não inicia processos Node. Instalar arquivos não ativa a interface.
Conclua primeiro o [setup pela TUI](remote-admin.md), depois entre como admin.

## Gerar o pacote separadamente

Use apenas código/revisão que você confia. Node 22, pnpm conforme o manifesto Web
e Python 3.11+ são necessários **somente na máquina que empacota o painel**:

```sh
pnpm --dir web install --frozen-lockfile
pnpm --dir web typecheck
pnpm --dir web build
python3 -B -m unittest discover -s scripts -p 'test_package_control_panel.py' -v
python3 scripts/package_control_panel.py --output dist/control_panel.tar.gz
```

O script não sobrescreve saídas existentes e informa dois hashes diferentes:

- **Package SHA-256**: valida o arquivo transportado (também em `.sha256`).
- **Runtime digest**: SHA-256 dos bytes de `control_panel/www/bundle.json`, usado
  na confirmação de ativação. O manifesto contém hashes de todos os assets.

Alternativas opcionais: workflow manual [release-panel](../.github/workflows/release-panel.yml)
ou build de [Dockerfile.panel](../Dockerfile.panel):

```sh
docker build -f Dockerfile.panel --output type=local,dest=dist/panel-artifacts .
```

Esse build não é dependência da imagem padrão. Não é preciso reconstruir Rust.

## Instalar e ativar depois de iniciar

1. Transfira o pacote e valide o checksum **obtido de uma fonte confiável**.
   Um checksum distribuído junto de código malicioso não autentica sua origem.
   O painel executa JavaScript confiável na mesma origem da API; não é sandbox.
2. Como operador do servidor, extraia somente um pacote confiável sob o diretório
   de extensões do control plane. O layout final deve ser:

   ```text
   <control-data>/extensions/control_panel/extension.toml
   <control-data>/extensions/control_panel/www/bundle.json
   <control-data>/extensions/control_panel/www/index.html
   <control-data>/extensions/control_panel/www/assets/...
   ```

   Em `wm host --data-dir /data`, `<control-data>` é `/data/control`.
   Em `wm serve`, é o `data_dir` do arquivo de configuração.
   Preserve a propriedade/permissões do serviço. Não instale links simbólicos.
   Não existe endpoint de upload, shell ou extração arbitrária no servidor.
3. Abra `wm admin --server https://mesh.exemplo.com tui` e entre como **admin**.
   Vá a **Extensões**, pressione `p`, compare o digest mostrado com o **Runtime
   digest** do seu builder confiável, pressione `a` e confirme com `y`.
   `r` reinspeciona arquivos; `d` e `y` desativam sem parar a API/servidor.
4. Abra a URL do servidor no navegador. A tela de login é pública, mas os dados
   administrativos continuam exigindo sessão admin e CSRF. Conta comum e
   bootstrap não conseguem ativar o painel nem acessar APIs administrativas.

A confirmação usa um digest exato: se os arquivos mudarem entre inspeção e
ativação, o servidor recusa. Para atualizar, desative, substitua o pacote,
reinspecione e confirme o novo digest. O servidor guarda um snapshot verificado
em memória; mudar o disco não muda JavaScript já ativo. Clientes com assets em
cache podem precisar recarregar a página após atualização/desativação.

## Persistência e limites

A confiança fica em `<control-data>/control-panel-trust.sha256`, **fora do pacote**.
Instalar/extrair o pacote nunca deve escrever esse arquivo. Reinícios só restauram
um painel com o mesmo digest e hashes válidos; mudança, corrupção ou symlink
mantém o painel desativado. A ativação/entrega também falha fechada durante setup.

No máximo 1024 assets, 8 MiB por arquivo e 32 MiB no total; tipos estáticos e
caminhos são restritos. Em Unix, a leitura usa descritores relativos com
`O_NOFOLLOW` em cada componente. Plataformas sem esse mecanismo recusam carregar
o painel. Requests de assets não leem arquivos no disco. O padrão desativado
não carrega o bundle nem consulta o banco a cada rota estática. Isso não é uma
medição de CPU/RAM do servidor inteiro, que ainda mantém seus serviços de rede.

A API de runtime é `GET/PUT /api/v1/extensions/control_panel/runtime`, admin-only;
PUT exige `x-wm-csrf` e `{"enabled":true,"digest":"<sha256>"}` ou
`{"enabled":false}`. Rotas `/api`, `/relay`, `/healthz`, `/download` e `/j` nunca
são substituídas pelo fallback SPA; assets ausentes retornam 404, não HTML.
