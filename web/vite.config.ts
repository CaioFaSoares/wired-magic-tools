import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Em dev, o backend do control plane roda em 127.0.0.1:8080 por padrão.
// WM_PROXY_TARGET permite apontar para outra porta sem alterar o arquivo.
const proxyTarget = process.env.WM_PROXY_TARGET ?? "http://127.0.0.1:8080";

export default defineConfig({
  base: "/",
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    strictPort: false,
    proxy: {
      "/api": {
        target: proxyTarget,
        changeOrigin: true,
        // SSE precisa dos headers imediatamente. Sem selfHandleResponse, o
        // Node segura os headers até o primeiro chunk e o painel parece
        // "conectando" por até 15 s (o keepalive do backend).
        selfHandleResponse: true,
        configure(proxy) {
          proxy.on("proxyRes", (proxyRes, _req, res) => {
            if (res.headersSent) return;
            res.writeHead(proxyRes.statusCode ?? 502, proxyRes.headers);
            res.flushHeaders();
            proxyRes.pipe(res);
          });
          proxy.on("error", (_error, _req, res) => {
            if ("writeHead" in res && !res.headersSent) {
              res.writeHead(502, { "Content-Type": "application/json; charset=utf-8" });
              res.end(
                JSON.stringify({
                  error: { code: "proxy_error", message: "proxy de dev não alcançou o control plane" },
                }),
              );
            } else if ("end" in res) {
              res.end();
            }
          });
        },
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    target: "es2022",
    rollupOptions: {
      output: {
        manualChunks: {
          "react-vendor": ["react", "react-dom", "react-router-dom"],
          "query-vendor": ["@tanstack/react-query"],
          "yaml-vendor": ["yaml"],
        },
      },
    },
  },
});
