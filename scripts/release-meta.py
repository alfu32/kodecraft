#!/usr/bin/env python3
"""Resolve the checked-out manual release source and reject conflicting tags."""
import os
from pathlib import Path
import re
import subprocess
import tomllib

version = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", version):
    raise SystemExit(f"Invalid release version: {version}")
tag = f"v{version}"
sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
source_ref = os.environ.get("SOURCE_REF", "")
if source_ref.startswith("refs/tags/") and source_ref.removeprefix("refs/tags/") != tag:
    raise SystemExit(f"Selected tag {source_ref} differs from Cargo version {tag}")
if source_ref.startswith("v") and not source_ref.startswith("refs/") and source_ref != tag:
    raise SystemExit(f"Selected tag {source_ref} differs from Cargo version {tag}")
existing = subprocess.run(["git", "rev-parse", f"refs/tags/{tag}^{{commit}}"], text=True, capture_output=True)
if existing.returncode == 0 and existing.stdout.strip() != sha:
    raise SystemExit(f"Release tag {tag} identifies a different commit")
for key, value in (("version", version), ("tag", tag), ("sha", sha)):
    print(f"{key}={value}")
