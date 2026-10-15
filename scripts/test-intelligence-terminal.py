#!/usr/bin/env python3
"""Qualify native completion through the real CLI using only synthetic projects."""
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("app_terminal", ROOT / "scripts/test-app-terminal.py")
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-intelligence-pty-") as directory:
        base = Path(directory)
        for size in [(100, 30), (60, 18)]:
            root = base / f"project-{size[0]}"
            root.mkdir()
            path = root / "Native.kt"
            source = "class Customer { val id: Long = 0; val name: String = \"\" }\nfun inspect(customer: Customer) {\n    customer.\n}\n"
            path.write_text(source)
            result = terminal.run_case(path, base / f"config-{size[0]}", size, [
                # Down twice from the fresh file caret, End, Ctrl+Space.
                (b"Ready", 0.1, b"\x1b[B\x1b[B\x1b[F\x00"),
                (b"member", 0.1, b"\r"),
                (b"\x1b[?1049h", 0.1, b"\x13"),
                (b"\x1b[?1049h", 0.2, b"\x11"),
            ], expected=[b"member"])
            assert path.read_text() == source.replace("customer.\n", "customer.id\n"), "native completion was not inserted/saved exactly"
            assert (root / ".kode-rs/index.sqlite").is_file(), "native SQLite cache missing"
            cases.append({"case": "native-completion-insert-save-and-shutdown", **result})
    output = {"cases": cases, "passed": len(cases)}
    (ROOT / "docs/parity/L09-pty.json").write_text(json.dumps(output, indent=2) + "\n")
    print(f"Passed {len(cases)} native intelligence CLI/PTY checks")


if __name__ == "__main__":
    main()
