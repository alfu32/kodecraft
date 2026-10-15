#!/usr/bin/env python3
"""Capture frozen Kotlin semantic identity functions with explicit byte inputs."""
import json
from pathlib import Path
import subprocess
import tempfile
ROOT=Path(__file__).resolve().parents[1]
SOURCE=ROOT.parent/'kode/src/main/kotlin/editor/codeintel/model/SemanticModel.kt'
BASELINE='7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68'
assert subprocess.check_output(['git','-C',str(ROOT.parent/'kode'),'rev-parse','HEAD'],text=True).strip()==BASELINE
cases=[dict(path='Program.kt',prefix='',name='Program',kind='CLASS',scope='FILE'),dict(path='src/😀.kt',prefix='val emoji = "😀"; ',name='Føø',kind='FUNCTION',scope='FUNCTION'),dict(path='c/model.h',prefix='/* héader */\n',name='Customer',kind='STRUCT',scope='TYPE')]
lines=['import editor.codeintel.model.*','fun main() {']
for case in cases:
    path=json.dumps(case['path'],ensure_ascii=False);name=json.dumps(case['name'],ensure_ascii=False)
    start=len(case['prefix'].encode());end=start+len(case['name'].encode());case.update(start=start,end=end,offsetUnit='UTF8_BYTES',legacyUtf16Start=len(case['prefix'].encode('utf-16-le'))//2)
    lines += ['run {',f'val file = SemanticIds.file({path})',f'println("${{file.value}}|${{SemanticIds.symbol(file,{start},SymbolKind.{case["kind"]},{name}).value}}|${{SemanticIds.scope(file,{start},ScopeKind.{case["scope"]}).value}}|${{SemanticIds.occurrence(file,{start},{end})}}|${{SemanticIds.type("Int").value}}")','}']
lines+=['}']
with tempfile.TemporaryDirectory(prefix='kode-semantic-oracle-') as directory:
    directory=Path(directory);helper=directory/'Oracle.kt';helper.write_text('\n'.join(lines)+'\n');jar=directory/'oracle.jar'
    subprocess.run(['kotlinc',str(SOURCE),str(helper),'-include-runtime','-d',str(jar)],check=True)
    output=subprocess.check_output(['java','-jar',str(jar)],text=True)
for case,line in zip(cases,output.splitlines(),strict=True):
    case.update(dict(zip(['fileId','symbolId','scopeId','occurrenceId','typeId'],map(int,line.split('|')),strict=True)))
target=ROOT/'fixtures/languages/semantic/ids-kotlin.json';target.write_text(json.dumps(dict(baseline=BASELINE,cases=cases),ensure_ascii=False,indent=2)+'\n')
print(f'Captured {len(cases)} frozen Kotlin byte-coordinate identity cases')
