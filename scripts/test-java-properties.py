#!/usr/bin/env python3
"""Verify the synthetic Latin-1/escape fixture against actual java.util.Properties."""
import base64
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
fixture = json.loads((root / "fixtures/database/java-properties.json").read_text())
payload = base64.b64encode(fixture["inputLatin1"].encode("latin-1")).decode()
output = subprocess.check_output(["java", str(root / "fixtures/database/PropertiesOracle.java"), payload], text=True)
actual = {}
for line in output.splitlines():
    key, value = line.split("\t")
    actual[base64.b64decode(key).decode()] = base64.b64decode(value).decode()
assert actual == fixture["expected"], "Java properties fixture differs from actual JDK behavior"
print(f"Java Properties oracle verified {len(actual)} synthetic properties")
