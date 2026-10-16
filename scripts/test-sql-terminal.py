#!/usr/bin/env python3
"""Real SQL console execute/save/restart PTYs, using isolated public H2 fixtures."""
import importlib.util
import json
from pathlib import Path
import shutil
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("database_terminal", ROOT / "scripts/test-database-terminal.py")
database = importlib.util.module_from_spec(spec)
spec.loader.exec_module(database)


def main():
    helper = ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar"
    h2 = Path("/tmp/kode-rs-jdbc-maven-repository/com/h2database/h2/2.2.224/h2-2.2.224.jar")
    java = shutil.which("java")
    assert helper.is_file() and h2.is_file() and java
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-sql-pty-") as directory:
        base = Path(directory)
        catalog = base / "lsp.json"
        catalog.write_text('{"servers":[]}')
        for width, height in [(100, 34), (60, 18)]:
            case = base / str(width)
            project = case / "project"
            project.mkdir(parents=True)
            db = case / "db"
            db.mkdir()
            source = {"id":"fixture","name":"PTY SQL","driver":{"id":"h2","driverClass":"org.h2.Driver","jarPath":str(h2),"artifact":{"repositoryType":"LOCAL_FILE"}},"jdbcUrl":"jdbc:h2:mem:pty_sql","username":"sa","autoCommit":False}
            (db / "datasources-v1.json").write_text(json.dumps({"version":1,"dataSources":[source]}))
            pid = case / "java.pid"
            wrapper = case / "java-fixture"
            wrapper.write_text(f"#!{sys.executable}\nimport json,os,sys\nwith open({str(pid)!r},'w') as out:\n json.dump({{'pid':os.getpid(),'start':open('/proc/self/stat').read().rsplit(') ',1)[1].split()[19]}},out)\nos.execv({java!r},[{java!r},*sys.argv[1:]])\n")
            wrapper.chmod(0o700)
            env = {"KODE_LSP_CATALOG":str(catalog),"KODE_LSP_HOME":str(case / "no-lsp"),"KODE_RS_DATABASE_HOME":str(db),"KODE_RS_JDBC_HOME":str(case / "plugins"),"KODE_CONFIG_HOME":str(case / "unused-legacy"),"KODE_PLUGINS_HOME":str(case / "unused-plugins"),"KODE_JDBC_HELPER":str(helper),"KODE_JAVA":str(wrapper)}
            sql = f"select 'PTY{width}';"
            draft = f"-- RESTORED{width}\n"
            # CSI-u encodes Ctrl+Enter without conflating it with ordinary Enter.
            actions = [(b"Ready",0.15,b"\x1b4"),
                       (b"PTY SQL",0.15,b"c"),
                       (b"PTY_SQL",0.15,b"s"),
                       (b"Execute",0.15,b"\x01"+database.paste(sql)),
                       (f"PTY{width}".encode(),0.15,b"\x1b[13;5u"),
                       (b"Completed",0.15,b"\x13"),
                       (b"Save SQL session",0.15,b"\x01"+database.paste("saved-session.sql")+b"\r"),
                       (b"SQL session saved",0.15,b"\x01"+database.paste(draft)+b"\x11")]
            try:
                result = database.terminal.run_case(project,case / "config",(width,height),actions,expected=[b"Completed",b"SQL session saved"],env_overrides=env)
                assert (project / "saved-session.sql").read_text() == sql
                state = json.loads((project / ".kode-rs.json").read_text())
                sessions = state["database"]["sessions"]
                assert len(sessions) == 1 and sessions[0]["id"] == "fixture:1"
                assert sessions[0]["buffer"] == draft and sessions[0]["autoCommit"] is False
                record = pid.read_text()
                assert not Path(f'/proc/{json.loads(record)["pid"]}').exists()
                cases.append({"case":"actual-h2-ctrl-enter-file-save-dirty-session-quit","owned_helper_reaped":True,**result})
                result = database.terminal.run_case(project,case / "config",(width,height),[(f"RESTORED{width}".encode(),0.2,b"\x11")],expected=[f"RESTORED{width}".encode()],env_overrides=env)
                assert pid.read_text() == record, "restoring a buffer started JDBC"
                assert not (db / "credentials-v1.json").exists()
                assert not (case / "unused-legacy").exists()
                cases.append({"case":"restore-unsaved-sql-without-starting-java","credential_store_untouched":True,**result})
            finally:
                database.cleanup_helper(pid)
    (ROOT / "docs/parity/D08-pty.json").write_text(json.dumps({"cases":cases,"passed":len(cases)},indent=2)+"\n")
    print(f"Passed {len(cases)} SQL CLI/PTY checks")


if __name__ == "__main__":
    main()
