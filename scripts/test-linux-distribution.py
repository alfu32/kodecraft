#!/usr/bin/env python3
"""Qualify an actual Linux archive, launchers, notices and clean-container browser.

Requires Docker, Playwright/Chrome and local Java for the external-helper case.
All workspaces, configuration, containers and browser input are owned fixtures.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import urllib.request
import subprocess
import tarfile
import tempfile
import uuid
from playwright.sync_api import sync_playwright
from distribution_notices import ROOT, sha

IMAGE = "ubuntu@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55"


def module(name, file):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / file)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, default=ROOT / "target/distribution-z03/kode-0.1.0-x86_64-unknown-linux-gnu.tar.gz")
    parser.add_argument("--repro-archive", type=Path)
    parser.add_argument("--chrome", default=shutil.which("google-chrome"))
    args = parser.parse_args()
    database = module("db_terminal", "test-database-terminal.py")
    browser = module("browser_terminal", "test-serve-browser.py")
    cases = []
    data = args.archive.read_bytes()
    if args.repro_archive:
        assert sha(args.repro_archive.read_bytes()) == sha(data)
    with tempfile.TemporaryDirectory(prefix="kode-distribution-uat-") as directory:
        base = Path(directory)
        install = base / "installation with spaces"
        install.mkdir()
        with tarfile.open(args.archive) as payload:
            assert all(member.isfile() for member in payload.getmembers())
            payload.extractall(install,filter="data")
        package = next(install.iterdir())
        manifest = json.loads((package / "package-manifest.json").read_text())
        actual = {p.relative_to(package).as_posix() for p in package.rglob("*") if p.is_file()}
        assert actual == set(manifest["files"]) | {"package-manifest.json"}
        for name, entry in manifest["files"].items():
            path = package / name
            content = path.read_bytes()
            assert not path.is_symlink() and len(content)==entry["size"] and sha(content)==entry["sha256"]
            assert bool(path.stat().st_mode & 0o111) == entry["executable"]
            if content.startswith(b"\x7fELF"):
                assert str(Path.home()).encode() not in content, f"Developer source path in {name}"
        assert not any(Path(name).name in {"java","java.exe","libjvm.so","jvm.dll","modules"} for name in actual)
        notices = json.loads((package / "notices/manifest.json").read_text())
        for entry in notices["components"]:
            assert entry["files"] and all((package / "notices" / name).is_file() for name in entry["files"])
        assert notices["helperSha256"] == sha((package / "jdbc/kode-jdbc-bridge.jar").read_bytes())
        native = json.loads((package / "native-manifest.json").read_text())
        assert manifest["browserTerminalBundled"] and manifest["minimumGlibc"]=="2.39"
        assert len(native["elf"]) >= 5
        # Launch from an unrelated cwd through the shipped shell and PowerShell
        # launchers. This verifies argv handling and native exit-code forwarding.
        unrelated = base / "unrelated cwd"
        unrelated.mkdir()
        for launcher in ("kode.sh", "kode.ps1"):
            command = [str(package / launcher), "--version"] if launcher.endswith(".sh") else ["pwsh","-NoProfile","-File",str(package / launcher),"--version"]
            result = subprocess.run(command,cwd=unrelated,capture_output=True,text=True)
            assert result.returncode==0 and "kode 0.1.0" in result.stdout
            invalid = subprocess.run(command[:-1]+["--unknown-qualification-option"],cwd=unrelated,capture_output=True,text=True)
            assert invalid.returncode!=0
        cases.append({"case":"manifest-notices-source-paths-launchers", "status":"passed", "files":len(actual), "rustNoticeComponents":sum(c["kind"]=="rust" for c in notices["components"]), "javaNoticeComponents":sum(c["kind"]=="java" for c in notices["components"]), "windowsCmdQualification":"not run; no Windows artifact claimed"})
        # Actual packaged core and shaded H2 helper, without a developer helper or
        # Maven classpath override. The core fixture explicitly has no Java.
        database.terminal.BINARY = package / "kode"
        for has_java in (False, True):
            case = base / ("java" if has_java else "no-java")
            project = case / "project"
            project.mkdir(parents=True)
            note = project / "Note.txt";note.write_text("packaged editor\n")
            db = case / "db";db.mkdir()
            (db / "datasources-v1.json").write_text(json.dumps({"version":1,"dataSources":[{"id":"fixture","name":"Package SQL","driver":{"id":"h2","driverClass":"org.h2.Driver"},"jdbcUrl":"jdbc:h2:mem:package_sql","username":"sa"}]}))
            catalog = case / "lsp.json";catalog.write_text('{"servers":[]}')
            env = {"KODE_RS_DATABASE_HOME":str(db),"KODE_RS_JDBC_HOME":str(case / "plugins"),"KODE_LSP_CATALOG":str(catalog),"KODE_LSP_HOME":str(case / "lsp"),"KODE_JAVA":shutil.which("java") if has_java else str(case / "absent-java"),"KODE_JDBC_HELPER":str(package / "jdbc/kode-jdbc-bridge.jar")}
            actions = [(b"Ready",.15,b"\x1b4"),(b"Package SQL",.15,b"c")]
            if has_java:
                actions += [(b"PACKAGE_SQL",.15,b"s"),(b"Execute",.15,b"\x01"+database.paste("select 91 as ARCHIVE_PROOF;")),(b"ARCHIVE_PROOF",.15,b"\x1b[13;5u"),(b"Completed",.15,b"\x11")]
            else:
                actions += [(b"F5 Get Java",.15,b"\x11")]
            result = database.terminal.run_case(note,case / "config",(100,34),actions,expected=[b"Completed" if has_java else b"F5 Get Java"],env_overrides=env)
            assert not (db / "credentials-v1.json").exists() and not (db / "java-runtime").exists()
            assert note.read_text()=="packaged editor\n"
            cases.append({"case":"packaged-real-h2-sql" if has_java else "packaged-no-java-core",**result})
        # A fresh OS image has no libuv/json-c or Java installed. The gateway and
        # Rust child must run using only the archive plus the documented base.
        workspace = base / "container workspace"
        project = workspace / "project";project.mkdir(parents=True)
        note = project / "Note.txt";note.write_text("container original\n")
        name = "kode-z03-" + uuid.uuid4().hex
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1",0))
            port = reservation.getsockname()[1]
        command = ["docker","run","--detach","--rm","--name",name,"--user",f"{os.getuid()}:{os.getgid()}","--read-only","--tmpfs","/tmp:rw,nosuid,mode=1777","--mount",f"type=bind,source={package},target=/opt/kode,readonly","--mount",f"type=bind,source={workspace},target=/workspace","--network","host","--env","XDG_CONFIG_HOME=/workspace/config","--env","KODE_RS_DATABASE_HOME=/workspace/db","--env","KODE_RS_JDBC_HOME=/workspace/plugins","--env","KODE_LSP_HOME=/workspace/lsp","--env","KODE_JAVA=/not-installed/java",IMAGE,"/opt/kode/kode","serve","--host","127.0.0.1","--port",str(port),"/workspace/project"]
        subprocess.run(command,check=True,capture_output=True,text=True)
        try:
            def pids():
                output = subprocess.check_output(["docker","top",name,"-eo","pid,comm"],text=True)
                return [line.split()[0] for line in output.splitlines()[1:] if line.split()[-1]=="kode"]
            with sync_playwright() as playwright:
                chrome = playwright.chromium.launch(executable_path=args.chrome,headless=True,args=["--no-sandbox"])
                try:
                    page = chrome.new_page(viewport={"width":1100,"height":650})
                    frames = []
                    page.on("websocket",lambda ws:ws.on("framereceived",lambda payload:frames.append(payload.decode(errors="replace") if isinstance(payload,bytes) else payload)))
                    page.on("dialog",lambda dialog:dialog.accept())
                    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                    def listening():
                        try:
                            with opener.open(f"http://127.0.0.1:{port}",timeout=.2) as response:
                                return response.status==200
                        except OSError:
                            return False
                    browser.until(listening,"container gateway HTTP response")
                    page.goto(f"http://127.0.0.1:{port}")
                    browser.visible(page,"Select a file to begin editing.")
                    page.keyboard.press("Enter")
                    browser.click_text(page,"Note.txt")
                    browser.visible(page,"container original")
                    browser.click_text(page,"container original")
                    page.keyboard.press("Control+a")
                    page.evaluate("term.paste('container saved\\n')")
                    page.keyboard.press("Control+s")
                    browser.until(lambda:note.read_text()=="container saved\n","container physical save")
                    assert len(pids())==1
                    page.close()
                    browser.until(lambda:not pids(),"container Rust child reaped")
                finally:
                    chrome.close()
            result = subprocess.run(["docker","exec",name,"/bin/sh","-c","if command -v java; then exit 1; fi; if test -s /etc/ssl/certs/ca-certificates.crt; then exit 1; fi; ldd /opt/kode/ttyd"],capture_output=True,text=True,check=True)
            assert "/opt/kode/lib/libjson-c.so.5" in result.stdout and "not found" not in result.stdout
            cases.append({"case":"clean-ubuntu-container-browser-save-disconnect", "image":IMAGE,"network":"host-loopback","status":"passed","javaAbsent":True,"caTrustStoreAbsent":True,"packagedLibraries":True,"childReaped":True,"physicalSave":True})
        except Exception:
            if 'frames' in locals():
                print("Container child terminal tail:",repr("".join(frames)[-3500:]))
            logs = subprocess.run(["docker","logs",name],capture_output=True,text=True)
            print((logs.stdout + logs.stderr)[-5000:])
            print(subprocess.run(["docker","inspect", "--format", "{{json .NetworkSettings.Ports}}", name],capture_output=True,text=True).stdout)
            print(subprocess.run(["docker","top",name,"-eo","pid,ppid,comm"],capture_output=True,text=True).stdout)
            raise
        finally:
            subprocess.run(["docker","rm","--force",name],capture_output=True,text=True)
    evidence = {"status":"passed","archive":args.archive.name,"sha256":sha(data),"compressedBytes":len(data),"files":len(actual),"minimumGlibc":manifest["minimumGlibc"],"reproducibleAssembly":bool(args.repro_archive),"javaRuntimeBundled":False,"browserTerminalBundled":True,"cases":cases,"passed":len(cases)}
    (ROOT / "docs/parity/Z03-archive.json").write_text(json.dumps(evidence,indent=2)+"\n")
    print(json.dumps(evidence,indent=2))


if __name__ == "__main__":
    main()
