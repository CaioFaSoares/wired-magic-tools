# Chat público (TUI)

O chat padrão é a TUI embutida no `wm`. Ela fala com o servidor `chat_gui`
pela mesma porta HTTP usada pelo painel web, então funciona em qualquer nó que
alcance o servidor pela malha — sem abrir outra porta.

```sh
# no nó servidor
sudo wm extensions enable chat_gui

# em outro nó (TUI)
wm chat --url http://10.66.0.x:8790 --author caio
```

Atalhos: `Enter` envia, `PgUp`/`PgDn` rolam, `Esc`/`q` saem. A TUI usa uma
única URL; para descobrir o endereço do share, use `wm open chat-gui-<nó> --print`
(ou abra pelo navegador). O servidor `chat_gui` publica o share
`chat-gui-<nó>` e expõe `8790` somente no overlay.

Esta extensão é `node_driver = "none"`: não há sidecar nem processo por nó. O
manifesto existe para o catálogo e para o gerenciador de pacotes distribuir o
pacote TUI. O binário `wm` é o artefato real.
