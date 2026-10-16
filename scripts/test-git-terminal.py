#!/usr/bin/env python3
"""Real Git panel input/acknowledgements in isolated local repository PTYs."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("app_terminal", ROOT / "scripts/test-app-terminal.py")
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args]).decode().strip()


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-git-pty-") as directory:
        base = Path(directory)
        catalog = base / "empty-catalog.json"
        catalog.write_text('{"servers":[]}')
        environment = {"KODE_LSP_CATALOG": str(catalog), "KODE_LSP_HOME": str(base / "no-installs")}
        for width, height in [(100, 30), (60, 18)]:
            root = base / f"project-{width}"
            root.mkdir()
            git(root, "init", "--template=", "--initial-branch=main")
            for name, value in [("user.name", "Synthetic Author"), ("user.email", "synthetic@example.invalid"),
                                ("core.autocrlf", "false"), ("core.excludesfile", str(base / "no-excludes")),
                                ("core.attributesfile", str(base / "no-attributes"))]:
                git(root, "config", name, value)
            (root / ".gitignore").write_text(".kode-rs*\n")
            path = root / "Note.txt"
            path.write_bytes(b"original\r\n")
            git(root, "add", ".gitignore", "Note.txt")
            git(root, "-c", "core.hooksPath=/dev/null", "commit", "-m", "baseline")
            before = git(root, "rev-parse", "HEAD")
            path.write_bytes(b"changed\r\n")
            message = f"PTY commit {width}"
            def mouse(screen):
                for y,row in enumerate(screen.cells):
                    x=''.join(row).find('[ Commit ]')
                    if x>=0:return f'\x1b[<0;{x+3};{y+1}M\x1b[<0;{x+3};{y+1}m'.encode()
                return None
            actions = [
                (b"Ready", 0.15, b"\x1b3"),
                (b"Note.txt", 0.15, b" "),
                (b"Staged", 0.15, b"\t\x1b[200~" + message.encode() + b"\x1b[201~"),
                (message.encode(), 0.15, mouse),
                (b"Committed", 0.2, b"\x11"),
            ]
            result = terminal.run_case(root, base / f"config-{width}", (width, height), actions,
                                       expected=[b"Note.txt", b"Committed"], env_overrides=environment)
            assert git(root, "rev-parse", "HEAD") != before
            assert git(root, "log", "-1", "--format=%B") == message
            assert git(root, "rev-list", "--count", "HEAD") == "2", "mouse submitted more than once"
            assert git(root, "status", "--porcelain") == ""
            assert subprocess.check_output(["git", "-C", str(root), "show", "HEAD:Note.txt"]) == b"changed\r\n"
            assert path.read_bytes() == b"changed\r\n"
            assert not (root / ".git/index.lock").exists()
            cases.append({"case": "stage-shared-message-mouse-commit", "exact_crlf": True, **result})
        root = base / "no-repository"
        root.mkdir()
        result = terminal.run_case(root, base / "init-config", (60, 18), [
            (b"\x1b[?1049h", 0.2, b"\x1b3"),
            (b"Not a Git", 0.15, b"i"),
            (b"Initialized", 0.2, b"\x11"),
        ], expected=[b"Not a Git", b"Initialized"], env_overrides=environment)
        assert (root / ".git/HEAD").is_file()
        assert not (root / ".git/index.lock").exists()
        cases.append({"case": "missing-repository-initialize", **result})
        assert not (base / "no-installs").exists()
    (ROOT / "docs/parity/G02-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} Git CLI/PTY checks")


if __name__ == "__main__":
    main()
