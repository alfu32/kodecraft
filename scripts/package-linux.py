#!/usr/bin/env python3
"""Assemble a deterministic Linux application archive from built native/helper artifacts.

Build inputs first with cargo build --release --locked and Maven package. This
assembler never copies a JDK/JVM or a developer's driver/configuration directory.
Z03 owns complete release CI, native dependency bundling, installers and notices.
"""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import tarfile
import tomllib
from distribution_notices import safe, sha
from native_bundle import glibc

ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-unknown-linux-gnu"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/kode")
    parser.add_argument("--helper", type=Path, default=ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar")
    parser.add_argument("--output", type=Path, default=ROOT / "target/distribution")
    parser.add_argument("--notices", type=Path, default=ROOT / "target/distribution-notices")
    parser.add_argument("--native-bundle", type=Path, help="Verified standalone ttyd with its relative library closure and notices")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with binary.open("rb") as source:
        header = source.read(64)
    if header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != 62:
        raise SystemExit("Only an actual Linux x86_64 ELF binary is qualified by this assembler")
    dynamic = subprocess.check_output(["readelf", "-d", str(binary)], text=True)
    interpreter = subprocess.check_output(["readelf", "-l", str(binary)], text=True)
    if "ld-linux-x86-64.so.2" not in interpreter:
        raise SystemExit("This assembler requires the verified Linux x86_64 GNU target")
    files = {}

    def add(name, path, role, executable=False):
        if path.is_symlink() or not path.is_file():
            raise SystemExit(f"Package input must be an ordinary file: {path}")
        if name in files:
            raise ValueError(f"Duplicate package payload path: {name}")
        data = path.read_bytes()
        if data.startswith(b"\x7fELF") and str(Path.home()).encode() in data:
            raise ValueError("Native build contains developer source paths; build with scripts/build-release.py")
        files[name] = (data, role, executable)

    add("kode", binary, "native-application", True)
    add("jdbc/kode-jdbc-bridge.jar", args.helper, "external-java-helper")
    for path in sorted((ROOT / "resources").rglob("*")):
        if path.is_file():
            add(path.relative_to(ROOT / "resources").as_posix(), path, "resource")
        elif path.is_symlink():
            raise SystemExit(f"Resource symlinks are not package inputs: {path}")
    add("LICENSE.md", ROOT / "LICENSE.md", "license")
    add("README.md", ROOT / "README.md", "documentation")
    add("packaging/README.md", ROOT / "packaging/README.md", "documentation")
    for launcher in sorted((ROOT / "packaging/launchers").iterdir()):
        add(launcher.name, launcher, "launcher", launcher.suffix == ".sh")
    notices = json.loads((args.notices / "manifest.json").read_text())
    if notices["target"] != TARGET or notices["cargoLockSha256"] != sha((ROOT / "Cargo.lock").read_bytes()) or notices["helperSha256"] != sha(args.helper.read_bytes()):
        raise ValueError("Notice inputs differ from the current lock/helper; regenerate distribution notices")
    for name, entry in notices["files"].items():
        path = args.notices / safe(name)
        if sha(path.read_bytes()) != entry["sha256"] or path.stat().st_size != entry["size"]:
            raise ValueError("Distribution notice content does not match its manifest")
        add("notices/" + name, path, "license-notice")
    add("notices/manifest.json", args.notices / "manifest.json", "notice-manifest")
    native = None
    if args.native_bundle:
        native = json.loads((args.native_bundle / "native-manifest.json").read_text())
        if native["target"] != TARGET:
            raise ValueError("Native bundle target differs from application target")
        for name, entry in native["files"].items():
            path = args.native_bundle / safe(name)
            if sha(path.read_bytes()) != entry["sha256"] or path.stat().st_size != entry["size"]:
                raise ValueError("Native bundle content differs from its manifest")
            add(name, path, "native-browser" if not name.startswith("notices/") else "license-notice", entry["executable"])
        add("native-manifest.json", args.native_bundle / "native-manifest.json", "native-manifest")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    manifest = {"schemaVersion": 1, "target": TARGET, "version": version,
                "javaRuntime": "external; optional separately downloaded runtime",
                "minimumGlibc": max([glibc(binary)] + ([native["minimumGlibc"]] if native else []), key=lambda version:tuple(map(int,version.split('.')))),
                "browserTerminalBundled": native is not None,
                "nativeDependencies": sorted(set(re.findall(r"Shared library: \[(.*?)\]", dynamic))),
                "files": {name: {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data), "role": role, "executable": executable}
                          for name, (data, role, executable) in sorted(files.items())}}
    encoded = (json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode()
    files["package-manifest.json"] = (encoded, "manifest", False)
    # An explicit allowlist makes runtime separation reviewable and fail-closed.
    assert not any(name.startswith(("runtime/", "jre/", "jvm/")) or name.endswith(("libjvm.so", "/lib/modules")) for name in files)
    args.output.mkdir(parents=True, exist_ok=True)
    name = f"kode-{version}-{TARGET}"
    archive = args.output / (name + ".tar.gz")
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "1767225600"))
    with archive.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=epoch) as compressed, tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as output:
        for relative, (data, _, executable) in sorted(files.items()):
            info = tarfile.TarInfo(name + "/" + relative)
            info.size = len(data)
            info.mode = 0o755 if executable else 0o644
            info.mtime = epoch
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            output.addfile(info, io.BytesIO(data))
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    (args.output / (archive.name + ".sha256")).write_text(f"{checksum}  {archive.name}\n")
    (args.output / (name + ".manifest.json")).write_bytes(encoded)
    print(json.dumps({"archive": str(archive), "sha256": checksum, "compressedBytes": archive.stat().st_size, "files": len(files), "javaRuntimeBundled": False, "nativeDependencies": manifest["nativeDependencies"]}))


if __name__ == "__main__":
    main()
