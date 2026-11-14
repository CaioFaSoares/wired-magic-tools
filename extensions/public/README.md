# Extensões públicas

Extensões desta pasta fazem parte da distribuição aberta e do catálogo
documentado. Use `visibility = "public"` no manifesto, mantenha um `README.md`
com o contrato e o modelo de segurança, e evite segredos ou endpoints internos.

- `cloudflared/` — rotas de projeto para quick/named tunnel.
- `chat/` — TUI de chat (`node_driver = "none"`).
- `chat_gui/` — servidor web do chat, share `chat-gui-<nó>`.
- `debug/` — TUI de performance e descoberta (`node_driver = "none"`).
- `debug_gui/` — painel web, share `debug-gui-<nó>` e `debug-discover`.
