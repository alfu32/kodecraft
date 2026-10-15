#!/usr/bin/env python3
"""Exercise actual ANSI highlight output and edits in synthetic CLI projects."""
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
    with tempfile.TemporaryDirectory(prefix="kode-highlights-pty-") as directory:
        base = Path(directory)
        for size in [(100, 30), (60, 18)]:
            root = base / f"project-{size[0]}"
            root.mkdir()
            path = root / "Highlights.kt"
            source = "// naïve — 😀\r\nclass Customer {\r\n\tval id: Long = 42;\r\n}\r\n"
            path.write_bytes(source.encode())
            prefix = "// inserted 😀\n// another line\n"
            colors = [b"38;2;232;191;106", b"38;2;165;194;97", b"38;2;104;151;187"]
            result = terminal.run_case(path, base / f"config-{size[0]}", size, [
                # Native property color can appear only after background analysis.
                (colors[1], 0.1, b"\x1b[200~" + prefix.encode() + b"\x1b[201~"),
                (b"TreeSitter", 0.2, b"\x13"),
                (b"TreeSitter", 0.2, b"\x11"),
            ], expected=[b"TreeSitter", b"|-->", *colors])
            assert path.read_bytes() == (prefix + source).encode(), "highlight remapping changed saved source bytes"
            cases.append({"case": "native-lexical-semantic-colors-and-utf8-edit-remap", "truecolor_classes": ["class", "property", "number"], **result})
    output = {"cases": cases, "passed": len(cases)}
    (ROOT / "docs/parity/E11-pty.json").write_text(json.dumps(output, indent=2) + "\n")
    print(f"Passed {len(cases)} highlight CLI/PTY checks")


if __name__ == "__main__":
    main()
