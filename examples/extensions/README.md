# Exemplos de extensões

O loader lê extensões de `<data_dir>/extensions/<id>/extension.toml`; este
diretório é apenas um catálogo de exemplos para copiar.

## `hello`

Sidecar declarativo mínimo usando o driver `exec`.

```sh
cp -R examples/extensions/hello <data_dir>/extensions/hello
# no painel: Extensões → Recarregar manifestos → Hello Sidecar → Configurar
```

A configuração por nó é um JSON validado:

```json
{
  "command": "/usr/local/bin/hello",
  "args": ["--port", "9000"],
  "env": {"HELLO_ENV": "prod"},
  "working_dir": "/opt/hello",
  "public_urls": ["https://hello.example.com"],
  "restart": "on_failure"
}
```

O comando roda sem shell, como o usuário do agente (root). Veja
[`docs/extensions.md`](../../docs/extensions.md) para o contrato completo e o
guia de extensões code-backed (cloudflared).
