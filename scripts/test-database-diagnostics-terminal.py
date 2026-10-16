#!/usr/bin/env python3
"""Read/copy long, redacted JDBC failures in actual isolated CLI PTYs."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location("db_terminal",ROOT/"scripts/test-database-terminal.py")
db_terminal=importlib.util.module_from_spec(spec)
spec.loader.exec_module(db_terminal)
terminal=db_terminal.terminal


def click(x,y):
    return f"\x1b[<0;{x+1};{y+1}M\x1b[<0;{x+1};{y+1}m".encode()


def copy_button(screen):
    for y,row in enumerate(screen.cells):
        text="".join(row)
        if "Copy Ctrl+C" in text:return click(text.index("Copy Ctrl+C"),y)
    return None


def main():
    helper=ROOT/"jdbc-bridge/target/kode-jdbc-bridge.jar"
    java=shutil.which("java")
    assert helper.is_file() and java
    cases=[]
    with tempfile.TemporaryDirectory(prefix="kode-diagnostics-pty-") as directory:
        base=Path(directory)
        source=base/"LongErrorDriver.java"
        source.write_text('''package uat;
import java.sql.*;import java.util.*;import java.util.logging.Logger;
public final class LongErrorDriver implements Driver {
 public Connection connect(String url,Properties info)throws SQLException {
  var failure=new SQLException("FIRST_CONNECTION_FAILURE\\n"+"Diagnostic detail that must remain readable. ".repeat(250)+"\\nTAIL_CONNECTION_REASON password="+info.getProperty("password"),"28000",73);
  failure.setNextException(new SQLException("CHAIN_EXPLANATION_TAIL","HY000",84));throw failure;
 }
 public boolean acceptsURL(String url){return url.startsWith("jdbc:uat:");}
 public DriverPropertyInfo[] getPropertyInfo(String u,Properties p){return new DriverPropertyInfo[0];}
 public int getMajorVersion(){return 1;}public int getMinorVersion(){return 0;}public boolean jdbcCompliant(){return false;}
 public Logger getParentLogger(){return Logger.getGlobal();}
}''')
        classes=base/"classes"
        subprocess.run(["javac","--release","21","-d",str(classes),str(source)],check=True,capture_output=True)
        jar=base/"long-error.jar"
        subprocess.run(["jar","--create","--file",str(jar),"-C",str(classes),"."],check=True,capture_output=True)
        catalog=base/"lsp.json";catalog.write_text('{"servers":[]}')
        for width in [100,60]:
            case=base/str(width);case.mkdir();project=case/"project";project.mkdir()
            note=project/"Note.txt";note.write_text("native editor remains usable\n")
            db=case/"db";db.mkdir()
            definition={"id":"diagnostic","name":"Diagnostic datasource","driver":{"id":"uat-custom","displayName":"Synthetic failure driver","driverClass":"uat.LongErrorDriver","jarPath":str(jar),"artifact":{"repositoryType":"LOCAL_FILE"}},"jdbcUrl":"jdbc:uat:long-error","username":"synthetic-user"}
            store=db/"datasources-v1.json";store.write_text(json.dumps({"version":1,"dataSources":[definition]}))
            pid=case/"java.pid";wrapper=case/"java-fixture"
            wrapper.write_text(f"#!{sys.executable}\nimport json,os,sys\nwith open({str(pid)!r},'w') as out:\n json.dump({{'pid':os.getpid(),'start':open('/proc/self/stat').read().rsplit(') ',1)[1].split()[19]}},out)\nos.execv({java!r},[{java!r},*sys.argv[1:]])\n")
            wrapper.chmod(0o700)
            env={"KODE_LSP_CATALOG":str(catalog),"KODE_LSP_HOME":str(case/"no-lsp"),"KODE_RS_DATABASE_HOME":str(db),"KODE_RS_JDBC_HOME":str(case/"plugins"),"KODE_JDBC_HELPER":str(helper),"KODE_JAVA":str(wrapper),"KODE_CONFIG_HOME":str(case/"unused-legacy"),"KODE_PLUGINS_HOME":str(case/"unused-plugins")}
            actions=[
                (b"Ready",.15,b"\x1b4"),
                (b"Diagnostic datasource",.15,b"\x1b[12~\t\t\t\t"+db_terminal.paste("synthetic-private-password")+b"\x1bt"),
                (b"FIRST_CO",.15,b"\x1b[14~"),
                (b"Copy Ctrl+C",.15,b"\x1b[F"),
                (b"CHAIN_EXPLANATION_TAIL",.15,copy_button),
                (b"CHAIN_EXPLANATION_TAIL",.15,b"\x1b"),
                (b"FIRST_CO",.15,b"\x1b"),
                (b"FIRST_CO",.2,click(40,1)+b"\x01\x16"),
                (b"TAIL_CONNECTION_REASON",.4,b"\x13"),
                (b"TAIL_CONNECTION_REASON",.3,b"\x11"),
            ]
            try:
                result=terminal.run_case(note,case/"config",(width,34),actions,expected=[b"CHAIN_EXPLANATION_TAIL"],env_overrides=env)
                copied=note.read_text()
                assert "FIRST_CONNECTION_FAILURE" in copied and "TAIL_CONNECTION_REASON" in copied and "CHAIN_EXPLANATION_TAIL" in copied
                assert "28000" in copied and "73" in copied and "HY000" in copied and "84" in copied
                assert copied.count("Diagnostic detail that must remain readable.")==250
                assert "synthetic-private-password" not in copied and "<redacted>" in copied
                saved=json.loads((project/".kode-rs.json").read_text())
                assert "synthetic-private-password" not in json.dumps(saved)
                assert not (db/"credentials-v1.json").exists(),"temporary test password was persisted"
                record=json.loads(pid.read_text());assert not Path(f'/proc/{record["pid"]}').exists()
                cases.append({"case":"long-multiline-connection-error-scroll-copy-edit-save","full_tail_and_chain":True,"password_redacted":True,"helper_reaped":True,**result})
                print(f"Passed diagnostic PTY at {width} columns",flush=True)
            finally:
                db_terminal.cleanup_helper(pid)
    (ROOT/"docs/parity/U02-pty.json").write_text(json.dumps({"cases":cases,"passed":len(cases)},indent=2)+"\n")


if __name__=="__main__":main()
