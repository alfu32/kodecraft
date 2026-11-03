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
        # URLs stay in block lists: the Store's older YAML parser rejects a colon
        # inside an unquoted [flow, list], though snap pack and current PyYAML accept it.
        metadata = f'''name: kode-rs
title: Kode
version: '{version}'
summary: A full code editor and IDE that runs in your terminal
description: |
  Kode is a full code editor and IDE that runs in your terminal. Open a project
  locally or over SSH and get a mouse-aware editor with code intelligence, search,
  Git, database and REST clients, plus image and hex viewers, all in one window.

  Understands your code without any setup. Highlighting, completion, go to
  definition and find usages work straight away, across files and including your
  unsaved edits. Analysis is deepest for Kotlin and C, followed by C++, Go,
  JavaScript, TypeScript, Python, PHP, Ruby, Bash, Swift, C#, Lua, SQL, Zig and
  more. Java, Rust and others get pattern-based symbols, and more than 200 other
  languages and file types get keyword highlighting. Add a language server for
  compiler-grade results.

  A real editor. Multiple cursors and selections, word-by-word navigation,
  folding, a minimap, undo and redo, and full mouse support. Turn every search
  match into a cursor with Alt+Enter.

  Find anything. Press Ctrl+F to find and replace in the current window with
  regular expressions. Press Alt+F to search the whole project and edit the
  results right inside the search dialog.

  Git. Stage changes, write commits, switch local branches, create tags, browse
  history, read side-by-side or unified diffs, and restore a single chunk.

  Databases. Run SQL and browse schemas on anything with a JDBC driver. Database
  connections need Java 21 or newer; everything else works without Java.

  REST client. Import and export Postman collections, import OpenAPI, and send
  HTTP, Server-Sent Events and WebSocket requests with OAuth, AWS Signature, NTLM
  and other authentication.

  Images and binary files. View images in the terminal, and inspect binary files
  in a hex view. Byte edits stay in memory, because binary files can't be saved yet.

  Picks up where you left off. Open files, undo history and even unsaved changes
  survive a restart.

  Run it with: kode-rs /path/to/project
links:
  website:
    - https://github.com/alfu32/kodecraft
  source-code:
    - https://github.com/alfu32/kodecraft
  issues:
    - https://github.com/alfu32/kodecraft/issues
base: core24
architectures: [{snap_arch}]
confinement: strict
grade: stable
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
        publish(artifact,{"format":"snap","target":manifest["target"],"base":"core24","confinement":"strict","grade":"stable","javaRuntimeBundled":False,"payloadArchiveSha256":sha(args.archive.read_bytes()),"payloadManifestSha256":sha((package / "package-manifest.json").read_bytes()),"packer":args.packer,"packerVersion":subprocess.check_output([args.packer,"--version"],text=True).strip()})


if __name__ == "__main__":
    main()
