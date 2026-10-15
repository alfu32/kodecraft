#!/usr/bin/env python3
"""Record pinned grammar ABI, node fields and exact synthetic byte ranges."""
import json
from pathlib import Path
import subprocess
ROOT=Path(__file__).resolve().parents[1]
result=subprocess.run(['cargo','run','--offline','-p','kode-syntax','--example','parser_inventory','-j','2'],cwd=ROOT,stdout=subprocess.PIPE,check=True)
data=json.loads(result.stdout)
errors=[binding['grammar']['language'] for binding in data['bindings'] if binding['hasErrors']]
assert not errors, f'Qualification snippets have parse errors: {errors}'
assert not data['keywords']['diagnostics']
manifest=dict(runtime=data['runtime'],coordinateUnit='UTF8_BYTES',ownRustUnsafe='forbidden',grammars=[binding['grammar'] for binding in data['bindings']])
path=ROOT/'resources/grammars/manifest.json';path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(manifest,indent=2)+'\n')
path=ROOT/'fixtures/languages/parsers/node-fields.json';path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
path=ROOT/'docs/parity/L02-patterns.json';path.write_text(json.dumps(data['keywords'],ensure_ascii=False,indent=2)+'\n')
print(f'Qualified {len(data["bindings"])} parser variants and {len(data["keywords"]["languages"])} keyword patterns')
