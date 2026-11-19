#!/usr/bin/env python3
"""Smoke HTTP do wm real; somente biblioteca padrão, loopback e estado descartável."""
import argparse
import base64
import http.client
import hashlib
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


def run(binary, startup_timeout, host_no_agent=False):
    root = Path(__file__).resolve().parents[1]
    binary = binary.resolve()
    check(binary.is_file() and os.access(binary, os.X_OK),
          "binário ausente/não executável; rode cargo build -p wired-magic antes")
    sources = [root / "Cargo.toml", root / "Cargo.lock"]
    sources += list((root / "crates").rglob("*.rs"))
    sources += list((root / "crates").rglob("Cargo.toml"))
    newer = sum(p.stat().st_mtime > binary.stat().st_mtime for p in sources if p.exists())
    print(f"Binário: {binary.name}; fontes Rust mais recentes: {newer}")
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
            'approval_required = true\n'
            'stun_enabled = false\nstun_listen = "127.0.0.1:0"\nrelay_enabled = false\n',
            encoding="utf-8")
        config.chmod(0o600)
        environment = {k: v for k, v in os.environ.items()
                       if not k.startswith("WM_") and k not in ("RUST_LOG", "RUST_BACKTRACE")}
        environment["RUST_LOG"] = "off"
        bootstrap_password = secrets.token_urlsafe(32)
        user_password = secrets.token_urlsafe(32)
        environment["WM_BOOTSTRAP_USERNAME"] = "setup-smoke"
        environment["WM_BOOTSTRAP_PASSWORD"] = bootstrap_password
        command = [str(binary), "serve", "--config", str(config)]
        control_data = temporary / "data"
        if host_no_agent:
            command = [str(binary), "host", "--no-agent", "--approval-required",
                       "--public-url", f"http://127.0.0.1:{port}",
                       "--listen", f"127.0.0.1:{port}", "--stun-listen", "127.0.0.1:0",
                       "--data-dir", str(temporary / "data")]
            control_data = control_data / "control"
        print("Modo: " + ("host --no-agent (sem TUN)" if host_no_agent else "serve"))
        def start_process():
            return subprocess.Popen(command,
                                    cwd=temporary, env=environment, stdin=subprocess.DEVNULL,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        def stop_process(child):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
            else:
                child.wait(timeout=5)

        process = start_process()
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

        def wait_healthy():
            deadline = time.monotonic() + startup_timeout
            while True:
                try:
                    health, _ = request("GET", "/healthz")
                    check(health.get("ok") is True, "health sem ok=true")
                    break
                except (OSError, http.client.HTTPException):
                    check(time.monotonic() < deadline, "timeout aguardando health")
                    time.sleep(0.1)

        def restart():
            nonlocal process
            stop_process(process)
            # Intentionally preserve the exact bootstrap environment and database.
            process = start_process()
            wait_healthy()

        wait_healthy()
        print("PASS health e inicialização isolada")
        api = "/api/v1"
        request("GET", api + "/admin/nodes", status=401)
        request("GET", api + "/map", token="wmn_invalid", status=401)
        check(request("GET", api + "/setup")[0]["required"] is True, "setup novo não está pendente")
        setup, headers = request("POST", api + "/setup/login", {"username": "setup-smoke", "password": bootstrap_password})
        setup_cookie = headers["set-cookie"].split(";", 1)[0]
        setup_csrf = setup["csrf_token"]
        request("GET", api + "/admin/nodes", cookie=setup_cookie, status=401)
        accounts = {"admin": {"username": "admin", "password": password},
                    "user": {"username": "daily", "password": user_password}}
        request("POST", api + "/setup/complete", accounts, cookie=setup_cookie, status=403)
        created, _ = request("POST", api + "/setup/complete", accounts, cookie=setup_cookie, csrf=setup_csrf)
        check(created["admin"]["role"] == "admin" and created["user"]["role"] == "user", "roles do setup incorretas")
        check(request("GET", api + "/setup")[0]["required"] is False, "setup não foi consumido")
        request("POST", api + "/setup/login", {"username": "setup-smoke", "password": bootstrap_password}, status=409)
        request("POST", api + "/setup/complete", accounts, cookie=setup_cookie, csrf=setup_csrf, status=409)
        restart()
        check(request("GET", api + "/setup")[0]["required"] is False, "restart reabriu setup")
        request("POST", api + "/setup/login", {"username": "setup-smoke", "password": bootstrap_password}, status=409)
        request("POST", api + "/setup/complete", accounts, cookie=setup_cookie, csrf=setup_csrf, status=409)
        user, headers = request("POST", api + "/auth/login", {"username": "daily", "password": user_password})
        user_cookie = headers["set-cookie"].split(";", 1)[0]
        check(user["user"]["role"] == "user", "conta cotidiana virou admin")
        request("GET", api + "/admin/nodes", cookie=user_cookie, status=403)
        request("GET", api + "/extensions/control_panel/runtime", cookie=user_cookie, status=403)
        request("POST", api + "/auth/logout", cookie=user_cookie, csrf=user["csrf_token"])
        request("GET", api + "/auth/session", cookie=user_cookie, status=401)
        print("PASS bootstrap env, setup atômico, CSRF, replay negado inclusive após restart e usuário sem admin")
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
        root_body, headers = request("GET", "/")
        check("text/plain" in headers.get("content-type", "") and b"headless" in root_body,
              "servidor default deve iniciar sem painel/Node")
        check(headers.get("cache-control") == "no-store", "root headless deve impedir cache")
        for route in ("/login", "/nodes", "/nodes/smoke-a", "/acl", "/assets/smoke-missing.js"):
            request("GET", route, status=404)
        panel, _ = request("GET", api + "/extensions/control_panel/runtime", cookie=cookie)
        check(panel["enabled"] is False, "instalação/default não pode ativar painel")
        request("GET", api + "/smoke-missing", status=404)
        print("PASS root headless, painel opcional inativo e 404 sem fallback indevido")

        # Minimal trusted static fixture, not a Node build or browser/React test.
        www = control_data / "extensions/control_panel/www"
        (www / "assets").mkdir(parents=True)
        assets = {"index.html": b"<!doctype html><html><script src='/assets/app.js'></script></html>",
                  "assets/app.js": b"console.log('trusted smoke fixture');"}
        for name, contents in assets.items():
            (www / name).write_bytes(contents)
        manifest = json.dumps({"schema": 1, "id": "control_panel", "version": "smoke",
                               "files": {name: hashlib.sha256(data).hexdigest() for name, data in assets.items()}},
                              sort_keys=True, separators=(",", ":")).encode() + b"\n"
        (www / "bundle.json").write_bytes(manifest)
        digest = hashlib.sha256(manifest).hexdigest()
        runtime = api + "/extensions/control_panel/runtime"
        panel, _ = request("GET", runtime, cookie=cookie)
        check(panel["installed"] is True and panel["enabled"] is False and panel["digest"] == digest,
              "instalar arquivos ativou painel ou digest divergiu")
        check(b"headless" in request("GET", "/")[0], "instalação publicou UI sem confirmação")
        enable = {"enabled": True, "digest": digest}
        request("PUT", runtime, enable, cookie=cookie, status=403)
        request("PUT", runtime, {"enabled": True, "digest": "0" * 64}, cookie=cookie, csrf=csrf, status=400)
        check(request("PUT", runtime, enable, cookie=cookie, csrf=csrf)[0]["enabled"] is True,
              "ativação com digest confirmado falhou")
        html, headers = request("GET", "/login")
        check(html == assets["index.html"] and "text/html" in headers.get("content-type", ""), "SPA opcional ausente")
        check(request("GET", "/relay-status")[0] == assets["index.html"], "página de status do relay indisponível")
        check("script-src 'self'" in headers.get("content-security-policy", ""), "CSP do painel ausente")
        head, headers = request("HEAD", "/assets/app.js")
        check(head == b"" and int(headers.get("content-length", "-1")) == len(assets["assets/app.js"]), "HEAD de asset incorreto")
        (www / "assets/app.js").write_bytes(b"changed without trust")
        check(request("GET", "/assets/app.js")[0] == assets["assets/app.js"], "snapshot ativo mudou pelo disco")
        restart()
        check(b"headless" in request("GET", "/")[0], "restart publicou pacote corrompido")
        check(request("GET", runtime, cookie=cookie)[0]["enabled"] is False, "pacote corrompido permaneceu ativo")
        (www / "assets/app.js").write_bytes(assets["assets/app.js"])
        request("PUT", runtime, enable, cookie=cookie, csrf=csrf)
        restart()
        check(request("GET", "/")[0] == assets["index.html"], "restart perdeu confiança de pacote intacto")
        for route in ("/api/smoke-missing", "/relay/smoke-missing", "/assets/smoke-missing.js"):
            request("GET", route, status=404)
        request("PUT", runtime, {"enabled": False}, cookie=cookie, csrf=csrf)
        restart()
        check(b"headless" in request("GET", "/")[0], "restart reativou painel desativado")
        check(request("GET", api + "/setup")[0]["required"] is False, "reinícios do painel reabriram setup")
        print("PASS painel: instalação inerte, CSRF/digest, snapshot, HEAD/CSP, persistência, corrupção e desativação após restart")
        admin("POST", "/logout")
        request("GET", api + "/admin/me", cookie=cookie, status=401)
        print("PASS logout invalida sessão; smoke HTTP concluído (não testa dataplane/Seance)")
    finally:
        if process is not None:
            stop_process(process)
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
    parser.add_argument("--host-no-agent", action="store_true",
                        help="exercita wm host --no-agent, incluindo data/control, sem TUN")
    args = parser.parse_args()
    if args.startup_timeout <= 0 or args.startup_timeout > 120:
        parser.error("startup-timeout deve estar entre 0 e 120 segundos")
    try:
        run(args.binary, args.startup_timeout, args.host_no_agent)
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
