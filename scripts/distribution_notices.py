#!/usr/bin/env python3
"""Collect exact Rust/Linux and shaded-helper license texts without network access.

Build the JDBC helper first. Missing texts, changed pinned inputs and unsafe paths
are errors. Upstream texts omitted from crates/JARs live in packaging/licenses/.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tarfile
import tomllib
import xml.etree.ElementTree as ET
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-unknown-linux-gnu"
PREFIXES = ("license", "licence", "copying", "notice", "copyright")
CODE_SUFFIXES = {".rs", ".c", ".h", ".js", ".cpp", ".py", ".toml", ".class", ".json"}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def legal_name(name):
    path = PurePosixPath(name)
    return path.name.lower().startswith(PREFIXES) and path.suffix.lower() not in CODE_SUFFIXES


def safe(name):
    path = PurePosixPath(name)
    if not path.parts or path.is_absolute() or any(part in ("..", ".") for part in path.parts) or "\\" in name or ":" in name:
        raise ValueError(f"Unsafe notice path: {name}")
    return name


def java_inputs():
    listing = ROOT / "jdbc-bridge/target/runtime-dependencies.txt"
    if not listing.is_file():
        raise ValueError("Build the JDBC helper to capture exact runtime notice inputs")
    result = []
    for line in listing.read_text().splitlines():
        fields = line.strip().split(" -- ", 1)[0].split(":")
        if len(fields) != 6:
            continue
        group, artifact, kind, version, scope, location = fields
        if kind != "jar" or scope not in ("compile", "runtime") or not all(re.fullmatch(r"[A-Za-z0-9_.-]+", part) for part in (group, artifact, version)):
            raise ValueError("Unsupported Java runtime dependency coordinate")
        jar = Path(location).resolve(strict=True)
        if jar.name != f"{artifact}-{version}.jar":
            raise ValueError("Java artifact filename does not match its coordinate")
        pom = jar.with_suffix(".pom")
        result.append(({"group":group, "artifact":artifact, "version":version, "jarSha256":sha(jar.read_bytes()), "pomSha256":sha(pom.read_bytes())}, jar, pom))
    if not result or len({tuple(sorted(entry.items())) for entry, _, _ in result}) != len(result):
        raise ValueError("Missing or duplicate Java runtime dependencies")
    return sorted(result, key=lambda item: (item[0]["group"], item[0]["artifact"]))


def collect():
    files = {}
    components = []

    def add(name, data):
        safe(name)
        if not data.strip() or len(data) > 8 * 1024 * 1024:
            raise ValueError(f"Empty or oversized notice: {name}")
        if name in files and files[name] != data:
            raise ValueError(f"Conflicting notice: {name}")
        files[name] = data
        return name

    def override(entry, prefix):
        name = safe(entry["file"])
        path = ROOT / "packaging" / name
        if path.is_symlink():
            raise ValueError("Notice overrides must be ordinary files")
        data = path.read_bytes()
        if sha(data) != entry["sha256"]:
            raise ValueError(f"Pinned upstream notice changed: {name}")
        return add(prefix + "/" + Path(name).name, data)

    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT))
    tree = subprocess.check_output(["cargo", "tree", "--locked", "-p", "kode", "--target", TARGET, "--edges", "normal,build", "--prefix", "none", "--format", "{p}"], cwd=ROOT, text=True)
    selected = {tuple(line.removesuffix(" (*)").split()[:2]) for line in tree.splitlines()}
    overrides = {(p["name"], p["version"]):p for p in json.loads((ROOT / "packaging/license-overrides.json").read_text())["packages"]}
    packages = [p for p in metadata["packages"] if p["id"] not in metadata["workspace_members"] and (p["name"], "v" + p["version"]) in selected]
    cargo_lock = {(p["name"], p["version"]):p for p in tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]}
    for package in sorted(packages, key=lambda p: (p["name"], p["version"])):
        source = Path(package["manifest_path"]).parent
        prefix = f"rust/{package['name']}-{package['version']}"
        component = {"kind":"rust", "name":package["name"], "version":package["version"], "license":package["license"], "repository":package["repository"], "files":[]}
        checksum = cargo_lock[(package["name"], package["version"])]["checksum"]
        archive_path = source.parents[2] / "cache" / source.parent.name / (source.name + ".crate")
        if sha(archive_path.read_bytes()) != checksum:
            raise ValueError(f"Locked Cargo archive checksum mismatch: {package['name']}")
        component["crateChecksum"] = checksum
        with tarfile.open(archive_path) as archive:
            members = {member.name.removeprefix(source.name + "/"):member for member in archive.getmembers() if member.isfile()}
            for relative, member in sorted(members.items()):
                if legal_name(relative):
                    component["files"].append(add(prefix + "/" + safe(relative), archive.extractfile(member).read()))
            if not component["files"]:
                entry = overrides.get((package["name"], package["version"]))
                if not entry or not entry["files"]:
                    raise ValueError(f"No full license text for {package['name']} {package['version']}")
                vcs = json.loads(archive.extractfile(members[".cargo_vcs_info.json"]).read())
                if vcs["git"]["sha1"] != entry["sourceRevision"]:
                    raise ValueError("Crate source revision differs from notice override")
                component["upstreamLicenseSources"] = entry["files"]
                component["files"] = [override(file, prefix) for file in entry["files"]]
                # Retain upstream license declarations and authors, including
                # packages that declare dual licensing but omit full texts.
                for name in ("Cargo.toml.orig", "README.md"):
                    component["files"].append(add(prefix + "/upstream-" + name, archive.extractfile(members[name]).read()))
                if entry.get("selectedLicense"):
                    component["selectedLicense"] = entry["selectedLicense"]
        components.append(component)

    java = java_inputs()
    lock = json.loads((ROOT / "packaging/java-dependencies.json").read_text())
    if [entry for entry, _, _ in java] != lock["artifacts"]:
        raise ValueError("Java runtime dependency lock differs; inspect the resolved artifacts before updating it")
    extras = {p["component"]:p for p in json.loads((ROOT / "packaging/extra-license-sources.json").read_text())["files"]}
    for entry, jar, pom in java:
        group, artifact, version = (entry[key] for key in ("group", "artifact", "version"))
        prefix = f"java/{group}.{artifact}-{version}"
        component = {"kind":"java", "name":f"{group}:{artifact}", "version":version, "jarSha256":entry["jarSha256"], "sourceArchive":f"https://repo.maven.apache.org/maven2/{group.replace('.', '/')}/{artifact}/{version}/{artifact}-{version}-sources.jar", "files":[]}
        with zipfile.ZipFile(jar) as archive:
            for name in sorted(archive.namelist()):
                if not name.endswith("/") and legal_name(name):
                    component["files"].append(add(prefix + "/jar/" + safe(name), archive.read(name)))
        if not component["files"]:
            extra = extras.get(f"{artifact}-{version}")
            if not extra or extra.get("missing"):
                raise ValueError(f"No full license text for Java artifact {artifact} {version}")
            component["files"].append(override(extra, prefix))
            component["upstreamLicenseSources"] = [extra]
        # Keep original and inherited POM license/developer declarations rather
        # than synthesizing or guessing copyright holders.
        repository = jar.parents[len(group.split('.')) + 2]
        visited = set()
        current = pom
        while current not in visited:
            visited.add(current)
            data = current.read_bytes()
            component["files"].append(add(prefix + "/poms/" + current.name, data))
            parent = ET.fromstring(data).find("{*}parent")
            if parent is None:
                break
            parent_group, parent_artifact, parent_version = (parent.findtext("{*}" + key) for key in ("groupId", "artifactId", "version"))
            if not all(value and re.fullmatch(r"[A-Za-z0-9_.-]+", value) for value in (parent_group, parent_artifact, parent_version)):
                raise ValueError("Unresolved Java parent POM coordinate")
            current = repository.joinpath(*parent_group.split('.'), parent_artifact, parent_version, f"{parent_artifact}-{parent_version}.pom")
        components.append(component)
    add("README.txt", b"Third-party license and notice texts for Kode's Linux Rust build/runtime dependencies and shaded external JDBC helper. Original upstream notices and declarations are retained; build-time crates may be included. The application is MIT licensed. Native/browser components have separate notices when bundled. Source archive URLs identify the unmodified Java library sources; H2's original dual-license text is retained. No JDBC Java runtime is included.\n")
    manifest = {"schemaVersion":1, "target":TARGET, "scope":"Linux normal/build Rust dependencies and exact Java helper runtime dependencies", "cargoLockSha256":sha((ROOT / "Cargo.lock").read_bytes()), "helperSha256":sha((ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar").read_bytes()), "components":components, "files":{name:{"sha256":sha(data), "size":len(data)} for name, data in sorted(files.items())}}
    return files, manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/distribution-notices")
    parser.add_argument("--write-java-lock", action="store_true", help="Capture the inspected exact runtime artifact/POM hashes")
    args = parser.parse_args()
    if args.write_java_lock:
        (ROOT / "packaging/java-dependencies.json").write_text(json.dumps({"schemaVersion":1,"artifacts":[entry for entry, _, _ in java_inputs()]}, indent=2)+"\n")
    files, manifest = collect()
    args.output.mkdir(parents=True, exist_ok=True)
    for name, data in files.items():
        path = args.output / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    (args.output / "manifest.json").write_text(json.dumps(manifest, sort_keys=True, indent=2)+"\n")
    print(json.dumps({"components":len(manifest["components"]), "rust":sum(c["kind"]=="rust" for c in manifest["components"]), "java":sum(c["kind"]=="java" for c in manifest["components"]), "files":len(files), "bytes":sum(len(data) for data in files.values())}))


if __name__ == "__main__":
    main()
