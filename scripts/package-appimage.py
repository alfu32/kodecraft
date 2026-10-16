#!/usr/bin/env python3
"""Build a real no-JVM AppImage from the verified Linux archive and pinned runtime."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from distribution_notices import ROOT, sha
from installer_support import download, payload, publish


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive",type=Path,default=ROOT / "target/distribution-z03/kode-0.1.0-x86_64-unknown-linux-gnu.tar.gz")
    parser.add_argument("--output",type=Path,default=ROOT / "target/installers")
    parser.add_argument("--cache",type=Path,default=ROOT / "target/installer-tools")
    args = parser.parse_args()
    inputs = json.loads((ROOT / "packaging/appimage-inputs.json").read_text())
    tool = download(inputs["tool"]["url"],inputs["tool"]["sha256"],args.cache / "appimagetool-1.9.1-x86_64.AppImage",32*1024*1024)
    runtime = download(inputs["runtime"]["url"],inputs["runtime"]["sha256"],args.cache / "runtime-20251108-x86_64",2*1024*1024)
    tool.chmod(0o755)
    args.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kode-appimage-stage-") as directory:
        stage = Path(directory)
        package, manifest = payload(args.archive,stage / "input")
        app = stage / "Kode.AppDir"
        (app / "usr/lib").mkdir(parents=True)
        shutil.copytree(package,app / "usr/lib/kode")
        shutil.copyfile(ROOT / "packaging/launchers/AppRun",app / "AppRun")
        (app / "AppRun").chmod(0o755)
        shutil.copyfile(ROOT / "packaging/kode.desktop",app / "kode.desktop")
        shutil.copyfile(ROOT / "packaging/kode.svg",app / "kode.svg")
        shutil.copyfile(ROOT / "packaging/kode.svg",app / ".DirIcon")
        subprocess.run(["desktop-file-validate",str(app / "kode.desktop")],check=True)
        for entry in inputs["notices"]:
            source = ROOT / "packaging" / entry["file"]
            if sha(source.read_bytes())!=entry["sha256"]:
                raise ValueError("Pinned AppImage runtime notice changed")
            target = app / "usr/lib/kode/notices/appimage" / source.relative_to(ROOT / "packaging/licenses/appimage-runtime")
            target.parent.mkdir(parents=True,exist_ok=True)
            target.write_bytes(source.read_bytes())
        (app / "usr/lib/kode/notices/appimage/manifest.json").write_text(json.dumps(inputs,sort_keys=True,indent=2)+"\n")
        epoch = int(os.environ.get("SOURCE_DATE_EPOCH","1767225600"))
        for path in sorted(app.rglob("*"),reverse=True):
            os.utime(path,(epoch,epoch),follow_symlinks=False)
        os.utime(app,(epoch,epoch))
        artifact = args.output / f"kode-{manifest['version']}-x86_64.AppImage"
        environment = dict(os.environ,ARCH="x86_64",SOURCE_DATE_EPOCH=str(epoch),APPIMAGE_EXTRACT_AND_RUN="1")
        command = [str(tool.resolve()),"--runtime-file",str(runtime.resolve()),"--no-appstream","--comp","zstd","--mksquashfs-opt=-processors","--mksquashfs-opt=2",str(app),str(artifact.resolve())]
        result = subprocess.run(command,cwd=stage,env=environment,capture_output=True,text=True)
        if result.returncode:
            raise RuntimeError("AppImage assembly failed:\n"+result.stdout[-3000:]+result.stderr[-3000:])
        artifact.chmod(0o755)
        publish(artifact,{"format":"appimage","target":manifest["target"],"minimumGlibc":manifest["minimumGlibc"],"javaRuntimeBundled":False,"payloadArchiveSha256":sha(args.archive.read_bytes()),"payloadManifestSha256":sha((package / "package-manifest.json").read_bytes()),"toolVersion":inputs["tool"]["version"],"toolSha256":inputs["tool"]["sha256"],"runtimeVersion":inputs["runtime"]["version"],"runtimeSha256":inputs["runtime"]["sha256"],"sourceDateEpoch":epoch})


if __name__ == "__main__":
    main()
