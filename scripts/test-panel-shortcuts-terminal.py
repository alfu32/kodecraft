#!/usr/bin/env python3
"""Deliver every Alt+number chord through real PTYs and click matching tab targets."""
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location("terminal",ROOT/"scripts/test-app-terminal.py")
terminal=importlib.util.module_from_spec(spec);spec.loader.exec_module(terminal)
PANELS=[("Files","note.txt"),("Project","Project Settings"),("Git","[r] [b] [t] [T]"),("Database","Databases"),("REST","REST API"),("About","Kode "),("Settings","LSP Servers"),("Help","Kode shortcuts")]


def visible(screen,title,marker,payload):
    header="".join(screen.cells[0])[:36]
    body="\n".join("".join(row)[:36] for row in screen.cells[1:-1])
    return payload if title in header and marker in body else None


def mouse_match(title,marker):
    def action(screen):
        header="".join(screen.cells[0])[:36]
        x=header.find(title)
        # F6 returns to the editor; clicking the same visible tab restores the panel.
        data=f"\x1b[17~\x1b[<0;{x+2};1M\x1b[<0;{x+2};1m".encode()
        return visible(screen,title,marker,data) if x>=0 else None
    return action


def main():
    cases=[]
    with tempfile.TemporaryDirectory(prefix="kode-panel-shortcuts-") as directory:
        base=Path(directory);catalog=base/"lsp.json";catalog.write_text('{"servers":[]}')
        for width in [100,60]:
            for encoding in ["escape-prefix","csi-u"]:
                project=base/f"{width}-{encoding}";project.mkdir()
                note=project/"note.txt";note.write_text("original\n")
                env={"KODE_LSP_CATALOG":str(catalog),"KODE_LSP_HOME":str(base/"no-lsp"),"KODE_RS_DATABASE_HOME":str(base/"database"),"KODE_RS_JDBC_HOME":str(base/"jdbc"),"KODE_JAVA":str(base/"absent-java")}
                actions=[]
                for number,(title,marker) in enumerate(PANELS,1):
                    # Chords start with editor focus, then verify mouse parity for the same tab.
                    chord=(b"\x1b"+str(number).encode()) if encoding=="escape-prefix" else f"\x1b[{ord(str(number))};3u".encode()
                    # Click in the unchanged native editor, then deliver the global chord.
                    focus=b"\x1b[<0;41;2M\x1b[<0;41;2m"
                    actions.append((b"\x1b[?1049h",.08,focus+chord))
                    actions.append((b"\x1b[?1049h",.08,mouse_match(title,marker)))
                    actions.append((b"\x1b[?1049h",.08,lambda screen,t=title,m=marker:visible(screen,t,m,b"")))
                actions.append((b"\x1b[?1049h",.12,b"\x11"))
                result=terminal.run_case(note,base/"config",(width,30),actions,env_overrides=env)
                assert note.read_text()=="original\n"
                saved=json.loads((project/".kode-rs.json").read_text())
                assert all(editor.get("history",{}).get("current",{}).get("text","original\n")=="original\n" for editor in saved.get("openEditors",[]))
                cases.append({"case":"all-eight-editor-focus-and-mouse-parity","encoding":encoding,"targets":[p[0] for p in PANELS],**result})
                print(f"Passed all eight panel shortcuts: {width} columns / {encoding}",flush=True)
    (ROOT/"docs/parity/U10-pty.json").write_text(json.dumps({"cases":cases,"passed":len(cases)},indent=2)+"\n")


if __name__=="__main__":main()
