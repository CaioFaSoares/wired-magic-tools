import { Link } from "react-router-dom";
import { Compass } from "lucide-react";
import { Button, LinkButton, PageHeader } from "../components/ui";

export function NotFoundPage() {
  return (
    <>
      <PageHeader eyebrow="Painel" title="Página não encontrada" />
      <div className="panel flex flex-col items-center justify-center px-5 py-14 text-center">
        <span className="mb-3 flex size-10 items-center justify-center rounded-lg border border-line bg-panel-2 text-faint">
          <Compass aria-hidden="true" className="size-5" />
        </span>
        <p className="font-mono text-xs text-faint">404</p>
        <p className="mt-1 text-sm font-medium text-ink">Esta rota não existe no painel</p>
        <p className="mt-1 max-w-md text-xs leading-5 text-muted">
          Verifique o endereço ou volte para a visão geral. As rotas disponíveis estão na barra
          lateral.
        </p>
        <div className="mt-4 flex flex-wrap items-center justify-center gap-2">
          <LinkButton to="/overview" variant="primary">Ir para a visão geral</LinkButton>
          <Link to="/nodes" className="text-xs text-accent hover:underline">
            Ver nós
          </Link>
        </div>
        <Button variant="ghost" size="sm" className="mt-2 text-faint" onClick={() => window.history.back()}>
          Voltar na história
        </Button>
      </div>
    </>
  );
}
