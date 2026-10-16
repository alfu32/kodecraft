#!/usr/bin/env python3
"""Exercise the actual request/response editors in isolated CLI terminals."""
import importlib.util
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("app_terminal", ROOT / "scripts/test-app-terminal.py")
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)


def paste(text):
    return b"\x1b[200~" + text.encode() + b"\x1b[201~"


def find(screen, label, first=True):
    matches = []
    for y, row in enumerate(screen.cells):
        text = "".join(row)
        start = text.find(label, 37)
        if start >= 0:
            matches.append((start, y))
    return (matches[0] if first else matches[-1]) if matches else None


def click(label, first=True):
    def payload(screen):
        position = find(screen, label, first)
        if position is None:
            return None
        x, y = position
        return f"\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m".encode()
    return payload


def main():
    hits = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            hits.append((self.path, body.decode(), self.headers.get("X-One"), self.headers.get("X-Two")))
            response = b'{"answer":"terminal-response"}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(response)))
            self.send_header("Set-Cookie", "synthetic=terminal; Path=/")
            self.end_headers()
            self.wfile.write(response)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    cases = []
    try:
        with tempfile.TemporaryDirectory(prefix="kode-rest-editor-pty-") as directory:
            base = Path(directory)
            catalog = base / "lsp.json"
            catalog.write_text('{"servers":[]}')
            for width, height in [(100, 34), (60, 34)]:
                case = base / str(width)
                project = case / "project"
                project.mkdir(parents=True)
                collection = {"info": {"name": "PTY editors"}, "item": [{"name": "PTY request", "request": {"method": "POST", "url": f"http://127.0.0.1:{server.server_port}/{width}", "header": [{"key": "X-Original", "value": "kept", "x-metadata": 7}], "body": {"mode": "raw", "raw": "body-fixture"}}}], "x-future": True}
                session = {"version": 1, "restApi": {"collection": collection}, "activeOrigin": {"Virtual": {"owner": "rest-node", "key": "0", "label": "REST request"}}}
                (project / ".kode-rs.json").write_text(json.dumps(session))
                environment = {"KODE_LSP_CATALOG": str(catalog), "KODE_LSP_HOME": str(case / "no-lsp"), "KODE_RS_DATABASE_HOME": str(case / "database"), "KODE_RS_JDBC_HOME": str(case / "plugins"), "KODE_JAVA": str(case / "absent-java")}
                body = f"payload-{width}\n"
                headers = "X-One: 1\nX-Two: 2"
                caret = []

                def up(screen):
                    caret.append((screen.x, screen.y))
                    return b"\x1b[A"

                def down(screen):
                    assert screen.y == caret[0][1] - 1, (caret, screen.x, screen.y, screen.text().decode())
                    return b"\x1b[B"

                def verify(screen):
                    assert (screen.x, screen.y) == caret[0], (caret, screen.x, screen.y)
                    position = find(screen, "Logical")
                    if position is None:
                        return None
                    x, y = position
                    return f"\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m".encode() + paste("MUST-NOT-MUTATE")

                actions = [
                    (b"REQUEST | PTY", 0.2, click("Headers")),
                    (b"X-Original", 0.2, b"\x01" + paste(headers)),
                    (b"X-Two", 0.2, click("Body")),
                    (b"body-fixture", 0.2, b"\x01" + paste(body) + b"\x1b[13;5u"),
                    (b"answer", 0.2, click("Headers")),
                    (b"X-Two", 0.3, click("Transaction")),
                    (b"Logical", 0.3, up),
                    (b"Logical", 0.2, down),
                    (b"Logical", 0.2, verify),
                    (b"read-only", 0.2, click("Raw")),
                    (b"REST raw", 0.2, b"\x11"),
                ]
                result = terminal.run_case(project, case / "config", (width, height), actions, expected=[b"answer", b"Logical", b"read-only"], env_overrides=environment)
                saved = json.loads((project / ".kode-rs.json").read_text())
                state = saved["restApi"]
                assert state["x-kode-rs-request-buffers"]["0"]["body"] == body
                assert state["x-kode-rs-request-buffers"]["0"]["headers"] == headers
                assert state["collection"]["item"][0]["request"]["body"]["raw"] == body
                assert state["collection"]["x-future"] is True
                assert not saved["openEditors"]
                assert "terminal-response" not in json.dumps(state) and "MUST-NOT-MUTATE" not in json.dumps(state)
                cases.append({"case": "headers-body-send-response-transaction-arrow-focus-readonly-raw", "header_caret_preserved": True, "only_owned_drafts_persisted": True, **result})
                result = terminal.run_case(project, case / "config", (width, height), [(b"REQUEST | PTY", 0.2, click("Body")), (body.strip().encode(), 0.2, b"\x11")], expected=[body.strip().encode()], env_overrides=environment)
                cases.append({"case": "restore-request-body-without-sending-or-starting-java", **result})
            assert hits == [("/100", "payload-100\n", "1", "2"), ("/60", "payload-60\n", "1", "2")], hits
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
    (ROOT / "docs/parity/R07-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} REST editor CLI/PTY checks")


if __name__ == "__main__":
    main()
