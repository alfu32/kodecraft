#!/usr/bin/env python3
"""Choose connection-type URL templates in the real TUI; never contacts a DB."""
import argparse
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('app_terminal',ROOT/'scripts/test-app-terminal.py')
terminal=importlib.util.module_from_spec(spec);spec.loader.exec_module(terminal)


def paste(value):
    return b'\x1b[200~'+value.encode()+b'\x1b[201~'


def click(screen,label):
    for y,row in enumerate(screen.cells):
        line=''.join(row);x=line.find(label)
        if x>=0:return f'\x1b[<0;{x+2};{y+1}M\x1b[<0;{x+2};{y+1}m'.encode()
    return None


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    args=parser.parse_args();terminal.BINARY=args.binary.resolve(strict=True)
    drivers=json.loads((ROOT/'resources/database/drivers.json').read_text())['drivers']
    catalog=json.loads((ROOT/'resources/database/jdbc-url-templates.json').read_text())['drivers']
    cases=[]
    with tempfile.TemporaryDirectory(prefix='kode-url-templates-') as directory:
        base=Path(directory);lsp=base/'lsp.json';lsp.write_text('{"servers":[]}')
        for width,height in ((100,34),(60,18)):
            for driver_id in ('h2','postgresql','oracle'):
                case=base/f'{width}-{driver_id}';case.mkdir();project=case/'project';project.mkdir()
                note=project/'Note.txt';note.write_text('unchanged editor\n')
                db=case/'db';db.mkdir();store=db/'datasources-v1.json'
                source={'id':'fixture','name':'URL fixture','driver':{'id':'h2','driverClass':'org.h2.Driver'},'jdbcUrl':'jdbc:h2:mem:original'}
                store.write_text(json.dumps({'version':1,'dataSources':[source]}))
                env={'KODE_LSP_CATALOG':str(lsp),'KODE_LSP_HOME':str(case/'lsp'),'KODE_RS_DATABASE_HOME':str(db),'KODE_RS_JDBC_HOME':str(case/'jdbc'),'KODE_JAVA':str(case/'absent-java')}
                driver_index=next(i for i,driver in enumerate(drivers) if driver['id']==driver_id)
                choices=next(entry['templates'] for entry in catalog if entry['driverId']==driver_id)
                selected=choices[1 if driver_id=='postgresql' else 0]
                change_type=b'\x1b[Z\x1b[Z\r\x1b[H'+b'\x1b[B'*driver_index+b'\r'
                def typed(screen,payload):
                    text='\n'.join(''.join(row) for row in screen.cells)
                    expected='Connection type: '+drivers[driver_index]['displayName']
                    if expected not in text or 'jdbc:h2:mem:manual' not in text:return None
                    return payload(screen) if callable(payload) else payload
                def choose(screen):
                    if driver_id=='postgresql':return b'\x1b[H\x1b[B\r'
                    return click(screen,selected['label'])
                def chosen(screen):
                    # Long URL fields scroll to their caret. Save only once the
                    # actual selected template's tail is visible in that field.
                    text='\n'.join(''.join(row) for row in screen.cells)
                    if '[Templates]' not in text or selected['template'][-12:] not in text:return None
                    return b'\x13'
                entered=b'\x1b[?1049h'
                actions=[
                    (b'Ready',.1,b'\x1b4'),
                    (b'URL fixture',.1,b'\x1b[12~'),
                    (b'Connection type:',.1,b'\t\t\x01'+paste('jdbc:h2:mem:manual')),
                    (b'jdbc:h2:mem:manual',.1,change_type),
                    (entered,.1,lambda screen:typed(screen,lambda s:click(s,'[Templates]'))),
                    (b'Standard',.1,b'\x1b'),
                    (entered,.1,lambda screen:typed(screen,b'\x1b[B')),
                    (b'Standard',.1,choose),
                    (entered,.1,chosen),
                    (b'Datasource saved',.1,b'\x1b[12~\t\t\x01'+paste('jdbc:discarded:typed')),
                    (b'jdbc:discarded:typed',.1,b'\x1b'),
                    (b'Databases',.1,b'\x11'),
                ]
                result=terminal.run_case(note,case/'config',(width,height),actions,env_overrides=env,timeout_seconds=20)
                saved=json.loads(store.read_text())['dataSources'][0]
                assert saved['driver']['id']==driver_id,saved
                assert saved['jdbcUrl']==selected['template'],saved
                assert note.read_text()=='unchanged editor\n'
                assert not (db/'credentials-v1.json').exists()
                cases.append({'case':'type-switch-manual-preservation-template-save-and-cancel','driver':driver_id,'choice':'keyboard' if driver_id=='postgresql' else 'mouse','template':selected['template'],'noDatabaseContact':True,**result})
                print('Passed URL templates:',driver_id,width,'columns',flush=True)
    evidence={'status':'passed','cases':cases,'passed':len(cases)}
    (ROOT/'docs/parity/U11-pty.json').write_text(json.dumps(evidence,indent=2)+'\n')


if __name__=='__main__':main()
