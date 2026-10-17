#!/usr/bin/env python3
"""Synthetic provider + real GUI browser + real Kode PTYs. No host credentials."""
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import threading
import tempfile
from urllib.parse import parse_qs, urlencode, urlsplit

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("editors", ROOT / "scripts/test-rest-editor-terminal.py")
editors = importlib.util.module_from_spec(spec)
spec.loader.exec_module(editors)


def main():
    from playwright.sync_api import sync_playwright
    grants, logins, callbacks, hits, failures, cases = [], [], [], [], [], []
    pending = {}

    class Handler(BaseHTTPRequestHandler):
        def respond(self, body, content="application/json", status=200):
            body = body.encode()
            self.send_response(status)
            self.send_header("Content-Type", content)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            route = urlsplit(self.path)
            if route.path == "/authorize":
                params = {k: v[0] for k, v in parse_qs(route.query).items()}
                pending[params["state"]] = params
                assert params["code_challenge_method"] == "S256"
                self.respond('<form method="post" action="/login"><input name="user"><input name="password" type="password"><input name="state" type="hidden" value="' + params["state"] + '"><button>Authorize</button></form>', "text/html")
            elif route.path == "/protected":
                header = self.headers.get("Authorization")
                if header not in ["Bearer runtime-rotated", "Bearer runtime-access"]:
                    failures.append("protected request had no acquired token")
                    self.respond("unauthorized", "text/plain", 401)
                else:
                    hits.append(header)
                    self.respond("OAUTH_AUTHORIZED", "text/plain")
            else:
                self.respond("missing", "text/plain", 404)

        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            form = {k: v[0] for k, v in parse_qs(body.decode()).items()}
            if self.path == "/login":
                assert form["user"] == "synthetic-user" and form["password"] == "synthetic-password"
                params = pending[form["state"]]
                callback = params["redirect_uri"]
                parsed = urlsplit(callback)
                assert parsed.scheme == "https" and parsed.hostname == "localhost" and parsed.port > 0 and parsed.path == "/callback"
                callbacks.append(callback)
                logins.append(form["state"])
                self.send_response(302)
                self.send_header("Location", callback + "?" + urlencode({"state": form["state"], "code": form["state"]}))
                self.send_header("Content-Length", "0")
                self.end_headers()
            elif self.path == "/token":
                try:
                    assert self.headers["Authorization"] == "Basic " + base64.b64encode(b"fixture-client:fixture-secret").decode()
                    grant = form["grant_type"]
                    if grant == "authorization_code":
                        params = pending[form["code"]]
                        assert form["redirect_uri"] == params["redirect_uri"]
                        assert base64.urlsafe_b64encode(hashlib.sha256(form["code_verifier"].encode()).digest()).decode().rstrip("=") == params["code_challenge"]
                    elif grant == "refresh_token":
                        assert form["refresh_token"] == "runtime-refresh"
                    else:
                        assert grant == "client_credentials"
                    grants.append(grant)
                    token = "runtime-rotated" if grant == "refresh_token" else "runtime-access"
                    self.respond(json.dumps({"access_token": token, "refresh_token": "runtime-refresh", "token_type": "Bearer", "expires_in": 60}))
                except Exception as error:
                    failures.append(type(error).__name__)
                    self.respond('{"error":"invalid_grant"}', status=400)
            else:
                self.respond("missing", "text/plain", 404)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base_url = f"http://127.0.0.1:{server.server_port}"
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(executable_path="/usr/bin/google-chrome", headless=True, args=["--no-sandbox"])
            try:
                def login(screen):
                    # Reassemble the actual wrapped URL displayed by the form.
                    lines = ["".join(row)[37:].strip() for row in screen.cells]
                    start = next((i for i, line in enumerate(lines) if line.startswith("Login URL: ")), None)
                    if start is None:
                        return None
                    parts = [lines[start].removeprefix("Login URL: ")]
                    for line in lines[start + 1:]:
                        if line.startswith("Paste redirect URL"):
                            break
                        parts.append(line)
                    url = "".join(parts)
                    assert url.startswith(base_url + "/authorize?")
                    page = browser.new_page(ignore_https_errors=True)
                    try:
                        page.goto(url)
                        page.locator('[name="user"]').fill("synthetic-user")
                        page.locator('[name="password"]').fill("synthetic-password")
                        page.get_by_role("button", name="Authorize").click()
                        page.wait_for_url("https://localhost:*/callback?*")
                        assert "Authorization received" in page.locator("body").inner_text()
                    finally:
                        page.close()
                    return b""

                with tempfile.TemporaryDirectory(prefix="kode-oauth-browser-") as directory:
                    base = Path(directory)
                    catalog = base / "lsp.json"
                    catalog.write_text('{"servers":[]}')
                    for width in [100, 60]:
                        for grant in ["authorization_code", "client_credentials"]:
                            project = base / f"{width}-{grant}"
                            project.mkdir()
                            values = {"grant_type": grant, "authUrl": base_url + "/authorize", "accessTokenUrl": base_url + "/token", "clientId": "fixture-client", "clientSecret": "fixture-secret", "scope": "read write", "redirect_uri": "https://oauth.pstmn.io/v1/callback", "accessToken": "{{obsolete_token}}"}
                            auth = {"type": "oauth2", "oauth2": [{"key": k, "value": v} for k, v in values.items()]}
                            collection = {"info": {"name": "OAuth fixture"}, "item": [{"name": "OAuth request", "request": {"method": "GET", "url": base_url + "/protected", "auth": auth}}]}
                            original = json.dumps(collection, sort_keys=True)
                            state = {"version": 1, "restApi": {"collection": collection}, "activeOrigin": {"Virtual": {"owner": "rest-node", "key": "0", "label": "REST request"}}}
                            (project / ".kode-rs.json").write_text(json.dumps(state))
                            actions = [(b"REQUEST | OAuth", .15, editors.click("Auth", first=False)), (b"Auth: oauth2", .15, editors.click("Acquire token"))]
                            if grant == "authorization_code":
                                actions += [(b"Copy login URL", .15, login)]
                            actions += [(b"Refresh token", .15, editors.click("Refresh token")), (b"Refresh token", .8, b"\x1b[13;5u"), (b"OAUTH_AUTHORIZED", .15, b"\x11")]
                            env = {"KODE_LSP_CATALOG": str(catalog), "KODE_LSP_HOME": str(base / "no-lsp"), "KODE_RS_DATABASE_HOME": str(project / "db"), "KODE_RS_JDBC_HOME": str(project / "jdbc"), "KODE_JAVA": str(base / "absent-java")}
                            result = editors.terminal.run_case(project, base / "config", (width, 80), actions, env_overrides=env)
                            saved = (project / ".kode-rs.json").read_text()
                            assert json.dumps(json.loads(saved)["restApi"]["collection"], sort_keys=True) == original
                            assert all(secret not in saved for secret in ["runtime-access", "runtime-rotated", "runtime-refresh", "code_verifier", "synthetic-password"])
                            assert not failures, failures
                            cases.append({"case": grant + "-refresh-send-nonpersistence", "browser": grant == "authorization_code", **result})
                            if grant == "authorization_code":
                                manual_actions = [
                                    (b"REQUEST | OAuth", .15, editors.click("Auth", first=False)),
                                    (b"Auth: oauth2", .15, editors.click("Paste access token")),
                                    (b"Paste OAuth 2.0 access token", .15, b"\x01" + editors.paste("runtime-access") + b"\r"),
                                    (b"Clear token", .15, b"\x1b[13;5u"),
                                    (b"OAUTH_AUTHORIZED", .15, b"\x11"),
                                ]
                                manual = editors.terminal.run_case(project, base / "config", (width, 24), manual_actions, env_overrides=env)
                                cases.append({"case": "paste-access-token-send-nonpersistence", **manual})
                                saved = (project / ".kode-rs.json").read_text()
                                assert "runtime-access" not in saved
                            def restarted(screen):
                                visible = screen.text()
                                assert b"Refresh token" not in visible and b"Clear token" not in visible
                                return b"\x11"
                            restart = editors.terminal.run_case(project, base / "config", (width, 80), [(b"REQUEST | OAuth", .15, editors.click("Auth", first=False)), (b"Acquire token", .2, restarted)], env_overrides=env)
                            cases.append({"case": grant + "-restart-has-no-runtime-token", **restart})
                            print(f"Passed OAuth {grant} at {width} columns", flush=True)
            finally:
                browser.close()
        assert len(logins) == 2 and len(hits) == 6 and grants.count("refresh_token") == 4
        import socket
        for callback in callbacks:
            parsed = urlsplit(callback)
            with socket.socket() as probe:
                assert probe.connect_ex(("127.0.0.1", parsed.port)) != 0, "callback listener survived completion"
    finally:
        server.shutdown()
        server.server_close()
    (ROOT / "docs/parity/R10-pty-browser.json").write_text(json.dumps({"cases": cases, "passed": len(cases), "pkce_verified": True, "postman_replaced": True, "listeners_released": True}, indent=2) + "\n")


if __name__ == "__main__":
    main()
