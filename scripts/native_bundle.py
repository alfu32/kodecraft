#!/usr/bin/env python3
"""Bundle the verified Linux ttyd and its ELF/plugin closure with relative RPATHs."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from distribution_notices import ROOT, sha

# The host provides glibc/the dynamic loader. Other ttyd dependencies are copied.
SYSTEM = {"libc.so.6", "libm.so.6", "libpthread.so.0", "libdl.so.2", "librt.so.1", "ld-linux-x86-64.so.2"}
REVISION = "a8ae135643c0ed7164f59f14f537b0b11f7a9788"
SOURCE_SHA = "5b3092f1de2c9e472cf85ea32ae8bab3d91f04945134429478224dea13968524"


def needed(path):
    dynamic = subprocess.check_output(["readelf", "-d", str(path)], text=True)
    return sorted(set(re.findall(r"Shared library: \[(.*?)\]", dynamic)))


def glibc(path):
    output = subprocess.check_output(["readelf", "--version-info", str(path)], text=True)
    versions = set(re.findall(r"GLIBC_(\d+\.\d+)", output))
    return max(versions, key=lambda v:tuple(map(int,v.split('.'))), default=None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ttyd-bundle", type=Path, required=True)
    parser.add_argument("--library-dir", type=Path, action="append", default=[])
    parser.add_argument("--output", type=Path, default=ROOT / "target/native-bundle")
    args = parser.parse_args()
    provenance = json.loads((args.ttyd_bundle / "ttyd-manifest.json").read_text())
    original = args.ttyd_bundle / "bin/ttyd"
    if provenance["source_revision"] != REVISION or provenance["source_sha256"] != SOURCE_SHA or provenance["executable_sha256"] != sha(original.read_bytes()):
        raise ValueError("ttyd provenance does not match the verified source/executable")
    if provenance["host"] != {"os":"Linux", "architecture":"x86_64"} or provenance["jni"] or provenance["shared_launcher"]:
        raise ValueError("Only the qualified standalone Linux x86_64 ttyd is supported")
    if provenance.get("event_loop") != "built-in-libuv":
        raise ValueError("Release ttyd must use built-in libuv; system event plugins are not relocatable")
    cache = subprocess.check_output(["ldconfig", "-p"], text=True)
    system_paths = {}
    for line in cache.splitlines():
        match = re.match(r"\s*(\S+) \(.*x86-64.*\) => (.*)", line)
        if match:
            system_paths.setdefault(match[1], Path(match[2]))

    def resolve(name):
        if not re.fullmatch(r"[A-Za-z0-9_.+-]+", name):
            raise ValueError("Unsafe ELF library name")
        for directory in args.library_dir:
            path = directory / name
            if path.is_file():
                return path.resolve()
        if name in system_paths:
            return system_paths[name].resolve()
        raise ValueError(f"Missing native runtime dependency: {name}")

    files = {}
    elf = []
    external = set()
    docs = {"libwebsockets":"static", "libuv":"static"}

    def add(name, data, executable=False):
        if name in files and files[name][0] != data:
            raise ValueError(f"Conflicting native file: {name}")
        files[name] = (data, executable)

    def copyright_for(path):
        # Custom .deb prefixes retain usr/share/doc; system libraries use dpkg.
        for directory in args.library_dir:
            if not path.is_relative_to(directory.resolve().parent):
                continue
            root = directory
            while root.name != "usr" and root != root.parent:
                root = root.parent
            if root.name == "usr":
                mapping = {"libwebsockets.so.19":"libwebsockets19t64", "libwebsockets-evlib_uv.so":"libwebsockets-evlib-uv", "libuv.so.1":"libuv1t64"}
                package = mapping.get(path.name)
                if package and (root / "share/doc" / package / "copyright").is_file():
                    return package, root / "share/doc" / package / "copyright"
        owner = subprocess.check_output(["dpkg-query", "-S", str(path)], text=True).splitlines()[0].split(": ",1)[0]
        package = owner.split(":")[0]
        document = Path("/usr/share/doc") / package / "copyright"
        if not document.is_file():
            raise ValueError(f"No native copyright notice for {path.name}")
        return package, document

    with tempfile.TemporaryDirectory(prefix="kode-native-stage-") as directory:
        stage = Path(directory)
        pending = [("ttyd", original)]
        seen = set()
        while pending:
            name, source = pending.pop(0)
            if name in seen:
                continue
            seen.add(name)
            target = stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            target.chmod(0o755)
            subprocess.run(["patchelf", "--set-rpath", "$ORIGIN/lib" if name == "ttyd" else "$ORIGIN", str(target)], check=True)
            dependencies = needed(target)
            elf.append({"file":name,"originalSha256":sha(source.read_bytes()),"needed":dependencies,"minimumGlibc":glibc(target)})
            add(name, target.read_bytes(), True)
            if name != "ttyd":
                package, document = copyright_for(source)
                add("notices/native/" + package + "/copyright", document.read_bytes())
                if package.startswith("libwebsockets"):
                    docs["libwebsockets"] = package
                if package.startswith("libuv"):
                    docs["libuv"] = package
            for dependency in dependencies:
                if dependency in SYSTEM:
                    external.add(dependency)
                else:
                    pending.append(("lib/" + dependency, resolve(dependency)))
    if not all(docs.values()):
        raise ValueError("Missing static libwebsockets/libuv notices")
    for component in ("libwebsockets", "libuv"):
        directory = args.ttyd_bundle / "share/licenses" / component
        if not directory.is_dir() or not any(directory.iterdir()):
            raise ValueError("Missing static native component notices")
        for path in sorted(directory.rglob("*")):
            if path.is_file():
                add("notices/native/" + component + "/" + path.relative_to(directory).as_posix(), path.read_bytes())
    add("notices/native/ttyd/LICENSE", (args.ttyd_bundle / "share/licenses/ttyd/LICENSE").read_bytes())
    add("notices/native/ttyd/build-manifest.json", (json.dumps(provenance,sort_keys=True,indent=2)+"\n").encode())
    # Debian copyright texts reference these standard full licenses.
    for path in sorted(Path("/usr/share/common-licenses").iterdir()):
        if path.is_file() and not path.is_symlink():
            add("notices/native/common-licenses/" + path.name, path.read_bytes())
    frontend = json.loads((ROOT / "packaging/ttyd-frontend-notices.json").read_text())
    if frontend["ttydSourceRevision"] != REVISION:
        raise ValueError("Browser notice catalog is for another ttyd source revision")
    for component in frontend["packages"] + [{"files":frontend["patches"]}]:
        for entry in component["files"]:
            path = ROOT / "packaging" / entry["file"]
            data = path.read_bytes()
            if sha(data) != entry["sha256"]:
                raise ValueError("Pinned browser notice/patch changed")
            add("notices/browser/" + Path(entry["file"]).relative_to("licenses/ttyd-frontend").as_posix(), data)
    add("notices/browser/manifest.json", (json.dumps(frontend,sort_keys=True,indent=2)+"\n").encode())
    manifest = {"schemaVersion":1,"target":"x86_64-unknown-linux-gnu","ttydRevision":REVISION,"externalSystemLibraries":sorted(external),"minimumGlibc":max((item["minimumGlibc"] for item in elf if item["minimumGlibc"]),key=lambda v:tuple(map(int,v.split('.')))),"elf":sorted(elf,key=lambda item:item["file"]),"files":{name:{"sha256":sha(data),"size":len(data),"executable":executable} for name,(data,executable) in sorted(files.items())}}
    args.output.mkdir(parents=True,exist_ok=True)
    for name,(data,executable) in files.items():
        path = args.output / name
        path.parent.mkdir(parents=True,exist_ok=True)
        path.write_bytes(data)
        path.chmod(0o755 if executable else 0o644)
    (args.output / "native-manifest.json").write_text(json.dumps(manifest,sort_keys=True,indent=2)+"\n")
    print(json.dumps({"files":len(files),"elf":len(elf),"minimumGlibc":manifest["minimumGlibc"],"externalSystemLibraries":manifest["externalSystemLibraries"]}))


if __name__ == "__main__":
    main()
