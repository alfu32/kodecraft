#!/usr/bin/env python3
"""Inspect the actual Linux main archive and run it elsewhere, with/without Java."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("runtime_terminal", ROOT / "scripts/test-java-runtime-terminal.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
ARCHIVE = ROOT / "target/distribution/kode-0.1.0-x86_64-unknown-linux-gnu.tar.gz"
REPRO = ROOT / "target/distribution-repro" / ARCHIVE.name


def main():
    assert ARCHIVE.is_file() and REPRO.is_file(), "Assemble both package reproducibility outputs first"
    digest = hashlib.sha256(ARCHIVE.read_bytes()).hexdigest()
    assert hashlib.sha256(REPRO.read_bytes()).hexdigest() == digest
    results = []
    with tempfile.TemporaryDirectory(prefix="kode-package-uat-") as directory:
        base = Path(directory)
        install = base / "install with spaces"
        install.mkdir()
        with tarfile.open(ARCHIVE) as archive:
            assert all(member.isfile() for member in archive.getmembers())
            archive.extractall(install, filter="data")
        package = next(install.iterdir())
        manifest = json.loads((package / "package-manifest.json").read_text())
        assert manifest["javaRuntime"].startswith("external;")
        assert {path.relative_to(package).as_posix() for path in package.rglob("*") if path.is_file()} == set(manifest["files"]) | {"package-manifest.json"}
        for name, entry in manifest["files"].items():
            path = package / name
            assert not path.is_symlink()
            assert path.stat().st_size == entry["size"]
            assert hashlib.sha256(path.read_bytes()).hexdigest() == entry["sha256"]
            assert bool(path.stat().st_mode & 0o111) == entry["executable"]
            assert path.name not in {"java", "java.exe", "libjvm.so", "jvm.dll", "modules"}
        terminal_evidence = json.loads((ROOT / "docs/parity/U03-pty.json").read_text())
        assert terminal_evidence["application_binary_sha256"] == manifest["files"]["kode"]["sha256"], "Terminal and package evidence must cover the same native executable"
        binary = package / "kode"
        assert binary.is_file() and (package / "jdbc/kode-jdbc-bridge.jar").is_file()
        fixture.database.terminal.BINARY = binary
        unrelated = base / "unrelated cwd"
        unrelated.mkdir()
        previous_cwd = Path.cwd()
        os.chdir(unrelated)
        # Ensure no inherited developer override supplies the adjacent helper.
        os.environ.pop("KODE_JDBC_HELPER", None)
        try:
            for local_java in (False, True):
                case = base / ("local-java" if local_java else "no-java")
                project = case / "project with spaces"
                project.mkdir(parents=True)
                note = project / "Note.txt"
                note.write_text("packaged core works without Java\n")
                db = case / "db"
                db.mkdir()
                source = {"id":"fixture", "name":"Packaged H2", "driver":{"id":"h2", "driverClass":"org.h2.Driver"}, "jdbcUrl":"jdbc:h2:mem:packaged_runtime", "username":"sa"}
                # H2 is in the qualified shaded helper; no developer Maven/JAR path.
                (db / "datasources-v1.json").write_text(json.dumps({"version":1,"dataSources":[source]}))
                catalog = case / "lsp.json"
                catalog.write_text('{"servers":[]}')
                release = fixture.Release("offline")
                env = {"KODE_LSP_CATALOG":str(catalog), "KODE_LSP_HOME":str(case / "lsp"), "KODE_RS_DATABASE_HOME":str(db), "KODE_RS_JDBC_HOME":str(case / "plugins"), "KODE_JAVA":shutil.which("java") if local_java else str(case / "absent-java"), "KODE_CONFIG_HOME":str(case / "legacy"), "KODE_PLUGINS_HOME":str(case / "legacy-plugins"), "KODE_RUNTIME_RELEASE_URL":release.url + "/latest"}
                actions = [(b"Ready", .15, b"\x1b4"), (b"Packaged H2", .15, b"c")]
                if local_java:
                    actions += [(b"PACKAGED_RUNTIME", .15, b"s"), (b"Execute", .15, b"\x01" + fixture.database.paste("select 91 as PACKAGE_PROOF;")), (b"PACKAGE_PROOF", .15, b"\x1b[13;5u"), (b"Completed", .15, b"\x11")]
                    expected = [b"Completed"]
                else:
                    actions += [(b"F5 Get Java", .15, b"\x11")]
                    expected = [b"F5 Get Java"]
                try:
                    result = fixture.database.terminal.run_case(note, case / "config", (100,34), actions, expected=expected, env_overrides=env)
                    assert release.requests == [], "A usable local JVM or ordinary core startup fetched runtime metadata"
                    assert not (db / "java-runtime").exists()
                    assert note.read_text() == "packaged core works without Java\n"
                    results.append({"case":"adjacent-helper-real-h2-sql" if local_java else "no-java-core-and-explicit-offer", "unrelated_cwd":True, "installation_path_with_spaces":True, "release_requests":0, **result})
                finally:
                    release.close()
        finally:
            os.chdir(previous_cwd)
    evidence = {"status":"passed", "target":manifest["target"], "archive":ARCHIVE.name, "sha256":digest, "compressedBytes":ARCHIVE.stat().st_size, "javaRuntimeBundled":False, "files":len(manifest["files"])+1, "reproducibleAssembly":True, "terminalBinaryMatchesPackage":True, "nativeDependencies":manifest["nativeDependencies"], "cases":results, "passed":len(results), "scope":"Linux archive assembly and U03 runtime separation; full Z03 installers, publication, native distribution and notice qualification remain pending"}
    (ROOT / "docs/parity/U03-package.json").write_text(json.dumps(evidence,indent=2)+"\n")
    print(json.dumps(evidence,indent=2))


if __name__ == "__main__":
    main()
