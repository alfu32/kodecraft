#!/usr/bin/env python3
"""Real file/SQL/REST multiple-cursor edits, mouse gestures, clipboard and restore."""
import argparse
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('app_terminal', ROOT / 'scripts/test-app-terminal.py')
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)
ENTERED = b'\x1b[?1049h'
ALL = b'\x1b[13;3u'
DOWN = b'\x1b[1;6B'


def paste(value):
    return b'\x1b[200~' + value.encode() + b'\x1b[201~'


def text(screen):
    return '\n'.join(''.join(row) for row in screen.cells)


def when(marker, payload):
    return lambda screen: (payload(screen) if callable(payload) else payload) if marker in text(screen) else None


def find(screen, label):
    for y, row in enumerate(screen.cells):
        x = ''.join(row).find(label, 37)
        if x >= 0:
            return x, y
    return None


def mouse(x, y, button=0, release=False):
    return f'\x1b[<{button};{x+1};{y+1}{"m" if release else "M"}'.encode()


def click(screen, label):
    position = find(screen, label)
    return None if position is None else mouse(*position) + mouse(*position, release=True)


def editing(screen, marker, payload):
    if marker not in text(screen):
        return None
    find_rows = [y for y, row in enumerate(screen.cells) if 'Find: ' in ''.join(row)]
    if find_rows and screen.y <= find_rows[-1] + 1:
        return None
    return payload


def search_edits(final_payload, start_marker='alpha beta', separator=' '):
    return [
        (ENTERED, .10, when(start_marker, b'\x06' + paste('alpha') + ALL)),
        (ENTERED, .10, lambda screen: editing(screen, f'alpha{separator}gamma', paste('X'))),
        (ENTERED, .10, when(f'X{separator}gamma', b'\x1bu')),
        (ENTERED, .10, when(f'alpha{separator}gamma', b'\x1br')),
        (ENTERED, .10, when(f'X{separator}gamma', b'\x1bu')),
        (ENTERED, .10, when(f'alpha{separator}gamma', b'Z')),
        (ENTERED, .10, when(f'Z{separator}gamma', b'\x1b')),
        (ENTERED, .10, lambda screen: editing(screen, f'Z{separator}gamma', b'\x1b[H' + DOWN + DOWN + paste('P'))),
        (ENTERED, .10, when(f'PZ{separator}delta', final_payload)),
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    terminal.BINARY = args.binary.resolve(strict=True)
    cases = []
    original = 'alpha beta\nalpha gamma\nalpha delta\n'
    expected = 'PZ beta\nPZ gamma\nPZ delta\n'
    with tempfile.TemporaryDirectory(prefix='kode-multiple-pty-') as directory:
        base = Path(directory)
        catalog = base / 'lsp.json'
        catalog.write_text('{"servers":[]}')
        for width, height in ((100, 34), (60, 24)):
            case = base / str(width)
            case.mkdir()
            env = {'KODE_LSP_CATALOG': str(catalog), 'KODE_LSP_HOME': str(case / 'lsp'),
                   'KODE_RS_DATABASE_HOME': str(case / 'db'), 'KODE_RS_JDBC_HOME': str(case / 'jdbc'),
                   'KODE_JAVA': str(case / 'absent-java')}
            project = case / 'file-project'
            project.mkdir()
            file = project / 'multi.txt'
            file.write_text(original)
            actions = search_edits(b'\x13')
            actions.append((ENTERED, .10, lambda screen: b'\x11' if file.read_text() == expected else None))
            result = terminal.run_case(file, case / 'config', (width, height), actions, env_overrides=env, timeout_seconds=25)
            assert file.read_text() == expected
            state = json.loads((project / '.kode-rs.json').read_text())
            assert len(state['openEditors'][0]['history']['current']['secondary']) == 2
            cases.append({'case': 'file-search-all-typing-paste-undo-redo-vertical-cursors-save', **result})

            clip_project = case / 'clipboard-project'
            clip_project.mkdir()
            clip_file = clip_project / 'clipboard.txt'
            clip_file.write_text('é cd ef\n')
            clip_expected = 'Yé\nef cd Yé\nef\n'
            actions = [
                (ENTERED, .15, when('é cd ef', b'\x06'+paste('é|ef')+ALL)),
                (ENTERED, .15, lambda screen: editing(screen, 'é cd ef', b'\x03\x18')),
                (ENTERED, .15, when(' cd ', paste('Y'))),
                (ENTERED, .15, when('Y cd Y', b'\x16')),
                (ENTERED, .15, when('ef cd Yé', b'\x13')),
                (ENTERED, .10, lambda screen: b'\x11' if clip_file.read_text() == clip_expected else None),
            ]
            result = terminal.run_case(clip_file, case / 'config-clipboard', (width, height), actions, env_overrides=env)
            assert clip_file.read_text() == clip_expected
            cases.append({'case': 'source-ordered-multi-copy-cut-and-native-clipboard-worker-paste', **result})

            project = case / 'mouse-project'
            project.mkdir()
            file = project / 'mouse.txt'
            file.write_text('ébcdefgh\n')
            def gestures(screen):
                position = find(screen, 'ébcdefgh')
                if position is None:
                    return None
                x, y = position
                data = b''
                for offset, alt in ((1, False), (4, True)):
                    data += mouse(x+offset, y, 8 if alt else 0) + mouse(x+offset, y, 8 if alt else 0, True)
                return data
            def toggle(screen):
                position = find(screen, 'ébcdefgh')
                if position is None: return None
                x, y = position
                return mouse(x+4, y, 8) + mouse(x+4, y, 8, True)
            def drag(screen):
                position = find(screen, 'ébcdefgh')
                if position is None: return None
                x, y = position
                return mouse(x+2, y, 8) + mouse(x+3, y, 40) + mouse(x+3, y, 8, True) + paste('X') + b'\x13'
            changed = 'éXbXdXefgh\n'
            actions = [(ENTERED, .15, gestures), (ENTERED, .45, toggle), (ENTERED, .45, toggle), (ENTERED, .45, drag), (ENTERED, .10, lambda screen: b'\x11' if file.read_text() == changed else None)]
            result = terminal.run_case(file, case / 'config-mouse', (width, height), actions, env_overrides=env)
            assert file.read_text() == changed
            cases.append({'case': 'alt-click-add-toggle-alt-drag-and-file-save', **result})
            restored = 'éXYbXYdXYefgh\n'
            actions = [(ENTERED, .15, when('éXbXdXefgh', b'Y\x13')),
                       (ENTERED, .10, lambda screen: b'\x11' if file.read_text() == restored else None)]
            result = terminal.run_case(project, case / 'config-mouse', (width, height), actions, env_overrides=env)
            assert file.read_text() == restored
            cases.append({'case': 'restart-restores-all-cursors-and-edit-targets', **result})

            project = case / 'sql-project'
            project.mkdir()
            sql = {'id': 'fixture:1', 'dataSourceId': 'fixture', 'title': 'sql@multi:1', 'buffer': original, 'autoCommit': True}
            (project / '.kode-rs.json').write_text(json.dumps({'version': 1, 'database': {'sessions': [sql]},
                'activeOrigin': {'Virtual': {'owner': 'database', 'key': 'fixture:1', 'label': 'SQL multiple'}}}))
            result = terminal.run_case(project, case / 'config-sql', (width, height), search_edits(b'\x11'), env_overrides=env, timeout_seconds=25)
            state = json.loads((project / '.kode-rs.json').read_text())
            assert state['database']['sessions'][0]['buffer'] == expected, state['database']
            cases.append({'case': 'sql-search-multiple-edits-undo-redo-vertical-and-owned-draft', 'javaRequired': False, **result})

            project = case / 'rest-project'
            project.mkdir()
            collection = {'info': {'name': 'Multiple'}, 'item': [{'name': 'Multiple request', 'request': {
                'method': 'POST', 'url': 'http://127.0.0.1:1/synthetic', 'header': [],
                'body': {'mode': 'raw', 'raw': original}}}], 'x-future': True}
            (project / '.kode-rs.json').write_text(json.dumps({'version': 1, 'restApi': {'collection': collection},
                'activeOrigin': {'Virtual': {'owner': 'rest-node', 'key': '0', 'label': 'REST request'}}}))
            actions = [(ENTERED, .15, lambda screen: click(screen, 'Body'))] + search_edits(lambda screen: click(screen, 'Headers'))
            # Paste three harmless headers, then exercise the same shared search/edit path.
            actions += [(ENTERED, .15, lambda screen: b'\x01'+paste('alpha: beta\nalpha: gamma\nalpha: delta\n') if 'Body' in text(screen) and 'Find: ' not in text(screen) else None)]
            actions += search_edits(lambda screen: click(screen, 'Raw'), 'alpha: beta', ': ')
            actions += [(ENTERED, .15, when('Logical', lambda screen: click(screen, 'Logical') + b'\x06'+paste('PZ')+ALL)),
                        (ENTERED, .15, lambda screen: editing(screen, 'PZ', paste('FORBIDDEN'))),
                        (ENTERED, .15, when('read-only', b'\x11'))]
            result = terminal.run_case(project, case / 'config-rest', (width, height), actions, env_overrides=env, timeout_seconds=35)
            state = json.loads((project / '.kode-rs.json').read_text())['restApi']
            buffers = state['x-kode-rs-request-buffers']['0']
            assert buffers['body'] == expected, buffers
            assert buffers['headers'] == 'PZ: beta\nPZ: gamma\nPZ: delta\n', buffers
            assert state['collection']['x-future'] is True
            assert 'FORBIDDEN' not in json.dumps(state)
            cases.append({'case': 'rest-body-headers-multiple-edits-and-raw-readonly-guard', **result})
            print('Passed file/mouse/restore/SQL/REST multiple cursors:', width, 'columns', flush=True)
    evidence = {'status': 'passed', 'cases': cases, 'passed': len(cases)}
    (ROOT / 'docs/parity/U04-pty.json').write_text(json.dumps(evidence, indent=2)+'\n')


if __name__ == '__main__':
    main()
