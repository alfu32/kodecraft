#!/usr/bin/env python3
"""Check literal serve argv, including executable/workspace paths with spaces."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/debug/kode'


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix='kode-serve-cli-') as directory:
        base = Path(directory)
        workspace = base / 'workspace with spaces'
        workspace.mkdir()
        fake = base / 'fixture ttyd'
        record = base / 'argv.json'
        fake.write_text(f'#!{sys.executable}\nimport json,os,sys\nfrom pathlib import Path\nPath(os.environ["ARGV_TRACE"]).write_text(json.dumps({{"args":sys.argv[1:],"cwd":os.getcwd(),"pid":os.getpid()}}))\nsys.exit(17)\n')
        fake.chmod(0o755)
        env = dict(os.environ, KODE_TTYD=str(fake), ARGV_TRACE=str(record))
        for trailing in [[], ['--', '--public', '-t', 'fontFamily=a b'], ['--unknown', '$(literal)']]:
            args = ['serve', '--post=11046', '-c', 'fixture:synthetic password', '--resources', 'resource root', str(workspace), *trailing]
            child = subprocess.Popen([str(BINARY), *args], cwd=base, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            stdout, stderr = child.communicate(timeout=10)
            assert child.returncode == 17, stderr
            trace = json.loads(record.read_text())
            assert trace['cwd'] == str(workspace)
            if sys.platform != 'win32':
                assert trace['pid'] == child.pid, 'Unix gateway retained an intermediate wrapper'
            expected = ['-p', '11046', '-i', '127.0.0.1', '-W', '-T', 'xterm-256color', '-w', str(workspace), '-t', 'titleFixed=Kode', '-d', '3', '-c', 'fixture:synthetic password']
            expected.extend(trailing[1:] if trailing[:1] == ['--'] else trailing)
            expected.extend(['--', str(BINARY.resolve()), '--resources', str(base / 'resource root'), str(workspace)])
            assert trace['args'] == expected, trace['args']
            assert b'synthetic password' not in stdout + stderr
            cases.append({'case': 'literal-argv-and-exit-status', 'forwarded_args': len(trailing), 'status': 'passed'})
        missing = subprocess.run([str(BINARY), 'serve', str(workspace)], env=dict(env, KODE_TTYD=str(base / 'missing')), capture_output=True)
        assert missing.returncode == 1 and b'KODE_TTYD' in missing.stderr
        invalid = subprocess.run([str(BINARY), 'serve', '--credential', 'synthetic-secret'], env=env, capture_output=True)
        assert invalid.returncode == 1 and b'synthetic-secret' not in invalid.stderr
        cases.append({'case': 'missing-launcher-invalid-credential', 'status': 'passed'})
    (ROOT / 'docs/parity/Z01-cli.json').write_text(json.dumps({'cases': cases, 'passed': len(cases)}, indent=2) + '\n')
    print(f'Passed {len(cases)} serve CLI/process checks')


if __name__ == '__main__':
    main()
