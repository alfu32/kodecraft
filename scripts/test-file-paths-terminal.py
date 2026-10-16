#!/usr/bin/env python3
"""Qualify Recent path labels, toggles, save identity and restart in actual PTYs."""
import argparse
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('app_terminal',ROOT/'scripts/test-app-terminal.py')
terminal=importlib.util.module_from_spec(spec);spec.loader.exec_module(terminal)


def left(screen):
    return [''.join(row)[:36] for row in screen.cells[:-1]]


def click(screen,text):
    for y,line in enumerate(left(screen)):
        x=line.find(text)
        if x>=0:return f'\x1b[<0;{x+2};{y+1}M\x1b[<0;{x+2};{y+1}m'.encode()
    return None


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    args=parser.parse_args();terminal.BINARY=args.binary.resolve(strict=True)
    cases=[]
    # Short owned paths make complete absolute paths visible in a normal sidebar.
    with tempfile.TemporaryDirectory(prefix='k8-') as directory:
        base=Path(directory);catalog=base/'lsp.json';catalog.write_text('{"servers":[]}')
        for width in (100,60):
            project=base/f'p{width}';project.mkdir()
            first=project/'a/dup.txt';second=project/'b/dup.txt';outside=base/'out.txt'
            for path,value in ((first,'first original\n'),(second,'second original\n'),(outside,'outside original\n')):
                path.parent.mkdir(parents=True,exist_ok=True);path.write_text(value)
            session=project/'.kode-rs.json'
            session.write_text(json.dumps({'version':1,'recentFiles':[{'path':'a/dup.txt'},{'path':'b/dup.txt'},{'path':str(outside)}]}))
            env={'KODE_LSP_CATALOG':str(catalog),'KODE_LSP_HOME':str(base/'lsp'),'KODE_RS_DATABASE_HOME':str(base/'db'),'KODE_RS_JDBC_HOME':str(base/'jdbc'),'KODE_JAVA':str(base/'absent-java')}
            def mode(screen,full,payload):
                rows=left(screen);text='\n'.join(rows)
                expected=[str(first),str(second)] if full else ['a/dup.txt','b/dup.txt']
                if not all('x '+path in text for path in [*expected,str(outside)]):return None
                if ('[paths:full]' if full else '[paths:rel]') not in text:return None
                return payload(screen) if callable(payload) else payload
            def saved(screen):
                if second.read_text()!='second saved\n':return None
                assert first.read_text()=='first original\n'
                assert outside.read_text()=='outside original\n'
                return b'\x1b1'
            entered=b'\x1b[?1049h'
            actions=[
                (b'Ready',.1,lambda screen:mode(screen,False,lambda s:click(s,'a/dup.txt'))),
                (b'first original',.1,b'\x1b1p'),
                (entered,.1,lambda screen:mode(screen,True,lambda s:click(s,str(second)))),
                (b'second original',.1,b'\x01\x1b[200~second saved\n\x1b[201~\x13'),
                (b'second saved',.1,saved),
                (entered,.1,lambda screen:mode(screen,True,lambda s:click(s,'[paths:full]'))),
                (entered,.1,lambda screen:mode(screen,False,b'p')),
                (entered,.1,lambda screen:mode(screen,True,b'\x11')),
            ]
            result=terminal.run_case(project,base/'config',(width,30),actions,env_overrides=env)
            state=json.loads(session.read_text());assert state['fullFilePaths'] is True
            assert {entry['path'] for entry in state['recentFiles']}=={'a/dup.txt','b/dup.txt',str(outside)}
            cases.append({'case':'relative-full-keyboard-mouse-and-save-identity','duplicateBasenames':True,'outsideRootAbsolute':True,**result})
            actions=[
                (b'second saved',.1,lambda screen:mode(screen,True,lambda s:click(s,'[paths:full]'))),
                (entered,.1,lambda screen:mode(screen,False,b'\x11')),
            ]
            result=terminal.run_case(project,base/'config',(width,30),actions,env_overrides=env)
            assert json.loads(session.read_text())['fullFilePaths'] is False
            assert first.read_text()=='first original\n' and second.read_text()=='second saved\n'
            cases.append({'case':'restart-restores-preference-and-selected-file','physicalFilesUnchanged':True,**result})
            print('Passed relative/full file paths and restart:',width,'columns',flush=True)
    evidence={'status':'passed','cases':cases,'passed':len(cases)}
    (ROOT/'docs/parity/U08-pty.json').write_text(json.dumps(evidence,indent=2)+'\n')


if __name__=='__main__':main()
