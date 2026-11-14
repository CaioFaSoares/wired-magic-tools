#!/bin/sh
# Saída do contrato wm-proto::TuiDocument. Roda a cada refresh_secs.
set -eu
cat <<'JSON'
{
  "title": "TUI Panel Example",
  "subtitle": "aba fornecida por uma extensão",
  "sections": [
    {
      "title": "Status",
      "lines": [
        "Extensão: tui-panel",
        "Modo: snapshot",
        "Este texto veio do stdout do panel.sh."
      ]
    },
    {
      "title": "Tabela",
      "columns": ["Chave", "Valor"],
      "rows": [
        ["refresh", "5s"],
        ["driver", "none"],
        ["json", "TuiDocument"]
      ]
    }
  ],
  "footer": "Enter não abre nada: modo snapshot é somente leitura."
}
JSON
