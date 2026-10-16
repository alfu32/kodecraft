#!/usr/bin/env python3
"""Optional JVM acceptance with the actual generated artifact and isolated CLI PTYs."""
import argparse
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("database_terminal", ROOT / "scripts/test-database-terminal.py")
database = importlib.util.module_from_spec(spec)
spec.loader.exec_module(database)
ARCHIVE = ROOT / "target/java-runtime-artifacts/kode-jdbc-runtime-x86_64-unknown-linux-gnu.zip"
NAME = ARCHIVE.name
F5 = b"\x1b[15~"


def click(x, y):
    return f"\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m".encode()


class Release:
    def __init__(self, mode):
        self.mode = mode
        self.requests = []
        self.zip_attempt = 0
        self.stopped = threading.Event()
        self.payload = ARCHIVE.read_bytes()
        self.checksum = f"{hashlib.sha256(self.payload).hexdigest()}  {NAME}\n".encode()
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                fixture.requests.append(self.path)
                if self.path == "/latest":
                    if mode == "offline":
                        self.send_error(503)
                        return
                    assets = [] if mode == "missing" else [
                        {"name": NAME, "browser_download_url": fixture.url + "/runtime.zip", "size": len(fixture.payload)},
                        {"name": NAME + ".sha256", "browser_download_url": fixture.url + "/checksum", "size": len(fixture.checksum)},
                    ]
                    body = json.dumps({"tag_name": "uat-java-21", "assets": assets}).encode()
                elif self.path == "/checksum":
                    body = fixture.checksum
                elif self.path == "/runtime.zip":
                    fixture.zip_attempt += 1
                    body = fixture.payload
                    if mode == "corrupt-retry" and fixture.zip_attempt == 1:
                        body = b"x" + body[1:]
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                try:
                    if self.path == "/runtime.zip" and mode in ("cancel-retry", "close-active", "hide-edit") and fixture.zip_attempt == 1:
                        for offset in range(0, len(body), 128 * 1024):
                            if fixture.stopped.wait(.04):
                                break
                            self.wfile.write(body[offset:offset+128*1024])
                            self.wfile.flush()
                    else:
                        self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.stopped.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", help="Run one named case without replacing the complete acceptance report")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/kode")
    arguments = parser.parse_args()
    selected_case = arguments.case
    database.terminal.BINARY = arguments.binary.resolve(strict=True)
    assert ARCHIVE.is_file(), "Run scripts/build-java-runtime.py first"
    helper = ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar"
    h2 = Path("/tmp/kode-rs-jdbc-maven-repository/com/h2database/h2/2.2.224/h2-2.2.224.jar")
    assert helper.is_file() and h2.is_file()
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-runtime-pty-") as directory:
        base = Path(directory)
        catalog = base / "lsp.json"
        catalog.write_text('{"servers":[]}')
        modes = [("accept", 100), ("accept", 60), ("decline", 60), ("corrupt-retry", 100), ("missing", 60), ("offline", 100), ("cancel-retry", 60), ("close-active", 100), ("hide-edit", 100), ("local-java", 60), ("late-java", 100), ("form", 100)]
        if selected_case and selected_case not in {mode for mode, _ in modes}:
            parser.error("Unknown case")
        for mode, width in modes:
            if selected_case and mode != selected_case:
                continue
            case = base / f"{mode}-{width}"
            project = case / "project"
            project.mkdir(parents=True)
            note = project / "Note.txt"
            note.write_text("core editor is usable\n")
            db = case / "db"
            db.mkdir()
            source = {"id":"fixture", "name":"Runtime SQL", "driver":{"id":"h2", "driverClass":"org.h2.Driver", "jarPath":str(h2), "artifact":{"repositoryType":"LOCAL_FILE"}}, "jdbcUrl":"jdbc:h2:mem:runtime_sql", "username":"sa"}
            store = db / "datasources-v1.json"
            store.write_text(json.dumps({"version":1,"dataSources":[source]}))
            release = Release(mode)
            env = {"KODE_LSP_CATALOG":str(catalog), "KODE_LSP_HOME":str(case / "no-lsp"), "KODE_RS_DATABASE_HOME":str(db), "KODE_RS_JDBC_HOME":str(case / "plugins"), "KODE_CONFIG_HOME":str(case / "unused-legacy"), "KODE_PLUGINS_HOME":str(case / "unused-plugins"), "KODE_JDBC_HELPER":str(helper), "KODE_JAVA":shutil.which("java") if mode == "local-java" else str(case / "absent-java"), "KODE_RUNTIME_RELEASE_URL":release.url + "/latest"}
            common = [(b"Ready", .15, b"\x1b4"), (b"Runtime SQL", .15, b"c")]
            offer = [(b"F5 Get Java", .15, F5)]
            accept = [(b"JDBC needs Java", .15, b"\r")]
            ready = [(b"Java runtime installed", .15, b"\r")]
            execute = [(b"Runtime SQL", .2, b"c"), (b"RUNTIME_SQL", .15, b"s"), (b"Execute", .15, b"\x01" + database.paste("select 73 as RUNTIME_PROOF;")), (b"RUNTIME_PROOF", .15, b"\x1b[13;5u"), (b"Completed", .15, b"\x11")]
            edit_quit = [(b"Runtime SQL", .15, click(40,1) + b"\x01" + database.paste("core edited after runtime action\n") + b"\x13"), (b"core edited", .15, b"\x11")]
            expected = []
            if mode == "local-java":
                actions = common + [(b"RUNTIME_SQL", .15, b"\x11")]
                expected = [b"RUNTIME_SQL"]
            elif mode == "form":
                actions = common + [(b"F5 Get Java", .15, b"\x1b[12~\t\x01" + database.paste("Draft fields preserved") + b"\x1bt"), (b"F5 Get Java", .15, F5)] + accept + ready + [(b"Draft fields preserved", .15, b"\x1bt"), (b"Success:", .15, b"\x1b")] + execute
                expected = [b"Java runtime installed", b"Success:", b"Completed"]
            elif mode == "late-java":
                def local_becomes_available(_screen):
                    (case / "absent-java").symlink_to(shutil.which("java"))
                    return b"\r"
                actions = common + offer + [(b"JDBC needs Java", .15, local_becomes_available), (b"Java is already available. Retry", .15, b"\r")] + execute
                expected = [b"Java is already available", b"Completed"]
            elif mode == "decline":
                actions = common + offer + [(b"JDBC needs Java", .15, b"\x1b")] + edit_quit
            elif mode in ("missing", "offline"):
                marker = b"matching" if mode == "missing" else b"503"
                actions = common + offer + accept + [(marker, .15, b"\x1b[14~"), (b"Copy Ctrl+C", .15, b"\x1b"), (marker, .15, b"\x1b")] + edit_quit
                expected = [marker, b"Copy Ctrl+C"]
            elif mode == "close-active":
                actions = common + offer + accept + [(b"Downloading", .15, b"\t\r"), (b"Runtime SQL", .15, b"\x11")]
            elif mode == "hide-edit":
                actions = common + offer + accept + [(b"Downloading", .15, b"\t\r")] + edit_quit[:1] + [(b"core edited", .15, b"\x1b4" + F5), (b"Java runtime installed", .15, b"\r"), (b"Runtime SQL", .15, b"\x11")]
                expected = [b"Java runtime installed"]
            else:
                retry = []
                if mode == "corrupt-retry":
                    retry = [(b"[Retry]", .15, b"\r")]
                elif mode == "cancel-retry":
                    retry = [(b"Downloading", .15, b"\r"), (b"cancelled", .15, b"\r")]
                actions = common + offer + accept + retry + ready + execute
                expected = [b"Java runtime installed", b"Completed"]
            try:
                result = database.terminal.run_case(note, case / "config", (width,34), actions, expected=expected, env_overrides=env, timeout_seconds=30)
                installed = db / "java-runtime/current.json"
                success = mode in ("accept", "corrupt-retry", "cancel-retry", "hide-edit", "form")
                assert installed.exists() == success
                assert release.requests.count("/latest") == (2 if mode in ("corrupt-retry", "cancel-retry") else 0 if mode in ("local-java", "late-java", "decline") else 1), release.requests
                assert not (db / "java-runtime/.install.lock").exists()
                assert not list((db / "java-runtime").glob(".tmp*"))
                assert not (db / "credentials-v1.json").exists()
                assert json.loads(store.read_text())["dataSources"] == [source]
                expected_note = "core edited after runtime action\n" if mode in ("decline", "missing", "offline", "hide-edit") else "core editor is usable\n"
                assert note.read_text() == expected_note
                # Both downloaded and configured helpers must be gone at process exit.
                for proc in Path("/proc").iterdir():
                    if proc.name.isdigit():
                        try:
                            command = (proc / "cmdline").read_bytes()
                        except (FileNotFoundError, PermissionError, ProcessLookupError):
                            continue
                        if b"kode-jdbc-bridge.jar" in command:
                            try:
                                environment = (proc / "environ").read_bytes().split(b"\0")
                            except (FileNotFoundError, PermissionError, ProcessLookupError):
                                continue
                            assert ("KODE_RS_DATABASE_HOME=" + str(db)).encode() not in environment, f"unreaped helper {proc.name}"
                cases.append({"case":mode,"actual_artifact":success,"release_requests":release.requests,"installed":success,"core_preserved":True, "owned_helpers_absent_after_exit":True, **result})
                print(f"Passed Java runtime PTY {mode} at {width} columns", flush=True)
            finally:
                release.close()
    if selected_case:
        return
    (ROOT / "docs/parity/U03-pty.json").write_text(json.dumps({"cases":cases,"passed":len(cases),"artifact_sha256":hashlib.sha256(ARCHIVE.read_bytes()).hexdigest(),"application_binary_sha256":hashlib.sha256(database.terminal.BINARY.read_bytes()).hexdigest()},indent=2)+"\n")


if __name__ == "__main__":
    main()
