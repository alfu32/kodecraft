#!/usr/bin/env python3
"""Convert terminal Copy-as-HTML (<pre>, <span>, <font>) to native SVG."""
import argparse, html, re, unicodedata
from html.parser import HTMLParser
from pathlib import Path

BG='#0F1424'; FG='#B6B9C5'
def cells(s):
    n=0
    for c in s:
        if unicodedata.combining(c) or c in '\ufe0e\ufe0f': continue
        n+=2 if unicodedata.east_asian_width(c) in 'WF' else 1
    return n
class Parser(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.stack=[(FG,BG,False,False)];self.lines=[[]]
    def handle_starttag(self,tag,attrs):
        a=dict(attrs);fg,bg,bold,italic=self.stack[-1]
        if tag in ('span','pre','font','b','strong','i','em'):
            css=dict((k.strip().lower(),v.strip()) for k,v in re.findall(r'([\w-]+)\s*:\s*([^;]+)',a.get('style','')))
            fg=a.get('color',css.get('color',fg));bg=css.get('background-color',bg)
            bold=bold or tag in ('b','strong') or css.get('font-weight','') in ('bold','700')
            italic=italic or tag in ('i','em')
            self.stack.append((fg,bg,bold,italic))
    def handle_endtag(self,tag):
        if tag in ('span','pre','font','b','strong','i','em') and len(self.stack)>1:self.stack.pop()
    def handle_data(self,data):
        for i,part in enumerate(data.split('\n')):
            if i:self.lines.append([])
            if part:self.lines[-1].append((part,self.stack[-1]))

def convert(source,dest,font=12,cell=7.25,line=15,pad=10):
    p=Parser();p.feed(Path(source).read_text(encoding='utf-8'))
    lines=p.lines
    while lines and not lines[-1]:lines.pop()
    width=max((sum(cells(t) for t,_ in runs) for runs in lines),default=0)
    out=[f'<svg xmlns="http://www.w3.org/2000/svg" width="{width*cell+2*pad:.1f}" height="{len(lines)*line+2*pad:.1f}" viewBox="0 0 {width*cell+2*pad:.1f} {len(lines)*line+2*pad:.1f}">',
         f'<rect width="100%" height="100%" fill="{BG}"/>']
    for row,runs in enumerate(lines):
        x=pad;y=pad+row*line
        for s,(fg,bg,bold,italic) in runs:
            w=cells(s)*cell
            if bg.lower()!=BG.lower():out.append(f'<rect x="{x:.2f}" y="{y:.2f}" width="{w:.2f}" height="{line:.2f}" fill="{html.escape(bg,quote=True)}"/>')
            if s.strip():
                out.append(f'<text x="{x:.2f}" y="{y+line*.79:.2f}" xml:space="preserve" fill="{html.escape(fg,quote=True)}" font-family="DejaVu Sans Mono, monospace" font-size="{font}"'+(' font-weight="bold"' if bold else '')+(' font-style="italic"' if italic else '')+f'>{html.escape(s)}</text>')
            x+=w
    out.append('</svg>');Path(dest).write_text('\n'.join(out),encoding='utf-8')
    return width,len(lines)
if __name__=='__main__':
    a=argparse.ArgumentParser();a.add_argument('input');a.add_argument('output');a.add_argument('--font',type=float,default=12);a.add_argument('--cell',type=float,default=7.25);a.add_argument('--line',type=float,default=15);x=a.parse_args()
    print(convert(x.input,x.output,x.font,x.cell,x.line))
