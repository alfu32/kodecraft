"""Shared verified inputs and payload handling for Linux installer formats."""
import hashlib
import json
import os
from pathlib import Path
import tarfile
import tempfile
import urllib.request
from distribution_notices import ROOT, safe, sha


def download(url, expected, destination, limit):
    destination = Path(destination)
    destination.parent.mkdir(parents=True,exist_ok=True)
    if destination.is_file():
        if sha(destination.read_bytes()) != expected:
            raise ValueError("Cached installer input checksum mismatch: " + destination.name)
        return destination
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=destination.parent,delete=False) as output:
            temporary = Path(output.name)
            digest = hashlib.sha256()
            count = 0
            with urllib.request.urlopen(url,timeout=60) as response:
                while chunk := response.read(1024*1024):
                    count += len(chunk)
                    if count > limit:
                        raise ValueError("Installer input exceeds its size limit")
                    output.write(chunk);digest.update(chunk)
            output.flush();os.fsync(output.fileno())
        if digest.hexdigest() != expected:
            raise ValueError("Downloaded installer input checksum mismatch: " + destination.name)
        os.replace(temporary,destination)
        return destination
    finally:
        if temporary and temporary.exists():
            temporary.unlink()


def payload(archive, directory):
    with tarfile.open(archive) as source:
        members = source.getmembers()
        if not members or not all(member.isfile() for member in members):
            raise ValueError("Installer payload must contain only ordinary manifested files")
        if len({member.name for member in members}) != len(members):
            raise ValueError("Duplicate installer input path")
        for member in members:
            safe(member.name)
        roots = {Path(member.name).parts[0] for member in members}
        if len(roots)!=1:
            raise ValueError("Installer payload has multiple roots")
        source.extractall(directory,filter="data")
    package = Path(directory) / roots.pop()
    manifest = json.loads((package / "package-manifest.json").read_text())
    if manifest["target"] not in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
        raise ValueError("Installers require a native Linux GNU payload")
    actual = {path.relative_to(package).as_posix() for path in package.rglob("*") if path.is_file()}
    if actual != set(manifest["files"]) | {"package-manifest.json"}:
        raise ValueError("Installer payload file inventory differs from its manifest")
    for name, entry in manifest["files"].items():
        path = package / safe(name)
        if path.is_symlink() or path.stat().st_size!=entry["size"] or sha(path.read_bytes())!=entry["sha256"] or bool(path.stat().st_mode & 0o111)!=entry["executable"]:
            raise ValueError("Installer payload content/mode differs from its manifest: " + name)
    return package, manifest


def publish(artifact, manifest):
    digest = sha(artifact.read_bytes())
    artifact.with_name(artifact.name+".sha256").write_text(f"{digest}  {artifact.name}\n")
    manifest.update(schemaVersion=1,artifact=artifact.name,sha256=digest,size=artifact.stat().st_size)
    artifact.with_name(artifact.name+".manifest.json").write_text(json.dumps(manifest,sort_keys=True,indent=2)+"\n")
    print(json.dumps(manifest),flush=True)
