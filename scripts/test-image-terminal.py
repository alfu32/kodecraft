#!/usr/bin/env python3
"""Run image mode/width controls in the real application and verify cell output."""
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('app_pty',ROOT/'scripts/test-app-terminal.py')
app=importlib.util.module_from_spec(spec);spec.loader.exec_module(app)
cases=[]
with tempfile.TemporaryDirectory(prefix='kode-image-pty-') as directory:
    base=Path(directory)
    for size in [(120,30),(20,6),(1,1)]:
        root=base/f'project-{size[0]}';root.mkdir();file=root/'picture.png'
        shutil.copyfile(ROOT/'fixtures/images/checker.png',file)
        entered=b'\x1b[?1049h';trigger=b'[mode]' if size[0]==120 else entered
        expected=['▄'.encode(),b'\x1b[38;2;'] if size[0]==120 else []
        case=app.run_case(file,base/'config',size,[
            (trigger,0.3,b'\r'),
            (entered,0.2,b'\x1b[C'),
            (entered,0.1,b'm'),
            (entered,0.2,b'\r'),
            (entered,0.2,b'\x11'),
        ],expected=expected)
        state=json.loads((root/'.kode-rs.json').read_text())
        assert state['recentFiles'][0]['viewerType']=='IMAGE'
        cases.append(dict(case='image-cells-mode-width-advanced-quit',**case))
(ROOT/'docs/parity/V03-pty.json').write_text(json.dumps(dict(cases=cases,passed=len(cases)),indent=2)+'\n')
print(f'Passed {len(cases)} real image application PTY checks')
