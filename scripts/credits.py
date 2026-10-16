#!/usr/bin/env python3
"""Generate dependency credits from locked Cargo metadata without local paths."""
import argparse
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / 'resources/credits-rust.json'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    command=['cargo', 'metadata', '--locked', '--format-version', '1']
    if args.offline: command.append('--offline')
    metadata = json.loads(subprocess.check_output(command, cwd=ROOT))
    workspace = set(metadata['workspace_members'])
    packages = [{field: package.get(field) for field in ['name', 'version', 'license', 'repository', 'homepage']}
                for package in metadata['packages'] if package['id'] not in workspace]
    packages.sort(key=lambda package: (package['name'], package['version']))
    text = json.dumps({'scope': 'Locked Rust dependencies, including build/test and target dependencies', 'packages': packages}, indent=2) + '\n'
    if args.check:
        if not OUTPUT.exists() or OUTPUT.read_text() != text:
            raise SystemExit('Dependency credits are stale; run python3 scripts/credits.py')
    else:
        OUTPUT.write_text(text)
    print(f'Checked {len(packages)} locked Rust dependency credits')


if __name__ == '__main__':
    main()
