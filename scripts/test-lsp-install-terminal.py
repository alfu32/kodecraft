#!/usr/bin/env python3
"""Real CLI failed-install details, wrapped navigation/copy and runtime prerequisite gates."""
import argparse
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('multiple_terminal', ROOT / 'scripts/test-multiple-cursors-terminal.py')
multi = importlib.util.module_from_spec(spec)
spec.loader.exec_module(multi)
terminal, when, ENTERED = multi.terminal, multi.when, multi.ENTERED


class Unavailable(http.server.BaseHTTPRequestHandler):
    hits = 0

    def do_GET(self):
        type(self).hits += 1
        self.send_response(503)
        self.send_header('Content-Length', '0')
        self.end_headers()

    def log_message(self, *args):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    args = parser.parse_args()
    terminal.BINARY = args.binary.resolve(strict=True)
    cases = []
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Unavailable)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        with tempfile.TemporaryDirectory(prefix='kode-lsp-install-pty-') as directory:
            base = Path(directory)
            for width, height in ((100, 34), (60, 24)):
                for runtime_failure in (False, True):
                    case = base / f'{width}-{runtime_failure}'
                    case.mkdir()
                    project = case / 'project'
                    project.mkdir()
                    note = project / 'clipboard.txt'
                    note.write_text('original file\n')
                    url = f'http://127.0.0.1:{server.server_port}/' + 'long-path/' * 600 + 'ACTIONABLE-END.zip'
                    download = {'url': url, 'command': ['absent-fixture-server']}
                    if runtime_failure:
                        download['command'] = ['node', 'server.js']
                        download['kodeRsPackage'] = {'version': 1, 'runtime': {'kind': 'node', 'minimum': '22.22.2'}}
                    catalog = case / 'catalog.json'
                    catalog.write_text(json.dumps({'servers': [{'id': 'fixture', 'name': 'Fixture Server', 'languages': ['fixture'],
                        'preferredCommand': ['absent-fixture-server'], 'versions': [{'version': '1', 'downloads': [download]}]}]}))
                    env = {'KODE_LSP_CATALOG': str(catalog), 'KODE_LSP_HOME': str(case / 'lsp'),
                           'KODE_RS_DATABASE_HOME': str(case / 'db'), 'KODE_RS_JDBC_HOME': str(case / 'jdbc'),
                           'KODE_JAVA': str(case / 'absent-java')}
                    hits_before = Unavailable.hits
                    if runtime_failure:
                        tools = case / 'tools'
                        tools.mkdir()
                        node = tools / 'node'
                        node.write_text("#!/bin/sh\nprintf 'v18.0.0\\n'\n")
                        node.chmod(0o755)
                        env['PATH'] = str(tools) + os.pathsep + os.environ['PATH']
                    actions = [(ENTERED, .15, when('original file', b'\x1b7')),
                               (ENTERED, .15, when('Fixture Server', b'\r'))]
                    if runtime_failure:
                        actions += [(ENTERED, .15, lambda screen: b'\x1bOS' if any('invalid LSP' in ''.join(row) and 'F4 Details' in ''.join(row) for row in screen.cells) else None),
                                    (ENTERED, .15, when('18.0.0', b'\x1b')),
                                    (ENTERED, .15, b'\x11')]
                    else:
                        actions += [(ENTERED, .15, when('HTTP', b'\x1bOS')),
                                    (ENTERED, .15, when('[Copy Ctrl+C]', b'\x1b[F')),
                                    (ENTERED, .15, when('ACTIONABLE-END', b'\x03')),
                                    (ENTERED, .15, b'\x1b'),
                                    (ENTERED, .15, b'\x1b[17~'),
                                    (ENTERED, .15, b'\x01\x16'),
                                    (ENTERED, .15, lambda screen: b'\x13' if 'ACTIONABLE-END.zip)' in ''.join(''.join(row[37:]).strip() for row in screen.cells) else None),
                                    (ENTERED, .15, lambda screen: b'\x11' if len(note.read_text()) > 5000 and 'ACTIONABLE-END' in note.read_text() else None)]
                    result = terminal.run_case(note, case / 'config', (width, height), actions, env_overrides=env, timeout_seconds=25)
                    assert not (case / 'lsp/fixture').exists()
                    assert not (case / 'lsp/.kode-rs-install.lock').exists()
                    if runtime_failure:
                        assert Unavailable.hits == hits_before
                        assert not (case / 'lsp').exists()
                        assert note.read_text() == 'original file\n'
                    else:
                        assert Unavailable.hits == hits_before + 1
                        assert 'Install fixture' in note.read_text() and '503' in note.read_text()
                        assert 'ACTIONABLE-END' in note.read_text() and len(note.read_text()) > 5000
                    cases.append({'case': 'old-node-prerequisite-before-download' if runtime_failure else 'http-failure-details-wrap-scroll-full-copy-to-file', **result})
                print('Passed LSP install diagnostics and runtime gate:', width, 'columns', flush=True)
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=5)
    (ROOT / 'docs/parity/U09-pty.json').write_text(json.dumps({'status': 'passed', 'binarySha256': hashlib.sha256(terminal.BINARY.read_bytes()).hexdigest(), 'passed': len(cases), 'cases': cases}, indent=2) + '\n')


if __name__ == '__main__':
    main()
