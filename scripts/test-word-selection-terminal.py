#!/usr/bin/env python3
"""Actual word/Alt-word selection in file, folded, SQL and REST CLI editors."""
import argparse
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('multiple_terminal', ROOT / 'scripts/test-multiple-cursors-terminal.py')
multi = importlib.util.module_from_spec(spec)
spec.loader.exec_module(multi)
terminal, paste, find, mouse, when, click = multi.terminal, multi.paste, multi.find, multi.mouse, multi.when, multi.click
ENTERED = multi.ENTERED


class Gestures:
    def __init__(self):
        self.end = None

    def double(self, label, offset=1, end=None, alt=False):
        def payload(screen):
            position = find(screen, label)
            if position is None:
                return None
            x, y = position
            self.end = (x + (len(label) if end is None else end), y)
            button = 8 if alt else 0
            press = mouse(x+offset, y, button) + mouse(x+offset, y, button, True)
            return press + press
        return payload

    def single(self, label, offset=1, alt=False):
        def payload(screen):
            position = find(screen, label)
            if position is None:
                return None
            x, y = position
            button = 8 if alt else 0
            return mouse(x+offset, y, button) + mouse(x+offset, y, button, True)
        return payload

    def ready(self, data):
        return lambda screen: data if (screen.x, screen.y) == self.end else None


def word_edits(gesture, after):
    return [
        (ENTERED, .15, gesture.double('éclair')),
        (ENTERED, .10, gesture.ready(paste('A'))),
        (ENTERED, .10, when('A', b'\x1bu')),
        (ENTERED, .15, when('éclair', gesture.single('tailword', offset=2, alt=True))),
        # Distinct U04 single click followed by a U06 pair over that existing cursor.
        (ENTERED, .45, gesture.double('tailword', offset=2, alt=True)),
        (ENTERED, .10, gesture.ready(paste('B'))),
        (ENTERED, .10, when('B', b'\x1bu')),
        (ENTERED, .15, when('éclair', b'\x1br')),
        (ENTERED, .15, after),
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    terminal.BINARY = args.binary.resolve(strict=True)
    cases = []
    with tempfile.TemporaryDirectory(prefix='kode-word-pty-') as directory:
        base = Path(directory)
        catalog = base / 'lsp.json'
        catalog.write_text('{"servers":[]}')
        for width, height in ((100, 34), (60, 24)):
            case = base / str(width)
            case.mkdir()
            env = {'KODE_LSP_CATALOG': str(catalog), 'KODE_LSP_HOME': str(case / 'lsp'),
                   'KODE_RS_DATABASE_HOME': str(case / 'db'), 'KODE_RS_JDBC_HOME': str(case / 'jdbc'),
                   'KODE_JAVA': str(case / 'absent-java')}
            project = case / 'file'
            project.mkdir()
            file = project / 'words.txt'
            file.write_text('éclair tailword\nsecond third\n')
            expected = 'B B\nsecond third\n'
            gesture = Gestures()
            actions = word_edits(gesture, when('B B', b'\x13'))
            actions.append((ENTERED, .10, lambda screen: b'\x11' if file.read_text() == expected else None))
            result = terminal.run_case(file, case / 'config', (width, height), actions, env_overrides=env, timeout_seconds=20)
            assert file.read_text() == expected
            cases.append({'case': 'unicode-double-and-alt-double-over-existing-cursor-edit-undo-redo-save', **result})
            expected = 'C C\nsecond third\n'
            actions = [(ENTERED, .15, when('B B', b'\x1bu')),
                       (ENTERED, .15, when('éclair', paste('C')+b'\x13')),
                       (ENTERED, .10, lambda screen: b'\x11' if file.read_text() == expected else None)]
            result = terminal.run_case(project, case / 'config', (width, height), actions, env_overrides=env)
            assert file.read_text() == expected
            cases.append({'case': 'restart-undo-restores-both-word-selections', **result})

            project = case / 'glyphs'
            project.mkdir()
            file = project / 'glyphs.txt'
            file.write_text('e\u0301cole\t世界😀  tail::end\n')
            expected = 'N_W界E_tail!:end\n'
            gesture = Gestures()
            actions = []
            for label, offset, end, inserted in [('e\\x0301cole', 4, None, 'N'), ('|-->', 2, None, '_'),
                                                  ('\\x4E16', 3, None, 'W'), ('\\x1F600', 4, None, 'E'),
                                                  ('E  tail', 1, 3, '_'), ('::end', 0, 1, '!')]:
                actions += [(ENTERED, .15, gesture.double(label, offset=offset, end=end)),
                            (ENTERED, .10, gesture.ready(paste(inserted)))]
            actions += [(ENTERED, .15, when('tail!', b'\x13')),
                        (ENTERED, .10, lambda screen: b'\x11' if file.read_text() == expected else None)]
            result = terminal.run_case(file, case / 'config-glyph', (width, height), actions, env_overrides=env, timeout_seconds=20)
            assert file.read_text() == expected
            cases.append({'case': 'combining-tab-cjk-emoji-glyph-interiors-whitespace-punctuation', **result})

            project = case / 'wrap'
            project.mkdir()
            file = project / 'wrapped.txt'
            file.write_text('prefix abcdefghijklmnopqrstuvwxyz0123456789 tail\n')
            expected = 'prefix W tail\n'
            gesture = Gestures()
            def wrapped_ready(screen):
                end = find(screen, ' tail')
                return paste('W') if end is not None and (screen.x, screen.y) == end else None
            actions = [(ENTERED, .15, gesture.double('abcdef', offset=2)), (ENTERED, .15, wrapped_ready),
                       (ENTERED, .15, when('prefix W tail', b'\x13')),
                       (ENTERED, .10, lambda screen: b'\x11' if file.read_text() == expected else None)]
            result = terminal.run_case(file, case / 'config-wrap', (width, height), actions, env_overrides=env)
            assert file.read_text() == expected
            cases.append({'case': 'word-spans-visual-wrapped-rows', **result})

            project = case / 'fold'
            project.mkdir()
            file = project / 'fold.rs'
            file.write_text('fn target_word() {\n    let x = 1;\n}\n')
            expected = 'fn changed() {\n    let x = 1;\n}\n'
            gesture = Gestures()
            def collapse(screen):
                position = find(screen, 'target_word')
                if position is None:
                    return None
                x, y = position
                if screen.cells[y][x-4] != '-':
                    return None
                return mouse(x-4, y)+mouse(x-4, y, release=True)
            def folded_double(screen):
                if ' ... ' not in multi.text(screen):
                    return None
                return gesture.double('target_word', offset=2)(screen)
            actions = [(ENTERED, .15, collapse), (ENTERED, .15, folded_double),
                       (ENTERED, .10, gesture.ready(paste('changed'))),
                       (ENTERED, .15, when('changed', b'\x13')),
                       (ENTERED, .10, lambda screen: b'\x11' if file.read_text() == expected else None)]
            result = terminal.run_case(file, case / 'config-fold', (width, height), actions, env_overrides=env)
            assert file.read_text() == expected
            cases.append({'case': 'word-in-fold-summary-keeps-hidden-source', **result})

            project = case / 'resize'
            project.mkdir()
            file = project / 'resize.txt'
            file.write_text('alpha bravo\n')
            gesture = Gestures()
            other_size = [60, 24] if width == 100 else [100, 34]
            actions = [
                (ENTERED, .10, gesture.single('alpha', offset=2)),
                (ENTERED, .02, {'resize': other_size}),
                (ENTERED, .08, gesture.single('alpha', offset=2)),
                (ENTERED, .10, when('alpha bravo', paste('X')+b'\x13')),
                (ENTERED, .10, lambda screen: b'\x1bu' if file.read_text() == 'alXpha bravo\n' else None),
                (ENTERED, .15, gesture.double('alpha', offset=1)),
                (ENTERED, .10, gesture.ready(paste('R')+b'\x13')),
                (ENTERED, .10, lambda screen: {'resize': [width, height]} if file.read_text() == 'R bravo\n' else None),
                (ENTERED, .15, gesture.double('bravo', offset=1)),
                (ENTERED, .10, gesture.ready(paste('S')+b'\x13')),
                (ENTERED, .10, lambda screen: b'\x11' if file.read_text() == 'R S\n' else None),
            ]
            result = terminal.run_case(file, case / 'config-resize', (width, height), actions, env_overrides=env)
            assert file.read_text() == 'R S\n'
            assert result['resizes'] == [other_size, [width, height]]
            cases.append({'case': 'real-resize-breaks-old-click-pair-and-new-word-hits-follow-new-layout', **result})

            project = case / 'sql'
            project.mkdir()
            session = {'id': 'fixture:1', 'dataSourceId': 'fixture', 'title': 'sql@word:1', 'buffer': 'éclair tailword\n'}
            (project / '.kode-rs.json').write_text(json.dumps({'version': 1, 'database': {'sessions': [session]},
                'activeOrigin': {'Virtual': {'owner': 'database', 'key': 'fixture:1', 'label': 'SQL word'}}}))
            result = terminal.run_case(project, case / 'config-sql', (width, height), word_edits(Gestures(), when('B B', b'\x11')), env_overrides=env)
            state = json.loads((project / '.kode-rs.json').read_text())
            assert state['database']['sessions'][0]['buffer'] == 'B B\n'
            cases.append({'case': 'sql-double-alt-double-and-owned-draft', 'javaRequired': False, **result})

            project = case / 'rest'
            project.mkdir()
            collection = {'info': {'name': 'Words'}, 'item': [{'name': 'Word request', 'request': {'method': 'POST',
                'url': 'http://127.0.0.1:1/synthetic', 'header': [], 'body': {'mode': 'raw', 'raw': 'éclair tailword\n'}}}]}
            (project / '.kode-rs.json').write_text(json.dumps({'version': 1, 'restApi': {'collection': collection},
                'activeOrigin': {'Virtual': {'owner': 'rest-node', 'key': '0', 'label': 'REST request'}}}))
            gesture = Gestures()
            actions = [(ENTERED, .15, lambda screen: click(screen, 'Body'))]
            actions += word_edits(gesture, when('B B', lambda screen: click(screen, 'Headers')))
            actions += [(ENTERED, .15, lambda screen: b'\x01'+paste('A: éclair\nB: tailword\n') if 'Find: ' not in multi.text(screen) else None)]
            actions += word_edits(gesture, when('A: B', lambda screen: click(screen, 'Raw')))
            actions += [(ENTERED, .15, gesture.double('Logical', offset=2)),
                        (ENTERED, .10, gesture.ready(paste('FORBIDDEN'))),
                        (ENTERED, .15, when('read-only', b'\x11'))]
            result = terminal.run_case(project, case / 'config-rest', (width, height), actions, env_overrides=env, timeout_seconds=25)
            state = json.loads((project / '.kode-rs.json').read_text())['restApi']
            assert state['x-kode-rs-request-buffers']['0']['body'] == 'B B\n'
            assert state['x-kode-rs-request-buffers']['0']['headers'] == 'A: B\nB: B\n'
            assert 'FORBIDDEN' not in json.dumps(state)
            cases.append({'case': 'rest-body-headers-word-selections-and-raw-readonly', **result})
            print('Passed word file/glyph/wrap/fold/SQL/REST:', width, 'columns', flush=True)
    (ROOT / 'docs/parity/U06-pty.json').write_text(json.dumps({'status': 'passed', 'cases': cases, 'passed': len(cases)}, indent=2)+'\n')


if __name__ == '__main__':
    main()
