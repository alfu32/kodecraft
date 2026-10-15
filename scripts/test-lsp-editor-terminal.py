#!/usr/bin/env python3
"""Real editor completion, save, native fallback and child disposal via CLI PTYs."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("app_terminal", ROOT / "scripts/test-app-terminal.py")
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-lsp-editor-pty-") as directory:
        base = Path(directory)
        for mode, size in [("fragment", (100, 30)), ("fragment", (60, 18)),
                           ("empty-primary", (100, 30)), ("crash-query", (60, 18)),
                           ("missing", (100, 30))]:
            name = f"{mode}-{size[0]}"
            root = base / name
            root.mkdir()
            path = root / "Use.kt"
            source = "class Customer { val id: Long = 0; val name: String = \"\" }\r\nfun inspect(customer: Customer) {\r\n    customer.\r\n}\r\n"
            path.write_bytes(source.encode())
            trace = base / f"{name}.jsonl"
            pid = base / f"{name}.pid"
            command = [str(base / "missing-executable")] if mode == "missing" else [sys.executable, str(ROOT / "fixtures/lsp/server.py"), mode, str(trace), str(pid)]
            catalog = base / f"{name}.json"
            catalog.write_text(json.dumps({"servers": [{"id": "fixture", "name": "Fixture",
                "languages": ["kotlin"], "versions": [], "preferredCommand": command,
                "configuration": {"fixture": {"option": 42}}}]}))
            marker = b"lsp" if mode == "fragment" else b"member"
            result = terminal.run_case(path, base / f"config-{name}", size, [
                (b"Ready", 0.2, b"\x1b[B\x1b[B\x1b[F\x00"),
                (marker, 0.1, b"\r"),
                (b"\x1b[?1049h", 0.1, b"\x13"),
                (b"\x1b[?1049h", 0.2, b"\x11"),
            ], expected=[marker], env_overrides={"KODE_LSP_CATALOG": str(catalog), "KODE_LSP_HOME": str(base / f"install-{name}")})
            inserted = "naïve" if mode == "fragment" else "id"
            assert path.read_bytes() == source.replace("customer.\r\n", f"customer.{inserted}\r\n").encode(), "wrong provider result or CRLF/UTF-8 save"
            if mode != "missing":
                messages = [json.loads(line) for line in trace.read_text().splitlines()]
                methods = [message.get("method") for message in messages]
                assert "textDocument/didOpen" in methods and "textDocument/completion" in methods
                if mode != "crash-query":
                    for method in ["textDocument/didChange", "textDocument/didSave", "textDocument/didClose", "shutdown", "exit"]:
                        assert method in methods, f"missing {method}: {methods}"
                    saved = next(message for message in messages if message.get("method") == "textDocument/didSave")
                    assert saved["params"]["text"].encode() == path.read_bytes()
                if sys.platform.startswith("linux"):
                    assert not Path(f"/proc/{pid.read_text()}").exists(), "LSP child was not reaped"
            assert not (base / f"install-{name}").exists(), "editor attachment installed a package"
            cases.append({"case": name, "provider": "lsp" if mode == "fragment" else "native", "exact_crlf_utf8_save": True, **result})
    (ROOT / "docs/parity/P03-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} LSP editor CLI/PTY checks")


if __name__ == "__main__":
    main()
