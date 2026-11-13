#!/usr/bin/env python3
"""Smoke HTTP do wm real; somente biblioteca padrão, loopback e estado descartável."""
import argparse
import base64
import http.client
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time


class Failure(Exception):
    pass


def check(condition, label):
    if not condition:
        raise Failure(label)


def run(binary, startup_timeout):
    root = Path(__file__).resolve().parents[1]
    binary = binary.resolve()
    check(binary.is_file() and os.access(binary, os.X_OK),
          "binário ausente/não executável; rode cargo build -p wired-magic antes")
    sources = [root / "Cargo.toml", root / "Cargo.lock"]
    sources += list((root / "crates").rglob("*.rs"))
    sources += list((root / "crates").rglob("Cargo.toml"))
    sources += [p for p in (root / "web" / "dist").rglob("*") if p.is_file()]
    newer = sum(p.stat().st_mtime > binary.stat().st_mtime for p in sources if p.exists())
    print(f"Binário: {binary.name}; fontes/assets mais recentes: {newer}")
    if newer:
        print("AVISO: reconstrua com cargo build -p wired-magic; este resultado vale apenas para o binário existente.")
    temporary = Path(tempfile.mkdtemp(prefix="wm-smoke-" )).resolve()
    identity = temporary.stat()
    process = None
    original_handlers = {}
    try:
        os.chmod(temporary, 0o700)
        with socket.socket() as reserve:
            reserve.bind(("127.0.0.1", 0))
            port = reserve.getsockname()[1]
        # A porta é liberada antes do bind pelo wm; colisão é falha, nunca reutilizar outro servidor.
        password = secrets.token_urlsafe(32)
        config = temporary / "control.toml"
        config.write_text(
            f'listen = "127.0.0.1:{port}"\npublic_url = "http://127.0.0.1:{port}"\n'
            f'data_dir = {json.dumps(str(temporary / "data"))}\n'
            f'admin_password = {json.dumps(password)}\napproval_required = true\n'
            'stun_enabled = false\nstun_listen = "127.0.0.1:0"\nrelay_enabled = false\n',
            encoding="utf-8")
        config.chmod(0o600)
        environment = {k: v for k, v in os.environ.items()
                       if not k.startswith("WM_") and k not in ("RUST_LOG", "RUST_BACKTRACE")}
        environment["RUST_LOG"] = "off"
        process = subprocess.Popen([str(binary), "serve", "--config", str(config)],
                                   cwd=temporary, env=environment, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        def interrupted(signum, frame):
            raise Failure("interrompido por sinal")
        for sig in (signal.SIGINT, signal.SIGTERM):
            original_handlers[sig] = signal.signal(sig, interrupted)

        def request(method, path, body=None, *, cookie=None, csrf=None, token=None, status=200):
            check(process.poll() is None, "processo wm encerrou inesperadamente")
            headers = {}
            if body is not None:
                headers["Content-Type"] = "application/json"
            if cookie:
                headers["Cookie"] = cookie
            if csrf:
                headers["x-wm-csrf"] = csrf
            if token:
                headers["Authorization"] = "Bearer " + token
            connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
            try:
                connection.request(method, path, json.dumps(body) if body is not None else None, headers)
                response = connection.getresponse()
                payload = response.read(2 * 1024 * 1024)
                check(response.status == status,
                      f"{method} {path}: HTTP {response.status}, esperado {status}")
                result = json.loads(payload) if "application/json" in response.getheader("Content-Type", "") else payload
                return result, dict(response.getheaders())
            finally:
                connection.close()

        deadline = time.monotonic() + startup_timeout
        while True:
            try:
                health, _ = request("GET", "/healthz")
                check(health.get("ok") is True, "health sem ok=true")
                break
            except (OSError, http.client.HTTPException):
                check(time.monotonic() < deadline, "timeout aguardando health")
                time.sleep(0.1)
        print("PASS health e inicialização isolada")
        api = "/api/v1"
        request("GET", api + "/admin/nodes", status=401)
        request("GET", api + "/map", token="wmn_invalid", status=401)
        request("POST", api + "/admin/login", {"username": "admin", "password": "wrong"}, status=401)
        login, headers = request("POST", api + "/admin/login", {"username": "admin", "password": password})
        cookie = headers["set-cookie"].split(";", 1)[0]
        csrf = login["csrf_token"]
        def admin(method, path, body=None, status=200):
            return request(method, api + "/admin" + path, body, cookie=cookie, csrf=csrf, status=status)[0]
        check(admin("GET", "/session")["csrf_token"] == csrf, "sessão/CSRF divergentes")
        invitation = {"name": "smoke", "max_uses": 1, "ttl_secs": 600, "tags": ["smoke"]}
        request("POST", api + "/admin/join-tokens", invitation, cookie=cookie, status=403)
        request("POST", api + "/admin/join-tokens", invitation, cookie=cookie, csrf="wrong", status=403)
        check(admin("GET", "/join-tokens") == [], "CSRF inválido criou convite")
        print("PASS login, autenticação negativa, sessão e CSRF sem mutação")

        def enroll(token, name, key=None, status=200):
            return request("POST", api + "/enroll", {
                "join_token": token, "name": name, "hostname": name + ".local",
                "os": "smoke", "arch": "test", "version": "0.1.0",
                "wg_public_key": key or base64.b64encode(secrets.token_bytes(32)).decode(),
                "tags": ["untrusted"], "nat_type": "unknown"
            }, status=status)[0]
        first_invite = admin("POST", "/join-tokens", invitation)
        join = first_invite["token"]
        enroll(join, "bad/name", status=400)
        enroll(join, "bad-key", key="invalid", status=400)
        check(admin("GET", "/join-tokens")[0]["uses"] == 0, "validação consumiu convite")
        first_key = base64.b64encode(secrets.token_bytes(32)).decode()
        first = enroll(join, "smoke-a", key=first_key)
        enroll(join, "reuse", status=401)
        request("GET", api + "/map", token=first["node_token"], status=403)
        admin("POST", "/nodes/" + first["node_id"] + "/approve")
        # Conflito de nome deve preservar o novo convite para uma matrícula válida.
        second_invite = admin("POST", "/join-tokens", dict(invitation, name="rollback"))
        enroll(second_invite["token"], "smoke-a", status=409)
        enroll(second_invite["token"], "duplicate-key", key=first_key, status=409)
        check(all(t["uses"] == (1 if t["id"] == first_invite["id"] else 0)
                  for t in admin("GET", "/join-tokens")), "conflito SQL consumiu convite")
        second = enroll(second_invite["token"], "smoke-b")
        admin("POST", "/nodes/" + second["node_id"] + "/approve")
        nodes = admin("GET", "/nodes")
        check({n["name"] for n in nodes} == {"smoke-a", "smoke-b"}, "nós fantasma após falhas")
        check(all(t["uses"] == 1 for t in admin("GET", "/join-tokens")), "contagem de usos inválida")
        network, _ = request("GET", api + "/map?version=0&wait=1", token=first["node_token"])
        check(network["me"]["name"] == "smoke-a", "nome próprio no mapa")
        check([p["name"] for p in network["peers"]] == ["smoke-b"], "nomes dos peers")
        check(network["peers"][0]["tags"] == ["smoke"], "tags fora da autoridade do convite")
        print("PASS enroll, pending/aprovação, nomes, mapa, uso único e rollback de validação/conflito")
        yaml = 'version: 1\nrules:\n  - action: accept\n    src: ["tag:smoke"]\n    dst: ["tag:smoke"]\n    protocol: tcp\n    ports: ["8421"]\n'
        acl = admin("PUT", "/acl", {"yaml": yaml})
        check(acl["compiled"]["default_allow"] is False, "ACL não restringiu default")
        for port_number, allowed in [(8421, True), (8422, False)]:
            sim = admin("POST", "/acl/simulate", {"src": "smoke-a", "dst": "smoke-b", "protocol": "tcp", "port": port_number})
            check(sim["allowed"] is allowed, "simulação ACL incorreta")
        admin("PUT", "/acl", {"yaml": "[invalid"}, status=400)
        check(admin("GET", "/acl")["compiled"] == acl["compiled"], "ACL inválida alterou compilação persistida")
        print("PASS ACL permit/deny e rollback de política inválida (somente control plane)")
        html, headers = request("GET", "/")
        if "text/html" not in headers.get("content-type", ""):
            raise Failure("painel não embutido; compile frontend antes do binário")
        for route in ("/login", "/nodes", "/nodes/smoke-a", "/acl"):
            document, h = request("GET", route)
            check(document == html and h.get("cache-control") == "no-cache", "deep link não retornou SPA sem cache")
        assets = re.findall(rb'(?:src|href)=[\"\'](/assets/[^\"\']+)', html)
        check(assets, "HTML sem assets locais verificáveis")
        for asset in assets:
            content, h = request("GET", asset.decode())
            check(content and "text/html" not in h.get("content-type", ""), "asset retornou HTML/vazio")
        request("GET", "/assets/smoke-missing.js", status=404)
        request("GET", api + "/smoke-missing", status=404)
        print("PASS deep links SPA, assets locais e 404 sem fallback indevido")
        admin("POST", "/logout")
        request("GET", api + "/admin/me", cookie=cookie, status=401)
        print("PASS logout invalida sessão; smoke HTTP concluído (não testa dataplane/Seance)")
    finally:
        if process is not None:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            else:
                process.wait(timeout=5)
        for sig, handler in original_handlers.items():
            signal.signal(sig, handler)
        # Remover exclusivamente o diretório criado aqui, verificando identidade/dono.
        current = temporary.lstat()
        check(not temporary.is_symlink() and current.st_ino == identity.st_ino
              and current.st_dev == identity.st_dev and current.st_uid == identity.st_uid
              and temporary.name.startswith("wm-smoke-"), "identidade do temporário mudou; cleanup recusado")
        shutil.rmtree(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/debug/wm")
    parser.add_argument("--startup-timeout", type=float, default=20)
    args = parser.parse_args()
    if args.startup_timeout <= 0 or args.startup_timeout > 120:
        parser.error("startup-timeout deve estar entre 0 e 120 segundos")
    try:
        run(args.binary, args.startup_timeout)
    except Failure as error:
        # Apenas rótulos nossos; nunca corpos HTTP, tokens ou senha.
        print("FAIL " + str(error), file=sys.stderr)
        return 1
    except (OSError, ValueError, KeyError, TypeError, http.client.HTTPException, subprocess.SubprocessError):
        # Não imprimir exceptions/tracebacks ou corpos HTTP: podem carregar credenciais.
        print("FAIL smoke local; etapa indicada pelo último PASS. Verifique build/API/porta disponível. Credenciais e respostas omitidas.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
