#!/usr/bin/env python3
"""Real REST sidebar/environment PTYs, with synthetic files and loopback HTTP."""
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


def main():
    hits = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            hits.append(self.path)
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"ok")

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    cases = []
    try:
        with tempfile.TemporaryDirectory(prefix="kode-rest-pty-") as directory:
            base = Path(directory)
            catalog = base / "lsp.json"
            catalog.write_text('{"servers":[]}')
            for width, height in [(100, 34), (60, 18)]:
                case = base / str(width)
                project = case / "project"
                project.mkdir(parents=True)
                collection = {"info": {"name": "PTY collection"}, "item": [{"name": "Loopback", "request": {"method": "GET", "url": f"http://127.0.0.1:{server.server_port}/{width}"}, "response": [{"body": "saved example"}]}], "x-future": {"keep": True}}
                (project / "input.json").write_text(json.dumps(collection))
                (project / "selected.json").write_text("explicitly selected previous file")
                environment = {"KODE_LSP_CATALOG": str(catalog), "KODE_LSP_HOME": str(case / "no-lsp"), "KODE_RS_DATABASE_HOME": str(case / "database"), "KODE_RS_JDBC_HOME": str(case / "plugins"), "KODE_JAVA": str(case / "absent-java")}
                draft = f"# PTY comment {width}\nhost=loopback\nunfinished"
                actions = [
                    (b"Ready", 0.15, b"\x1b7"),
                    (b"REST API", 0.15, b"i"),
                    (b"Import Postman", 0.15, b"\x01" + paste("input.json") + b"\r"),
                    (b"Imported collection", 0.15, b"r"),
                    (b"REST run finished", 0.15, b"e"),
                    (b"Export Postman", 0.15, b"\x01" + paste("selected.json") + b"\r"),
                    (b"Exported collection", 0.15, b"\tn"),
                    (b"New environment", 0.15, b"\x01" + paste("PTY env") + b"\r"),
                    (b"Created environment", 0.35, paste(draft)),
                    (b"host=loopback", 0.15, b"\x1b7c"),
                    (b"Copy environment", 0.15, b"\x01" + paste("PTY copy") + b"\r"),
                    (b"Copied environment", 0.35, b"\x11"),
                ]
                result = terminal.run_case(project, case / "config", (width, height), actions,
                                           expected=[b"REST run finished", b"host=loopback", b"Copied environment"], env_overrides=environment)
                assert json.loads((project / "selected.json").read_text()) == collection
                state = json.loads((project / ".kode-rs.json").read_text())["restApi"]
                assert len(state["environments"]) == 2
                for item in state["environments"]:
                    assert state["x-kode-rs-environment-buffers"][item["id"]] == draft
                    assert item["values"][0]["key"] == "host"
                    assert item["values"][0]["value"] == "loopback"
                assert "responses" not in state and "cookies" not in state
                cases.append({"case": "import-run-export-environment-edit-copy-quit", "unknown_fields_retained": True, **result})
                result = terminal.run_case(project, case / "config", (width, height),
                                           [(b"host=loopback", 0.2, b"\x11")], expected=[b"host=loopback"], env_overrides=environment)
                cases.append({"case": "restore-environment-buffer-without-java", **result})
            assert hits == ["/100", "/60"], hits
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
    (ROOT / "docs/parity/R06-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} REST CLI/PTY checks")


if __name__ == "__main__":
    main()
