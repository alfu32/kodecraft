#!/usr/bin/env python3
"""Actual hex layouts, UTF-8/hex find, wrap, validation and large-search stop."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import time

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('app_terminal',ROOT/'scripts/test-app-terminal.py')
terminal=importlib.util.module_from_spec(spec);spec.loader.exec_module(terminal)


def paste(value):return b'\x1b[200~'+value.encode()+b'\x1b[201~'
def text(screen):return '\n'.join(''.join(row) for row in screen.cells)
def click(screen,label,left_only=False):
    for y,row in enumerate(screen.cells):
        line=''.join(row);line=line[:36] if left_only else line
        x=line.find(label)
        if x>=0:return f'\x1b[<0;{x+2};{y+1}M\x1b[<0;{x+2};{y+1}m'.encode()
    return None
def when(marker,payload):
    return lambda screen:(payload(screen) if callable(payload) else payload) if marker in text(screen) else None
def layout(columns,payload):
    def ready(screen):
        lines=[''.join(row) for row in screen.cells]
        for y,line in enumerate(lines[:-1]):
            x=line.find('00000000')
            if x>=0 and lines[y+1][x:x+8]==f'{columns:08X}':
                return payload(screen) if callable(payload) else payload
        return None
    return ready


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    args=parser.parse_args();terminal.BINARY=args.binary.resolve(strict=True)
    cases=[];entered=b'\x1b[?1049h';previous=b'\x1b[13;2u'
    with tempfile.TemporaryDirectory(prefix='kode-hex-find-') as directory:
        base=Path(directory);catalog=base/'lsp.json';catalog.write_text('{"servers":[]}')
        env={'KODE_LSP_CATALOG':str(catalog),'KODE_LSP_HOME':str(base/'lsp'),'KODE_RS_DATABASE_HOME':str(base/'db'),'KODE_RS_JDBC_HOME':str(base/'jdbc'),'KODE_JAVA':str(base/'absent-java')}
        for width,height in ((200,30),(100,24),(60,18)):
            project=base/f'p{width}';project.mkdir();file=project/'bytes.bin'
            content=bytearray([0x10]*128);content[0]=0
            for offset in (15,95):content[offset:offset+5]='é界'.encode()
            for offset in (62,125):content[offset:offset+3]=bytes([0,255,65])
            original=bytes(content);file.write_bytes(original)
            actions=[
                (b'00000000',.08,lambda screen:click(screen,'[8]')),
                (entered,.08,layout(8,lambda screen:click(screen,'[32]'))),
                (entered,.08,layout(32,lambda screen:click(screen,'[16]'))),
                (entered,.08,layout(16,lambda screen:click(screen,'[Find]'))),
                (entered,.08,when('[Text]',paste('é界')+b'\r')),
                (entered,.08,when('Match @ 0000000F',b'\r')),
                (entered,.08,when('Match @ 0000005F',previous)),
                (entered,.08,when('Match @ 0000000F',b'\t\x01'+paste('00 FF 41')+b'\r')),
                (entered,.08,when('Match @ 0000003E',b'\r')),
                (entered,.08,when('Match @ 0000007D',previous)),
                (entered,.08,when('Match @ 0000003E',previous)),
                (entered,.08,when('Match @ 0000007D [wrap]',b'\r')),
                (entered,.08,when('Match @ 0000003E [wrap]',b'\x1b[19~')),
                (entered,.08,layout(32,b'\x01'+paste('0G')+b'\r')),
                (entered,.08,when('Use hex bytes: 00 FF',b'\x01'+paste('ABC')+b'\r')),
                (entered,.08,when('Odd hex digit count',b'\x01'+paste('FF FF FF FF')+b'\r')),
                (entered,.08,when('No match',b'\x1b')),
                (entered,.08,lambda screen:b'\x1bOR' if '[Stop]' not in text(screen) else None),
                (entered,.08,lambda screen:b'\x1b' if 'No match' in text(screen) and '[Stop]' in text(screen) else None),
                (entered,.08,lambda screen:b'\x1b[19~' if '[Stop]' not in text(screen) else None),
                (entered,.08,layout(8,b'\x11')),
            ]
            result=terminal.run_case(file,base/'config',(width,height),actions,env_overrides=env,timeout_seconds=20)
            assert file.read_bytes()==original
            cases.append({'case':'all-layouts-text-hex-next-previous-wrap-invalid-and-no-match','physicalBytesUnchanged':True,'utf8CrossesRow':True,'binaryAtEof':True,**result})
            print('Passed hex layouts/find:',width,'columns',flush=True)
        project=base/'large';project.mkdir();file=project/'large.bin';note=project/'Note.txt'
        note.write_text('small original\n')
        with file.open('wb') as output:
            output.write(b'\0'+b'A'*(1024*1024-1))
            for _ in range(63):output.write(b'A'*(1024*1024))
        original_sha=hashlib.sha256(file.read_bytes()).hexdigest()
        timings={}
        def stop(screen,payload):
            if 'Searching; Esc stops' not in text(screen):return None
            if callable(payload):
                data=payload(screen)
                if data is not None:timings['stopRequestAt']=time.monotonic()
                return data
            return payload
        def stopped(screen):
            if 'Search stopped' not in text(screen):return None
            timings['stopResponseMs']=round((time.monotonic()-timings['stopRequestAt'])*1000,2)
            return b'\r'
        def switch(screen):
            if 'Searching; Esc stops' not in text(screen):return None
            timings['switchAt']=time.monotonic();return b'\x1b1'
        def edit(screen):
            if 'small original' not in text(screen):return None
            timings['fileSwitchResponseMs']=round((time.monotonic()-timings['switchAt'])*1000,2)
            return b'\x01'+paste('typed after search cancellation\n')+b'\x13'
        actions=[
            (b'00000000',.1,b'\x06'),
            (entered,.02,when('[Text]',paste('A'*4095+'B')+b'\r')),
            (entered,.01,lambda screen:stop(screen,b'\x1b')),
            (entered,.05,lambda screen:b'\x06\r' if '[Stop]' not in text(screen) and '[Find]' in text(screen) else None),
            (entered,.01,lambda screen:stop(screen,lambda s:click(s,'[Stop]'))),
            (entered,.01,stopped),
            (entered,.01,switch),
            (entered,.01,lambda screen:click(screen,'Note.txt',left_only=True)),
            (entered,.01,edit),
            (entered,.05,when('typed after search cancellation',b'\x11')),
        ]
        result=terminal.run_case(file,base/'config-large',(100,30),actions,env_overrides=env,timeout_seconds=30)
        assert hashlib.sha256(file.read_bytes()).hexdigest()==original_sha
        assert note.read_text()=='typed after search cancellation\n'
        cases.append({'case':'64-mib-repetitive-search-escape-stop-switch-and-typing','fileBytes':file.stat().st_size,'physicalBytesUnchanged':True,'stopResponseMs':timings['stopResponseMs'],'fileSwitchResponseMs':timings['fileSwitchResponseMs'],**result})
        print('Passed 64 MiB search stop/switch/typing',flush=True)
    evidence={'status':'passed','cases':cases,'passed':len(cases)}
    (ROOT/'docs/parity/U05-pty.json').write_text(json.dumps(evidence,indent=2)+'\n')


if __name__=='__main__':main()
