#!/usr/bin/env python3
"""Compile frozen pure Kotlin AsciiCore; capture cell/color compatibility goldens."""
import base64
import json
from pathlib import Path
import re
import subprocess
import tempfile
ROOT=Path(__file__).resolve().parents[1]
SOURCE=ROOT.parent/'kode/src/main/kotlin/editor/lib/AsciiCore.kt'
BASELINE='7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68'
assert subprocess.check_output(['git','-C',str(ROOT.parent/'kode'),'rev-parse','HEAD'],text=True).strip()==BASELINE
cases=[]
checker=[0xffff0000,0xff00ff00,0xff0000ff,0x00ffffff]
for gray in [0.0,0.2,0.8]:
    cases.append(dict(name=f'checker-{gray}',width=2,height=2,pixels=checker,outWidth=3,outHeight=2,gray=gray,scatter=1600.0))
cases.append(dict(name='ramp',width=4,height=4,pixels=[0xff000000|((i*17)*0x010101) for i in range(16)],outWidth=2,outHeight=3,gray=0.15,scatter=100.0))
for mask in range(16):
    cases.append(dict(name=f'quadrants-{mask}',width=2,height=2,pixels=[0xffffffff if mask&(1<<i) else 0xff000000 for i in range(4)],outWidth=1,outHeight=1,gray=0.0,scatter=1600.0))
lines=['import editor.lib.AsciiCore','import java.util.Base64','fun main() {']
for case in cases:
    colors=', '.join(f'0x{color:08x}.toInt()' for color in case['pixels'])
    lines += ['run {',f'val pixels = intArrayOf({colors})']
    for mode,method in [('Quart','pixelsToQuadxels'),('Demi','pixelsToBixels'),('Bixel','pixelsToBixels'),('Braille','pixelsToBraille'),('Ascii','pixelsToAscii')]:
        extra=f", grayThreshold={case['gray']}, scatterThreshold={case['scatter']}" if mode in ['Braille','Ascii'] else ''
        lines.append(f'val result{mode} = AsciiCore.{method}(outWidth={case["outWidth"]}, outHeight={case["outHeight"]}{extra}, srcWidth={case["width"]}, srcHeight={case["height"]}) {{ x,y -> pixels[y*{case["width"]}+x] }}')
        lines.append(f'println("{case["name"]}|{mode}|" + Base64.getEncoder().encodeToString(result{mode}.toByteArray(Charsets.UTF_8)))')
    lines += ['}']
lines += ['}']
with tempfile.TemporaryDirectory(prefix='kode-image-oracle-') as directory:
    directory=Path(directory)
    helper=directory/'Oracle.kt';helper.write_text('\n'.join(lines)+'\n')
    jar=directory/'oracle.jar'
    subprocess.run(['kotlinc',str(SOURCE),str(helper),'-include-runtime','-d',str(jar)],check=True)
    output=subprocess.check_output(['java','-jar',str(jar)],text=True)
records=[]
for line in output.splitlines():
    name,mode,encoded=line.split('|')
    ansi=base64.b64decode(encoded).decode()
    foreground=background=None
    cells=[]
    for token in re.split(r'(\x1b\[[0-9;]*m)',ansi):
        if token.startswith('\x1b['):
            codes=[int(code) for code in token[2:-1].split(';')]
            while codes:
                code=codes.pop(0)
                if code==0: foreground=background=None
                elif code in [38,48] and codes[:1]==[2]:
                    color=codes[1:4];codes=codes[4:]
                    if code==38: foreground=color
                    else: background=color
        else:
            for char in token:
                if char!='\n': cells.append(dict(glyph=char,foreground=foreground,background=background))
    case=next(case for case in cases if case['name']==name)
    records.append(dict(**case,mode=mode,cells=cells))
target=ROOT/'fixtures/images/cells-oracle.json'
target.write_text(json.dumps(dict(baseline=BASELINE,source='AsciiCore.kt compiled with kotlinc',cases=records),ensure_ascii=False,indent=2)+'\n')
print(f'Captured {len(records)} original Kotlin cell/color cases')
