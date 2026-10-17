#!/usr/bin/env python3
"""Require all six native targets and installer formats before GitHub publication."""
import argparse
import hashlib
import json
from pathlib import Path
import tarfile
import zipfile

TARGETS = {
    "x86_64-unknown-linux-gnu": ("x86_64.AppImage", "amd64.snap"),
    "aarch64-unknown-linux-gnu": ("aarch64.AppImage", "arm64.snap"),
    "x86_64-pc-windows-msvc": ("windows-x64.msi", "windows-x64.msix"),
    "aarch64-pc-windows-msvc": ("windows-arm64.msi", "windows-arm64.msix"),
    "x86_64-apple-darwin": ("macos-x86_64.dmg",),
    "aarch64-apple-darwin": ("macos-aarch64.dmg",),
}


def verify_portable_manifest(data, root, version, tag, sha):
    manifest = json.loads(data)
    if (manifest.get("version"), manifest.get("tag"), manifest.get("sourceRevision"), manifest.get("target")) != (version, tag, sha, root.removeprefix(f"kode-{version}-")):
        raise ValueError(f"Portable archive metadata differs from release source: {root}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sha", required=True)
    args = parser.parse_args()
    if args.tag != args.version:
        raise ValueError("Release tag and package version differ")
    expected = {f"kode-jdbc-bridge-{args.version}.jar"}
    for target, installers in TARGETS.items():
        base = f"kode-{args.version}-{target}"
        expected.update((base + ".zip", base + ".tar.gz", f"kode-jdbc-runtime-{target}.zip"))
        for suffix in installers:
            if suffix.endswith(".snap"):
                expected.add(f"kode-rs_{args.version}_{suffix}")
            else:
                expected.add(f"kode-{args.version}-{suffix}")
    actual = {path.name for path in args.directory.iterdir() if path.is_file() and not path.name.endswith((".sha256", ".manifest.json"))}
    if actual != expected:
        raise ValueError(f"Release asset matrix differs: missing={sorted(expected-actual)}, unexpected={sorted(actual-expected)}")
    checksums = []
    for name in sorted(expected):
        path = args.directory / name
        if path.stat().st_size == 0:
            raise ValueError(f"Empty release asset: {name}")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        sidecar = args.directory / (name + ".sha256")
        if sidecar.exists() and sidecar.read_text().strip() != f"{digest}  {name}":
            raise ValueError(f"Invalid checksum sidecar: {name}")
        checksums.append(f"{digest}  {name}")
        if name.endswith(".tar.gz"):
            with tarfile.open(path) as archive:
                members = {member.name for member in archive.getmembers()}
                root = name.removesuffix(".tar.gz")
                if root + "/jdbc/kode-jdbc-bridge.jar" not in members or root + "/package-manifest.json" not in members:
                    raise ValueError(f"Portable archive lacks JDBC adapter or manifest: {name}")
                verify_portable_manifest(archive.extractfile(root + "/package-manifest.json").read(), root, args.version, args.tag, args.sha)
        if name.endswith(".zip") and name.startswith("kode-jdbc-runtime-"):
            with zipfile.ZipFile(path) as archive:
                if not any(member.endswith("/bin/java") or member.endswith("/bin/java.exe") for member in archive.namelist()):
                    raise ValueError(f"Java runtime archive lacks its executable: {name}")
        elif name.endswith(".zip"):
            with zipfile.ZipFile(path) as archive:
                members = set(archive.namelist())
                root = name.removesuffix(".zip")
                if root + "/jdbc/kode-jdbc-bridge.jar" not in members or root + "/package-manifest.json" not in members:
                    raise ValueError(f"Portable archive lacks JDBC adapter or manifest: {name}")
                verify_portable_manifest(archive.read(root + "/package-manifest.json"), root, args.version, args.tag, args.sha)
    Path("target").mkdir(parents=True, exist_ok=True)
    Path("target/SHA256SUMS.txt").write_text("\n".join(checksums) + "\n")
    Path("target/release-notes.md").write_text(
        f"Kode {args.version}\n\nSource: `{args.sha}`.\n\n"
        "Portable archives for Linux, Windows and macOS on x86_64 and arm64; "
        "AppImage and Snap for both Linux architectures; MSI and MSIX for both Windows architectures; "
        "DMG for both macOS architectures. The JDBC adapter jar is available separately and inside "
        "each portable archive. Optional per-platform Java 21+ runtimes are separate assets.\n\n"
        "Windows MSI/MSIX and macOS DMG packages are unsigned; platform trust settings may be required.\n"
    )
    print(f"Verified {len(expected)} release assets for {args.tag}")


if __name__ == "__main__":
    main()
