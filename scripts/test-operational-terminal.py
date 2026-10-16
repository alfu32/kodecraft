#!/usr/bin/env python3
"""Qualify generated Help, About, independent lists and idle metrics in real PTYs."""
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('app_terminal', ROOT / 'scripts/test-app-terminal.py')
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)


def panel_text(screen):
    return ''.join(''.join(row).split('│', 1)[0].rstrip() for row in screen.cells[:-1])


def when_panel(text, payload):
    def selected(screen):
        return payload if text in panel_text(screen) else None
    return selected


def click_language(screen):
    for y, row in enumerate(screen.cells):
        text = ''.join(row)
        x = text.find('- zig')
        if x >= 0:
            return f'\x1b[<0;{x+2};{y+1}M\x1b[<0;{x+2};{y+1}m'.encode()
    return None


def when_absent(text, payload):
    return lambda screen: payload if text not in screen.text() else None


def main():
    cases = []
    version = subprocess.check_output([str(terminal.BINARY), '--version'], text=True).split()[-1]
    credits = json.loads((ROOT / 'resources/credits-rust.json').read_text())['packages']
    last_source = next((package.get('repository') or package.get('homepage') for package in reversed(credits) if package.get('repository') or package.get('homepage')))
    samples = []

    def idle_metrics(screen):
        text = panel_text(screen) + ''.join(screen.cells[-1])
        match = re.search(r'Loop:(\d+)\s*Draw:(\d+)\s*RSS:(\d+)MiB\s*CPU:([\d.]+)%', text)
        if match and 0 < int(match[1]) <= 150 and int(match[2]) <= 5 and int(match[3]) > 0:
            samples.append({'width': screen.width, 'loop_per_second': int(match[1]), 'draw_per_second': int(match[2]), 'rss_mib': int(match[3]), 'cpu_percent': float(match[4])})
            return b'\x1b[17~\x1b[200~// settings \x1b[201~\x13'
        return None

    with tempfile.TemporaryDirectory(prefix='kode-operational-pty-') as directory:
        base = Path(directory)
        catalog = base / 'lsp.json'
        catalog.write_text('{"servers":[]}')
        env = {'KODE_LSP_CATALOG': str(catalog), 'KODE_LSP_HOME': str(base / 'lsp'),
               'KODE_RS_DATABASE_HOME': str(base / 'database'), 'KODE_RS_JDBC_HOME': str(base / 'plugins'),
               'KODE_JAVA': str(base / 'absent-java')}
        entered = b'\x1b[?1049h'
        for width, height in [(100, 30), (60, 18)]:
            project = base / str(width)
            project.mkdir()
            file = project / 'note.kt'
            file.write_text('original\n')
            result = terminal.run_case(file, base / 'config', (width, height), [
                (b'original', .15, b'\x1b8'),
                (b'Kode shortcuts', .1, b'\x1b[<0;2;1M\x1b[<0;2;1m\x1b[F'),
                (entered, .1, when_panel('About: Alt+6', b'\x1b[H')),
                (b'Kode shortcuts', .1, b'\x1b6'),
                (f'Kode {version} | MIT'.encode(), .1, b'\x1b[F'),
                (entered, .1, when_panel(last_source, b'\t\x1b[F')),
                (entered, .1, click_language),
                (entered, .1, when_panel('> zig', b'\t\x1b[H')),
                (entered, .1, when_panel('https://github.com/alfu32/kodecraft', b'\x1b7\t')),
                (b'Clear index', .1, idle_metrics),
                (b'settings', .2, b'\x11'),
            ], env_overrides=env)
            assert file.read_text() == '// settings original\n'
            cases.append({'case': 'mouse-help-about-lists-idle-metrics-and-edit-save', **result})
        for width,height in [(100,30),(60,18)]:
            project=base / f'search-{width}'
            project.mkdir()
            file=project / 'note.kt'
            file.write_text('original\n')
            result=terminal.run_case(file,base / 'config',(width,height),[
                (b'original',.15,b'\x06'),
                (b'Find:',.15,b'\x1b'),
                (entered,.15,when_absent(b'Find:',b'\x1bf')),
                (b'Project search',.15,b'\x1b'),
                (entered,.15,when_absent(b'Project search',b'\x1b\x06')),
                (b'Project search',.15,b'\x1b'),
                (entered,.15,when_absent(b'Project search',b'\x1b[102;6u')),
                (b'Project search',.15,b'\x1b'),
                (entered,.15,when_absent(b'Project search',b'\x11')),
            ],env_overrides=env)
            assert file.read_text()=='original\n'
            cases.append({'case':'uat-ctrl-f-file-alt-f-project-and-project-aliases',**result})
        project = base / 'tiny'
        project.mkdir()
        result = terminal.run_case(project, base / 'config', (20, 6), [
            (entered, .2, b'\x1b8\x1b[F\x1b[6~\x1b[H\x1b6\t\x1b[F\x1b[5~\x1b[H\x11'),
        ], env_overrides=env)
        cases.append({'case': 'tiny-help-about-navigation-and-mode-restoration', **result})
    (ROOT / 'docs/parity/Z02-operational-pty.json').write_text(json.dumps({'cases': cases, 'idle_samples': samples, 'passed': len(cases)}, indent=2) + '\n')
    search_cases = [case for case in cases if case['case'].startswith('uat-ctrl-f-file-alt-f-project')]
    (ROOT / 'docs/parity/U01-pty.json').write_text(json.dumps({'cases': search_cases, 'passed': len(search_cases)}, indent=2) + '\n')
    print(f'Passed {len(cases)} real operational UI PTYs; idle samples: {samples}')


if __name__ == '__main__':
    main()
