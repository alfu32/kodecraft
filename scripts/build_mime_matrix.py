#!/usr/bin/env python3
"""Capture source sample filename expectations without copying sample contents."""
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
table = json.loads((ROOT / "resources/mime-types.json").read_text())
extensions = {entry["extension"].removeprefix(".").lower(): entry for entry in table}
rows = []
source = ROOT.parent / "kode"
paths = subprocess.check_output(["git", "-C", str(source), "ls-files", "samples"], text=True).splitlines()
for path in sorted(source / relative for relative in paths):
    if not path.is_file():
        continue
    name = path.name
    suffix = name.rsplit(".", 1)[-1].lower() if "." in name else ""
    entry = extensions.get(suffix)
    if entry:
        rows.append({"filename": name, **entry})
    else:
        rows.append({"filename": name, "mime": "application/octet-stream", "extension": f".{suffix}" if suffix else "", "language": None, "category": "BINARY"})
target = ROOT / "fixtures/mime/sample-matrix.json"
target.parent.mkdir(parents=True, exist_ok=True)
target.write_text(json.dumps({"baseline": "7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68", "derivation": "Kotlin detectFilename last suffix; SampleMimeTable last row wins", "samples": rows}, indent=2) + "\n")
print(f"Recorded {len(rows)} sample filename mappings; no sample contents copied")
