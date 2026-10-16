#!/usr/bin/env python3
"""Real read-only diff/restore/context/viewer switching through isolated CLI PTYs."""
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
    return subprocess.check_output(["git", "-C", str(root), *args])


def retained_diff(screen):
    right="\n".join("".join(row)[37:] for row in screen.cells)
    return b"\x1b[17~\x1b[200~forbidden\x1b[201~" if "Diff" in right else None


def left_for_rest(screen):
    right="\n".join("".join(row)[37:] for row in screen.cells)
    left="\n".join("".join(row)[:36] for row in screen.cells)
    return b"\x11" if "REST API" in left and "Diff" not in right else None


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-git-diff-pty-") as directory:
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
            old = b"first\r\ncontext\r\ncontext2\r\nlast\r\n"
            new = b"FIRST\r\ncontext\r\ncontext2\r\nLAST\r\n"
            path.write_bytes(old)
            git(root, "add", ".gitignore", "Note.txt")
            git(root, "-c", "core.hooksPath=/dev/null", "commit", "-m", "baseline")
            head = git(root, "rev-parse", "HEAD")
            index = (root / ".git/index").read_bytes()
            path.write_bytes(new)
            entered = b"\x1b[?1049h"
            result = terminal.run_case(path, base / f"config-{width}", (width, height), [
                (b"Ready", 0.15, b"\x1b3"),
                (b"Note.txt", 0.15, b"\r"),
                (b"Diff", 0.15, b"\x1b3"),
                (entered, 0.15, retained_diff),
                (entered, 0.15, b"c"),
                (entered, 0.2, b"c"),
                (entered, 0.2, b"\r"),
                (b"Restored", 0.2, b"v"),
                (entered, 0.2, b"\x1b5"),
                (entered, 0.2, left_for_rest),
            ], expected=[b"Diff", b"Restored"], env_overrides=environment)
            assert path.read_bytes() == b"first\r\ncontext\r\ncontext2\r\nLAST\r\n", "restore changed an unselected chunk or line endings"
            assert git(root, "rev-parse", "HEAD") == head
            assert (root / ".git/index").read_bytes() == index
            assert not (root / ".git/index.lock").exists()
            cases.append({"case": "readonly-context-selected-restore-unified-leave", "exact_crlf": True, **result})
    (ROOT / "docs/parity/G03-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} Git diff CLI/PTY checks")


if __name__ == "__main__":
    main()
