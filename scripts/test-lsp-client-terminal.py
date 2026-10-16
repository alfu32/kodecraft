#!/usr/bin/env python3
"""Exercise acknowledged LSP child lifecycle in isolated real CLI terminals."""
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
    with tempfile.TemporaryDirectory(prefix="kode-lsp-client-pty-") as directory:
        base = Path(directory)
        for size in [(100, 30), (60, 18)]:
            project = base / f"project-{size[0]}"
            project.mkdir()
            trace = base / f"trace-{size[0]}.jsonl"
            pid_file = base / f"pid-{size[0]}"
            catalog = base / f"catalog-{size[0]}.json"
            catalog.write_text(json.dumps({"servers": [{
                "id": "fixture", "name": "Fixture Server", "languages": ["kotlin"],
                "preferredCommand": [sys.executable, str(ROOT / "fixtures/lsp/server.py"), "fragment", str(trace), str(pid_file)],
                "configuration": {"fixture": {"option": 42}}, "versions": []
            }]}))
            actions = [
                (b"\x1b[?1049h", 0.2, b"\x1b7"),
                (b"stopped | Installed", 0.1, b"s"),
                (b"Started fixture", 0.1, b"r"),
            ]
            expected = [b"LSP Servers", b"Started fixture", b"running | Installed", b"Restarted fixture"]
            if size[0] == 100:
                actions += [(b"Restarted fixture", 0.1, b"s"), (b"Stopped fixture", 0.1, b"\x11")]
                expected.append(b"Stopped fixture")
            else:
                actions.append((b"Restarted fixture", 0.1, b"\x11"))
            result = terminal.run_case(project, base / f"config-{size[0]}", size, actions,
                expected=expected, env_overrides={"KODE_LSP_CATALOG": str(catalog), "KODE_LSP_HOME": str(base / f"install-{size[0]}")})
            messages = [json.loads(line) for line in trace.read_text().splitlines()]
            methods = [message.get("method") for message in messages]
            for method in ["initialize", "initialized", "shutdown", "exit"]:
                assert methods.count(method) == 2, f"{method}: {methods}"
            for message in messages:
                if message.get("method") == "initialize":
                    assert message["params"]["workspaceFolders"][0]["uri"] == project.as_uri()
            if sys.platform.startswith("linux"):
                assert not Path(f"/proc/{pid_file.read_text()}").exists(), "LSP child was not reaped"
            assert not (base / f"install-{size[0]}").exists(), "lifecycle mutated installation root"
            cases.append({"case": "settings-start-restart-stop-and-quit" if size[0] == 100 else "quit-with-running-server", "handshakes": 2, "graceful_exits": 2, "child_reaped": True, **result})
    (ROOT / "docs/parity/P02-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} LSP client CLI/PTY checks")


if __name__ == "__main__":
    main()
