#!/usr/bin/env python3
"""Qualify settings in real PTYs, using only an isolated local server fixture."""
import importlib.util
import json
from pathlib import Path
import shlex
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("app_terminal", ROOT / "scripts/test-app-terminal.py")
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-lsp-settings-pty-") as directory:
        base = Path(directory)
        for size in [(100, 30), (60, 18)]:
            project = base / f"project-{size[0]}"
            project.mkdir()
            executable = base / f"server-{size[0]}"
            invoked = base / f"unexpected-launch-{size[0]}"
            executable.write_text("#!/bin/sh\nprintf invoked > " + shlex.quote(str(invoked)) + "\n")
            executable.chmod(0o755)
            catalog = base / f"catalog-{size[0]}.json"
            catalog.write_text(json.dumps({"servers": [{"id": "fixture-server", "name": "Fixture Server", "languages": ["rust"], "preferredCommand": [str(executable), "--stdio", "two words"], "versions": [{"version": "manual", "downloads": []}]}]}))
            install = base / f"install-{size[0]}"
            result = terminal.run_case(project, base / f"config-{size[0]}", size, [
                (b"\x1b[?1049h", 0.2, b"\x1b7"),
                (b"stopped | Installed", 0.1, b"\r"),
                (b"invalid LSP catalog", 0.1, b"\x11"),
            ], expected=[b"LSP Servers", b"Fixture Server", b"stopped | Installed", b"invalid LSP catalog"], env_overrides={"KODE_LSP_CATALOG": str(catalog), "KODE_LSP_HOME": str(install)})
            assert not invoked.exists(), "settings discovery or unavailable lifecycle launched the fixture"
            assert not install.exists(), "manual server action created an installation directory"
            cases.append({"case": "settings-local-discovery-status-and-acknowledged-errors", **result})
    (ROOT / "docs/parity/P01-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} LSP settings CLI/PTY checks")


if __name__ == "__main__":
    main()
