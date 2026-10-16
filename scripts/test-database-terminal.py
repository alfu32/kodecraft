#!/usr/bin/env python3
"""Isolated database panel/form PTYs, with actual Java/H2 and no-Java recovery."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("app_terminal", ROOT / "scripts/test-app-terminal.py")
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)


def paste(text):
    return b"\x1b[200~" + text.encode() + b"\x1b[201~"


def cleanup_helper(path):
    """On fixture failure, target only the exact process birth recorded by us."""
    if not path.exists():
        return
    record = json.loads(path.read_text())
    try:
        descriptor = os.pidfd_open(record["pid"])
    except ProcessLookupError:
        return
    try:
        stat = Path(f'/proc/{record["pid"]}/stat').read_text().rsplit(") ", 1)[1].split()
        if stat[19] == record["start"]:
            signal.pidfd_send_signal(descriptor, signal.SIGKILL)
    except (FileNotFoundError, ProcessLookupError):
        pass
    finally:
        os.close(descriptor)


def main():
    helper = ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar"
    assert helper.is_file(), "Build the JDBC helper first"
    java = shutil.which("java")
    assert java, "Java 21 is required for the H2 fixture"
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-database-pty-") as directory:
        base = Path(directory)
        catalog = base / "empty-lsp.json"
        catalog.write_text('{"servers":[]}')
        for unavailable, width, height in [(False, 100, 34), (False, 60, 18), (True, 60, 18)]:
            case = base / f"case-{unavailable}-{width}"
            case.mkdir()
            project = case / "project"
            project.mkdir()
            note = project / "Note.txt"
            note.write_text("unchanged native document\n")
            db = case / "database"
            db.mkdir()
            definition = {"id":"fixture","name":"PTY database","driver":{"id":"h2","displayName":"H2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224"},"jdbcUrl":"jdbc:h2:mem:pty_fixture","username":"sa"}
            store = db / "datasources-v1.json"
            store.write_text(json.dumps({"version":1,"dataSources":[definition]}))
            pid = case / "java.pid"
            wrapper = case / "java-fixture"
            wrapper.write_text(f"#!{sys.executable}\nimport json,os,sys\nwith open({str(pid)!r},'w') as out:\n json.dump({{'pid':os.getpid(),'start':open('/proc/self/stat').read().rsplit(') ',1)[1].split()[19]}},out)\nos.execv({java!r},[{java!r},*sys.argv[1:]])\n")
            wrapper.chmod(0o700)
            environment = {"KODE_LSP_CATALOG":str(catalog),"KODE_LSP_HOME":str(case / "no-lsp"),"KODE_RS_DATABASE_HOME":str(db),"KODE_RS_JDBC_HOME":str(case / "plugins"),"KODE_CONFIG_HOME":str(case / "unused-legacy-config"),"KODE_PLUGINS_HOME":str(case / "unused-legacy-plugins"),"KODE_JDBC_HELPER":str(helper),"KODE_JAVA":str(case / "absent-java") if unavailable else str(wrapper)}
            # The form's four footer buttons are separate horizontal targets at
            # both tested widths. SGR coordinates are one-based.
            modal_width, modal_height = min(width, 86), min(height, 34)
            left, top = (width - modal_width) // 2, (height - modal_height) // 2
            save_x = left + 1 + 2 * ((modal_width - 2) // 4) + 3
            save_y = top + modal_height - 1
            save = f"\x1b[<0;{save_x};{save_y}M\x1b[<0;{save_x};{save_y}m".encode()
            name = f"Edited fixture {width} {'offline' if unavailable else 'H2'}"
            if unavailable:
                actions = [
                    (b"Ready", 0.15, b"\x1b4"),
                    (b"PTY database", 0.15, b"\x1b[12~\t\t\t\x1bt"),
                    (b"Java is unavailable", 0.15, b"\x1b[Z\x1b[Z\x01" + paste(name)),
                    (name.encode(), 0.15, save),
                    (b"Datasource saved", 0.2, b"\x11"),
                ]
                expected = [b"Java is unavailable", b"Datasource saved"]
            else:
                actions = [
                    (b"Ready", 0.15, b"\x1b4"),
                    (b"PTY database", 0.15, b"c"),
                    (b"PTY_FIXTURE", 0.15, b"\x1b[12~\t\x01" + paste(name)),
                    (name.encode(), 0.15, save),
                    (b"Datasource saved", 0.2, b"\x11"),
                ]
                expected = [b"PTY_FIXTURE", b"Datasource saved"]
            try:
                result = terminal.run_case(note, case / "config", (width, height), actions,
                                           expected=expected, env_overrides=environment)
                if not unavailable:
                    owned_pid = json.loads(pid.read_text())["pid"]
                    assert not Path(f"/proc/{owned_pid}").exists(), "owned Java helper was not reaped"
            finally:
                cleanup_helper(pid)
            saved = json.loads(store.read_text())["dataSources"]
            assert len(saved) == 1 and saved[0]["name"] == name
            assert saved[0]["jdbcUrl"] == definition["jdbcUrl"]
            assert saved[0]["username"] == "sa"
            assert not (db / "credentials-v1.json").exists()
            assert note.read_text() == "unchanged native document\n"
            assert not (case / "unused-legacy-config").exists()
            if unavailable:
                assert not pid.exists()
            else:
                owned_pid = json.loads(pid.read_text())["pid"]
                assert not Path(f"/proc/{owned_pid}").exists(), "owned Java helper was not reaped"
            cases.append({"case":"no-java-form-error-edit-mouse-save" if unavailable else "actual-h2-connect-metadata-edit-mouse-save", "helper_started":not unavailable,"owned_helper_absent_after_exit":True,"private_credentials_untouched":True,"source_document_unchanged":True,**result})
        assert subprocess.check_output(["git","-C",str(ROOT.parent / "kode"),"status","--porcelain"]) == b""
    (ROOT / "docs/parity/D07-pty.json").write_text(json.dumps({"cases":cases,"passed":len(cases)},indent=2)+"\n")
    print(f"Passed {len(cases)} database CLI/PTY checks")


if __name__ == "__main__":
    main()
