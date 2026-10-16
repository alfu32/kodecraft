#!/usr/bin/env python3
"""Real CLI path-field choosers and their owning import/export/save operations."""
import argparse
import importlib.util
import hashlib
import json
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('multiple_terminal', ROOT / 'scripts/test-multiple-cursors-terminal.py')
multi = importlib.util.module_from_spec(spec)
spec.loader.exec_module(multi)
terminal, paste, mouse, when = multi.terminal, multi.paste, multi.mouse, multi.when
ENTERED = multi.ENTERED


def click(label):
    def payload(screen):
        for y, row in enumerate(screen.cells):
            x = ''.join(row).find(label)
            if x >= 0:
                return mouse(x, y) + mouse(x, y, release=True)
        return None
    return payload


def select(path):
    return when('[Select]', b'\t\x01' + paste(str(path)) + b'\t\t\t\r')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    terminal.BINARY = args.binary.resolve(strict=True)
    cases = []
    with tempfile.TemporaryDirectory(prefix='kode-picker-pty-') as directory:
        base = Path(directory)
        catalog = base / 'lsp.json'
        catalog.write_text('{"servers":[]}')
        for width, height in ((100, 34), (60, 24)):
            case = base / str(width)
            case.mkdir()
            outside = case / 'outside space 界'
            outside.mkdir()
            env = {'KODE_LSP_CATALOG': str(catalog), 'KODE_LSP_HOME': str(case / 'lsp'),
                   'KODE_RS_DATABASE_HOME': str(case / 'db'), 'KODE_RS_JDBC_HOME': str(case / 'jdbc'),
                   'KODE_JAVA': str(case / 'absent-java')}

            project = case / 'postman'
            project.mkdir()
            original = {'info': {'name': 'Chosen collection'}, 'item': [], 'x-future': {'keep': True}}
            source = outside / 'input 界.json'
            source.write_text(json.dumps(original))
            output = outside / 'export 界.json'
            actions = [
                (ENTERED, .15, b'\x1b5i'),
                (ENTERED, .15, when('Browse Alt+P', b'\x1bp')),
                (ENTERED, .15, select(source)),
                (ENTERED, .15, when('Browse Alt+P', b'\r')),
                (ENTERED, .15, when('Chosen collection', b'\x1b5e')),
                # Mouse path button, then a failed save-name validation followed by cancel.
                (ENTERED, .15, when('Browse Alt+P', b'\x01' + paste('retained.json'))),
                (ENTERED, .15, click('[Browse Alt+P]')),
                (ENTERED, .15, select(outside / 'missing-parent' / 'bad.json')),
                (ENTERED, .15, when('inspect save', b'\x1b')),
                (ENTERED, .15, when('retained.json', b'\x1bp')),
                (ENTERED, .15, select(output)),
                (ENTERED, .15, when('Browse Alt+P', click(' OK '))),
                (ENTERED, .15, lambda screen: b'\x11' if output.exists() else None),
            ]
            result = terminal.run_case(project, case / 'config-postman', (width, height), actions, env_overrides=env, timeout_seconds=25)
            assert json.loads(output.read_text()) == original
            assert not (outside / 'missing-parent').exists()
            assert not (project / 'retained.json').exists()
            cases.append({'case': 'postman-import-export-outside-unicode-mouse-keyboard-error-cancel', **result})

            project = case / 'openapi'
            project.mkdir()
            source = outside / 'api 界.yaml'
            source.write_text("openapi: 3.0.3\ninfo:\n  title: Selected API\n  version: 1.0.0\npaths:\n  /selected:\n    get:\n      summary: Selected operation\n      responses:\n        '200':\n          description: OK\n")
            actions = [(ENTERED, .15, b'\x1b5m'),
                       (ENTERED, .15, click('Import OpenAPI')),
                       (ENTERED, .15, when('Browse Alt+P', b'\x1bp')),
                       (ENTERED, .15, select(source)),
                       (ENTERED, .15, when('Browse Alt+P', b'\r')),
                       (ENTERED, .15, when('Imported collection', b'\x11'))]
            result = terminal.run_case(project, case / 'config-openapi', (width, height), actions, env_overrides=env, timeout_seconds=25)
            state = json.loads((project / '.kode-rs.json').read_text())
            assert '/selected' in json.dumps(state['restApi']['collection'])
            cases.append({'case': 'openapi-file-picker-and-import', **result})

            project = case / 'sql'
            project.mkdir()
            sql = "select 'picker SQL';\n"
            session = {'id': 'fixture:1', 'dataSourceId': 'fixture', 'title': 'sql@picker:1', 'buffer': sql}
            (project / '.kode-rs.json').write_text(json.dumps({'version': 1, 'database': {'sessions': [session]},
                'activeOrigin': {'Virtual': {'owner': 'database', 'key': 'fixture:1', 'label': 'SQL picker'}}}))
            output = outside / 'query 界.sql'
            actions = [(ENTERED, .15, when('picker SQL', b'\x13')),
                       (ENTERED, .15, when('Browse Alt+P', b'\x1bp')),
                       (ENTERED, .15, when('[Select]', {'resize': [1, 1]})),
                       (ENTERED, .20, b'\t\x01' + paste(str(output)) + b'\t\t\t\r'),
                       (ENTERED, .25, {'resize': [width, height]}),
                       (ENTERED, .15, when('Browse Alt+P', b'\r')),
                       (ENTERED, .15, lambda screen: b'\x11' if output.exists() else None)]
            result = terminal.run_case(project, case / 'config-sql', (width, height), actions, env_overrides=env, timeout_seconds=25)
            assert output.read_text() == sql
            cases.append({'case': 'sql-new-save-name-outside-unicode-single-cell-chooser', **result})

            project = case / 'jar'
            project.mkdir()
            db = case / 'db'
            db.mkdir(exist_ok=True)
            source = outside / 'driver 界.jar'
            source.write_bytes(b'path selection fixture; not executed as a driver')
            definition = {'id': 'fixture', 'name': 'Picker JDBC', 'driver': {'id': 'h2', 'driverClass': 'org.h2.Driver',
                          'jarPath': '/synthetic/first.jar:/synthetic/old.jar', 'x-driver': 7},
                          'jdbcUrl': 'jdbc:h2:mem:picker', 'x-datasource': 9}
            dbfile = db / 'datasources-v1.json'
            dbfile.write_text(json.dumps({'version': 1, 'dataSources': [definition]}))
            actions = [(ENTERED, .15, b'\x1b4'),
                       (ENTERED, .15, click('[edit]')),
                       (ENTERED, .15, when('Edit database connection', b'\t' * 11 + b'\x1bp')),
                       (ENTERED, .15, select(source)),
                       (ENTERED, .15, when('Edit database connection', b'\x13')),
                       (ENTERED, .15, lambda screen: b'\x11' if str(source) in dbfile.read_text() else None)]
            result = terminal.run_case(project, case / 'config-jar', (width, height), actions, env_overrides=env, timeout_seconds=25)
            saved = json.loads(dbfile.read_text())['dataSources'][0]
            assert saved['driver']['jarPath'] == '/synthetic/first.jar:' + str(source)
            assert saved['driver']['x-driver'] == 7 and saved['x-datasource'] == 9
            cases.append({'case': 'local-jar-classpath-preservation-and-datasource-save', 'javaRequired': False, **result})

            for multipart in (False, True):
                project = case / ('multipart' if multipart else 'binary')
                project.mkdir()
                source = outside / 'body 界.bin'
                source.write_bytes(b'body selected file')
                body = {'mode': 'formdata', 'formdata': [{'key': 'upload', 'type': 'file', 'src': ['first.bin'], 'x-row': 3}]} if multipart else {'mode': 'file', 'file': {'src': 'old.bin', 'x-file': 4}}
                collection = {'info': {'name': 'Picker'}, 'item': [{'name': 'File body', 'request': {'method': 'POST', 'url': 'http://127.0.0.1:1/unused', 'body': body}}]}
                (project / '.kode-rs.json').write_text(json.dumps({'version': 1, 'restApi': {'collection': collection},
                    'activeOrigin': {'Virtual': {'owner': 'rest-node', 'key': '0', 'label': 'Body picker'}}}))
                actions = [(ENTERED, .15, click('Body'))]
                if multipart:
                    actions += [(ENTERED, .15, click('upload')),
                                (ENTERED, .15, when('Type text/file', b'\x1bp'))]
                else:
                    actions += [(ENTERED, .15, click('[Browse Alt+P]' if width >= 100 else '[...]'))]
                actions += [(ENTERED, .15, select(source))]
                if multipart:
                    actions += [(ENTERED, .15, when('Type text/file', b'\r')),
                                (ENTERED, .15, when('upload', b'\x11'))]
                else:
                    actions += [(ENTERED, .15, when('Browse Alt+P' if width >= 100 else '[...]', b'\x11'))]
                result = terminal.run_case(project, case / ('config-multipart' if multipart else 'config-binary'), (width, height), actions, env_overrides=env, timeout_seconds=25)
                body = json.loads((project / '.kode-rs.json').read_text())['restApi']['collection']['item'][0]['request']['body']
                if multipart:
                    assert body['formdata'][0]['src'] == ['first.bin', str(source)]
                    assert body['formdata'][0]['x-row'] == 3
                else:
                    assert body['file']['src'] == str(source) and body['file']['x-file'] == 4
                cases.append({'case': 'multipart-source-array-and-metadata' if multipart else 'binary-body-file-and-metadata', **result})

            project = case / 'project'
            project.mkdir()
            source = project / 'src 界'
            source.mkdir()
            excluded = source / 'skip.txt'
            excluded.write_text('unchanged')
            actions = [(ENTERED, .15, b'\x1b2a'),
                       (ENTERED, .15, select(source)),
                       (ENTERED, .15, lambda screen: b'x' if '[Select]' not in multi.text(screen) and 'Source roots:' in multi.text(screen) else None),
                       (ENTERED, .15, select(excluded)),
                       (ENTERED, .15, lambda screen: b'\x1b1' if '[Select]' not in multi.text(screen) and 'Scan exclusions:' in multi.text(screen) else None),
                       (ENTERED, .15, click('[change]')),
                       (ENTERED, .15, select(outside)),
                       (ENTERED, .15, lambda screen: b'\x11' if '[Select]' not in multi.text(screen) and 'api ' in multi.text(screen) else None)]
            result = terminal.run_case(project, case / 'config-project', (width, height), actions, env_overrides=env, timeout_seconds=30)
            config = json.loads((project / '.kode-rs.json').read_text())
            assert 'src 界' in config['sourceRoots']
            assert 'src 界/skip.txt' in config['scanExclusions']
            assert excluded.read_text() == 'unchanged'
            cases.append({'case': 'source-root-file-exclusion-workspace-switch', **result})
            print('Passed all path-field chooser families:', width, 'columns', flush=True)
    (ROOT / 'docs/parity/U07-pty.json').write_text(json.dumps({'status': 'passed', 'binarySha256': hashlib.sha256(terminal.BINARY.read_bytes()).hexdigest(), 'passed': len(cases), 'cases': cases}, indent=2) + '\n')


if __name__ == '__main__':
    main()
