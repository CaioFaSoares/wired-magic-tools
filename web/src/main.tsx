import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./index.css";

async function bootstrap() {
  if (import.meta.env.VITE_MOCK === "1") {
    const { installMockApi } = await import("./lib/mock");
    installMockApi();
  }

  const container = document.getElementById("root");
  if (!container) throw new Error("elemento #root não encontrado");

  createRoot(container).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void bootstrap();
