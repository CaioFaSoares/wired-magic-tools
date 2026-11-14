# Debug público (TUI)

O debug padrão é a TUI local embutida no `wm`; não depende de sidecar, porta ou
servidor. Ela mostra CPU, memória, load, o processo `wm`, top processos e um
resumo das extensões locais descobertas.

```sh
wm debug              # dashboard local
wm debug discover     # detalhe das extensões locais (debug-discover)
```

Atalhos: `q`/`Esc` saem, `r` atualiza. O painel web equivalente é a extensão
`debug_gui` (`wm debug_gui`), com share `debug-gui-<nó>`.

A descoberta usa `<data_dir>/extensions` e grava o estado em
`.debug-discovery.json`; a janela padrão para “nova”/“atualizada” é 24h. O
gerenciador de pacotes pode distribuir este manifesto, mas o artefato real é o
binário `wm`.
