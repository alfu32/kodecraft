#!/usr/bin/env python3
"""Build a portable Kode archive from a native binary and the platform-neutral JDBC jar."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    windows = "windows" in args.target
    binary_name = "kode.exe" if windows else "kode"
    files = {}

    def add(name, path, executable=False):
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"Missing or symlinked package input: {path}")
        files[name] = (path.read_bytes(), executable)

    add(binary_name, args.binary, not windows)
    add("jdbc/kode-jdbc-bridge.jar", args.helper)
    for path in sorted((ROOT / "resources").rglob("*")):
        if path.is_file() or path.is_symlink():
            add(path.relative_to(ROOT / "resources").as_posix(), path)
    for name in ("LICENSE.md", "README.md", "packaging/README.md"):
        add(name, ROOT / name)
    for name in ("kode.sh", "kode.cmd", "kode.bat", "kode.ps1"):
        add(name, ROOT / "packaging/launchers" / name, name == "kode.sh")
    manifest = {
        "schemaVersion": 1,
        "target": args.target,
        "version": version,
        "tag": version,
        "sourceRevision": os.environ.get("KODE_BUILD_SHA") or subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "javaRuntime": "external; JDBC adapter jar included",
        "browserTerminalBundled": False,
        "files": {
            name: {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data), "executable": executable}
            for name, (data, executable) in sorted(files.items())
        },
    }
    files["package-manifest.json"] = ((json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode(), False)
    name = f"kode-{version}-{args.target}"
    args.output.mkdir(parents=True, exist_ok=True)
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "1767225600"))
    archive = args.output / (name + ".tar.gz")
    with archive.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=epoch) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as output:
            for relative, (data, executable) in sorted(files.items()):
                info = tarfile.TarInfo(f"{name}/{relative}")
                info.size = len(data)
                info.mtime = epoch
                info.mode = 0o755 if executable else 0o644
                output.addfile(info, io.BytesIO(data))
    zip_path = args.output / (name + ".zip")
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as output:
        for relative, (data, executable) in sorted(files.items()):
            info = zipfile.ZipInfo(f"{name}/{relative}")
            info.date_time = (2026, 1, 1, 0, 0, 0)
            info.external_attr = ((0o100755 if executable else 0o100644) << 16)
            info.compress_type = zipfile.ZIP_DEFLATED
            output.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
    for path in (archive, zip_path):
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        (args.output / (path.name + ".sha256")).write_text(f"{digest}  {path.name}\n")
        print(f"{path}: {digest}")


if __name__ == "__main__":
    main()
