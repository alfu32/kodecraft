#!/usr/bin/env python3
"""Run actual Settings -> native clear/rebuild workers without Java."""
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('app_terminal', ROOT / 'scripts/test-app-terminal.py')
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)


def when_indexed(payload, count, symbols=True):
    def select(screen):
        rows = [''.join(row).split('│', 1)[0] for row in screen.cells[:-1]]
        rows.append(''.join(screen.cells[-1]))
        text = ' '.join(' '.join(rows).split()).encode()
        return payload if b'index Ready' in text and f'{count} files'.encode() in text and (not symbols or f'{count} symbols'.encode() in text) else None
    return select


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix='kode-settings-pty-') as directory:
        base = Path(directory)
        catalog = base / 'lsp.json'
        catalog.write_text('{"servers":[]}')
        for width, height in [(100, 30), (60, 18)]:
            project = base / str(width) / 'workspace'
            (project / 'src').mkdir(parents=True)
            file = project / 'src/Model.kt'
            file.write_text('class Saved {}\n')
            (file.parent / '.kode/db').mkdir(parents=True)
            legacy = file.parent / '.kode/db/kode.mv.db'
            legacy.write_bytes(b'synthetic H2 sentinel')
            env = {'KODE_LSP_CATALOG': str(catalog), 'KODE_LSP_HOME': str(base / 'lsp'),
                   'KODE_RS_DATABASE_HOME': str(base / 'database'), 'KODE_RS_JDBC_HOME': str(base / 'plugins'),
                   'KODE_JAVA': str(base / 'absent-java')}
            result = terminal.run_case(file, base / 'config', (width, height), [
                (b'Saved', .15, when_indexed(b'\x1b4\t', 1, symbols=False)),
                (b'Clear index', .1, b'c'),
                (b'Clear index', .1, when_indexed(b'b', 0)),
                (b'Clear index', .1, when_indexed(b'\x1b[17~\x1b[200~// preserved \x1b[201~\x13', 1)),
                (b'preserved', .2, b'\x11'),
            ], env_overrides=env)
            assert file.read_text() == '// preserved class Saved {}\n'
            assert legacy.read_bytes() == b'synthetic H2 sentinel'
            assert not (base / 'database/credentials-v1.json').exists()
            cases.append({'case': 'settings-native-clear-rebuild-and-return-to-editor', 'java_required': False, 'legacy_database_retained': True, **result})
    (ROOT / 'docs/parity/Z02-index-pty.json').write_text(json.dumps({'cases': cases, 'passed': len(cases)}, indent=2) + '\n')
    print(f'Passed {len(cases)} real native index settings PTYs')


if __name__ == '__main__':
    main()
