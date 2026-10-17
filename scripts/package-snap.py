#!/usr/bin/env python3
"""Build a real strict core24 Snap from the verified no-JVM native payload."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from distribution_notices import ROOT, sha
from installer_support import payload, publish


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive",type=Path,default=ROOT / "target/distribution-z03/kode-0.1.0-x86_64-unknown-linux-gnu.tar.gz")
    parser.add_argument("--packer",choices=("snapcraft","snap"),default="snapcraft",help="snap pack also works in CI without a host snapd service")
    parser.add_argument("--output",type=Path,default=ROOT / "target/installers")
    args = parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kode-snap-stage-") as directory:
        stage = Path(directory)
        package, manifest = payload(args.archive,stage / "input")
        snap_arch = "arm64" if manifest["target"].startswith("aarch64") else "amd64"
        root = stage / "snap"
        (root / "opt").mkdir(parents=True)
        shutil.copytree(package,root / "opt/kode")
        (root / "bin").mkdir()
        shutil.copyfile(ROOT / "packaging/launchers/kode-snap",root / "bin/kode")
        (root / "bin/kode").chmod(0o755)
        (root / "meta/gui").mkdir(parents=True)
        shutil.copyfile(ROOT / "packaging/kode.svg",root / "meta/gui/icon.svg")
        version = manifest["version"]
        # Snapd supplies HOME. Keep Kode's mutable state in the revision-neutral
        # app data area and preserve the source Snap's ordinary interfaces.
        metadata = f'''name: kode-rs
version: '{version}'
summary: Terminal-first native code editor
description: |
  Native Kode editor with Git, REST and external-Java JDBC.
  Java is downloaded separately when explicitly requested.
base: core24
architectures: [{snap_arch}]
confinement: strict
grade: devel
license: MIT
apps:
  kode-rs:
    command: bin/kode
    environment:
      XDG_CONFIG_HOME: $SNAP_USER_COMMON/.config
      XDG_CACHE_HOME: $SNAP_USER_COMMON/.cache
      KODE_RS_DATABASE_HOME: $SNAP_USER_COMMON/database
      KODE_RS_JDBC_HOME: $SNAP_USER_COMMON/plugins
      KODE_LSP_HOME: $SNAP_USER_COMMON/lsp
    plugs: [home, removable-media, network, network-bind, desktop, desktop-legacy, x11, wayland]
'''
        (root / "meta/snap.yaml").write_text(metadata)
        epoch = int(os.environ.get("SOURCE_DATE_EPOCH","1767225600"))
        for path in sorted(root.rglob("*"),reverse=True):
            os.utime(path,(epoch,epoch),follow_symlinks=False)
        os.utime(root,(epoch,epoch))
        command = ["snapcraft","pack",str(root),"--output",str(args.output.resolve())] if args.packer=="snapcraft" else ["snap","pack",str(root),str(args.output.resolve()),"--compression=xz"]
        result = subprocess.run(command,check=True,cwd=stage,capture_output=True,text=True,env=dict(os.environ,SOURCE_DATE_EPOCH=str(epoch)))
        artifact = args.output / f"kode-rs_{version}_{snap_arch}.snap"
        if not artifact.is_file():
            raise ValueError("Snapcraft did not produce the expected artifact: " + result.stdout[-1000:])
        publish(artifact,{"format":"snap","target":manifest["target"],"base":"core24","confinement":"strict","grade":"devel","javaRuntimeBundled":False,"payloadArchiveSha256":sha(args.archive.read_bytes()),"payloadManifestSha256":sha((package / "package-manifest.json").read_bytes()),"packer":args.packer,"packerVersion":subprocess.check_output([args.packer,"--version"],text=True).strip()})


if __name__ == "__main__":
    main()
