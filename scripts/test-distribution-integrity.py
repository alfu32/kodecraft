#!/usr/bin/env python3
"""Actual package attempts with stale/corrupt owned inputs must publish no archive."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from distribution_notices import ROOT, sha


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-package-integrity-") as directory:
        base = Path(directory)
        notices = base / "notices"
        shutil.copytree(ROOT / "target/distribution-notices", notices)
        manifest = json.loads((notices / "manifest.json").read_text())
        selected = next(name for name in manifest["files"] if name.endswith("LICENSE") or name.endswith("LICENSE-MIT"))
        path = notices / selected
        path.write_bytes(path.read_bytes() + b"\nsynthetic corruption\n")
        native = base / "native"
        shutil.copytree(ROOT / "target/native-release-bundle",native)
        library = native / "lib/libjson-c.so.5"
        library.write_bytes(library.read_bytes() + b"synthetic corruption")
        helper = base / "helper.jar"
        helper.write_bytes((ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar").read_bytes() + b"synthetic stale helper")
        for name, arguments, expected in [("corrupt-full-license",["--notices",str(notices)],"Distribution notice content does not match its manifest"),("corrupt-native-library",["--native-bundle",str(native)],"Native bundle content differs from its manifest"),("stale-helper-notices",["--helper",str(helper)],"Notice inputs differ from the current lock/helper")]:
            output = base / name
            result = subprocess.run([sys.executable,str(ROOT / "scripts/package-linux.py"),"--output",str(output),*arguments],cwd=ROOT,capture_output=True,text=True)
            assert result.returncode!=0 and expected in result.stderr, (name,result.stderr[-1000:])
            assert not list(output.glob("*.tar.gz")), name
            cases.append({"case":name,"status":"passed","archivePublished":False})
    (ROOT / "docs/parity/Z03-integrity.json").write_text(json.dumps({"cases":cases,"passed":len(cases)},indent=2)+"\n")
    print(f"Passed {len(cases)} distribution integrity cases")


if __name__ == "__main__":
    main()
