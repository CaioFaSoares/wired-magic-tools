# Serviços — candidato de teste

A geração de serviços agora usa argumentos diretos: launchd não invoca shell;
systemd recebe cada argumento escapado. Identidades agent e host são separadas.
Isso foi validado por testes de geração, não por instalação/reboot reais.

## Instalação explícita

Faça primeiro o bootstrap/join foreground e preserve o diretório de dados. O
serviço não herda opções do último processo nem variáveis temporárias do shell.
Informe novamente as opções desejadas:

```sh
sudo wm --data-dir /var/lib/wired-magic service install --mode agent --expose 8421
sudo wm --data-dir /var/lib/wired-magic service status --mode agent
```

O modo agent aceita `--control`, `--tun-name`, `--force-relay`, `--udp-port`,
`--mtu` e `--expose`. Sem `--control`, usa o estado existente do agente.

Modo host: `--mode host`, `--public-url`, `--listen`, `--tls-cert`, `--tls-key`,
`--no-agent`, `--approval-required`, além de porta UDP/MTU/expose. Opções
incompatíveis são rejeitadas. Bootstrap de senha/tags/nome/labels não é inferido:
faça-o antes no foreground. Não coloque senhas nos argumentos persistidos.

## Migração de instalação antiga

Uma instalação existente não é sobrescrita. Preserve o diretório de dados,
pare/remova o serviço legado com `wm service uninstall --mode agent` (o ID antigo
host também era agent), depois reinstale indicando o modo correto e todas as
opções. Confirme antes qual instância/serviço está sendo removida. O novo host
usa `wired-magic-host.service` ou `com.wired-magic.host`; agent usa o sufixo
`agent`. Status e uninstall aceitam `--mode`.

A CLI usa locks por diretório e de rotas da máquina para impedir agentes
concorrentes. Não apague arquivos de lock com um processo ativo. Este mecanismo
não substitui ainda a revisão de entradas core/GUI fora dos comandos CLI.

## Aceite pendente em máquina descartável

1. Instalar em caminho com espaços e confirmar argv efetivo sem shell.
2. Reiniciar SO e verificar mesma identidade, IP, expose e acesso permitido.
3. Confirmar tentativa de segunda instância rejeitada sem alterar rotas.
4. Parar/remover serviço e confirmar limpeza operacional/rotas.
5. Reinstalar com mesmo diretório e comprovar recuperação sem novo convite.

Não execute instalação na máquina do operador sem revisar os comandos e o
impacto em rotas/serviços. Linux systemd e macOS launchd precisam de ensaios
separados; testes de strings não equivalem a esses ensaios.
