#!/usr/bin/env python3
"""Create/edit/resolve/run/inspect/export/restart through the real REST CLI UI."""
import importlib.util
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("rest_editor_terminal", ROOT / "scripts/test-rest-editor-terminal.py")
editors = importlib.util.module_from_spec(spec)
spec.loader.exec_module(editors)


def current(marker, payload):
    def action(screen):
        if marker not in screen.text():
            return None
        return payload(screen) if callable(payload) else payload
    return action


def url_field(screen):
    position = editors.find(screen, "REQUEST |")
    if position is None:
        return None
    _, y = position
    x, y = (46, y + 1) if screen.width - 37 >= 24 else (38, y + 2)
    return f"\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m".encode()


def resize_response(screen):
    position = editors.find(screen, "────")
    if position is None:
        return None
    x, y = position
    return f"\x1b[<0;{x+1};{y+1}M\x1b[<32;{x+1};{y-1}M\x1b[<0;{x+1};{y-1}m".encode()


def main():
    hits = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            hits.append((self.path, self.headers.get("Cookie")))
            body = ("response-" + self.path.rsplit("/", 1)[-1]).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Content-Type", "text/plain")
            self.send_header("Set-Cookie", "workflow=transient; Path=/")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    cases = []
    try:
        with tempfile.TemporaryDirectory(prefix="kode-rest-runner-pty-") as directory:
            base = Path(directory)
            catalog = base / "lsp.json"
            catalog.write_text('{"servers":[]}')
            for width in [100, 60]:
                case = base / str(width)
                project = case / "project"
                project.mkdir(parents=True)
                collection = {"info": {"name": "Workflow"}, "variable": [{"key": "base", "value": f"http://127.0.0.1:{server.server_port}/{width}", "x-variable": True}], "item": [], "x-future": {"keep": True}}
                (project / ".kode-rs.json").write_text(json.dumps({"version": 1, "restApi": {"collection": collection}}))
                env = {"KODE_LSP_CATALOG": str(catalog), "KODE_LSP_HOME": str(case / "no-lsp"), "KODE_RS_DATABASE_HOME": str(case / "database"), "KODE_RS_JDBC_HOME": str(case / "plugins"), "KODE_JAVA": str(case / "absent-java")}
                rename = b"\x1b7\x1b[12~"
                actions = [
                    (b"Ready", .15, b"\x1b7f"),
                    (b"FOLDER | New folder", .2, rename),
                    (b"Rename REST item", .2, b"\x01" + editors.paste("Selected") + b"\r"),
                    (b"Selected", .2, b"n"),
                    (b"REQUEST | New request", .2, rename),
                    (b"Rename REST item", .2, current(b"New request", b"\x01" + editors.paste("First") + b"\r")),
                    (b"REQUEST | First", .2, url_field),
                    (b"REQUEST | First", .2, b"\x01" + editors.paste("{{base}}/first") + b"\x1b[13;5u"),
                    (b"response-first", .2, current(b"response-first", b"\x1b7n")),
                    (b"REQUEST | New request", .2, current(b"REQUEST | New request", rename)),
                    (b"Rename REST item", .2, current(b"New request", b"\x01" + editors.paste("Second") + b"\r")),
                    (b"REQUEST | Second", .2, url_field),
                    (b"REQUEST | Second", .2, b"\x01" + editors.paste("{{base}}/second") + b"\x1b[13;5u"),
                    (b"response-second", .2, current(b"response-second", b"\x1b7\x1b[H\x1b[Br")),
                    (b"Collection run: finish", .2, editors.click("First | HTTP")),
                    (b"response-first", .2, current(b"response-first", b"\x1b[18~")),
                    (b"Collection run: finish", .2, current(b"Collection run: finish", b"\x1b[F\r")),
                    (b"response-second", .2, current(b"response-second", editors.click("Pretty"))),
                    (b"response-second", .3, resize_response),
                    (b"response-second", .3, b"\x1b7e"),
                    (b"Export Postman", .2, b"\x01" + editors.paste("out.json") + b"\r"),
                    (b"Exported collection", .2, b"\x11"),
                ]
                result = editors.terminal.run_case(project, case / "config", (width, 34), actions, expected=[b"Collection run: finish", b"response-first", b"response-second", b"Exported collection"], env_overrides=env)
                saved = json.loads((project / ".kode-rs.json").read_text())
                state = saved["restApi"]
                exported = json.loads((project / "out.json").read_text())
                assert exported == state["collection"]
                assert exported["x-future"] == {"keep": True} and exported["variable"][0]["x-variable"] is True
                assert [item["name"] for item in exported["item"][0]["item"]] == ["First", "Second"]
                assert state["ui"]["prettyPrintResponses"] is False
                assert state["ui"]["responsePanelHeight"] == 14
                assert "response-first" not in json.dumps(state) and "workflow=transient" not in json.dumps(state)
                assert not saved["openEditors"]
                cases.append({"case": "create-edit-variable-resolve-send-folder-run-select-export-preferences", "unknown_fields_retained": True, **result})
                result = editors.terminal.run_case(project, case / "config", (width, 34), [(b"REQUEST | Second", .2, b"\x1b[13;5u"), (b"response-second", .2, b"\x11")], expected=[b"REQUEST | Second", b"response-second"], env_overrides=env)
                cases.append({"case": "restart-selected-request-preferences-and-fresh-cookie-owner", **result})
                expected = [(f"/{width}/first", None), (f"/{width}/second", "workflow=transient"), (f"/{width}/first", "workflow=transient"), (f"/{width}/second", "workflow=transient"), (f"/{width}/second", None)]
                assert hits[-5:] == expected, hits
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
    (ROOT / "docs/parity/R08-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} REST runner workflow CLI/PTY checks")


if __name__ == "__main__":
    main()
