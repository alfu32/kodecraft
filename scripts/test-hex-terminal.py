#!/usr/bin/env python3
"""Qualify the real hex route and local byte clipboard in synthetic PTYs."""
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("app_pty", ROOT / "scripts/test-app-terminal.py")
app = importlib.util.module_from_spec(spec)
spec.loader.exec_module(app)
cases = []
with tempfile.TemporaryDirectory(prefix="kode-hex-pty-") as directory:
    base = Path(directory)
    for size in [(120, 30), (20, 6), (1, 1)]:
        root = base / f"project-{size[0]}"
        root.mkdir()
        file = root / "bytes.bin"
        original = bytes(range(256))
        file.write_bytes(original)
        entered = b"\x1b[?1049h"
        trigger = b"00 01 02" if size[0] == 120 else entered
        case = app.run_case(file, base / "config", size, [
            (trigger, 0.3, b"\x01"),
            (entered, 0.1, b"\x1bx"),
            (entered, 0.1, b"\x1bv"),
            (entered, 0.1, b"\x1b[6~\x1b[F"),
            (entered, 0.1, b"\x11"),
        ])
        assert file.read_bytes() == original, "temporary byte edits wrote the physical file"
        state = json.loads((root / ".kode-rs.json").read_text())
        assert state["recentFiles"][0]["viewerType"] == "HEX"
        cases.append({"case": "binary-local-cut-paste-quit", **case})
(ROOT / "docs/parity/V02-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
print(f"Passed {len(cases)} real hex application PTY checks")
