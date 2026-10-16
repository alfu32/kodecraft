#!/usr/bin/env python3
"""Build the pinned standalone ttyd executable with native development libraries.

Requires CMake, a C compiler, libuv, libwebsockets (with libuv), json-c, zlib,
and optional OpenSSL development packages. This never builds JNI or a shared
library launcher. Release packaging must include/declare native runtime libraries.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
REVISION = 'a8ae135643c0ed7164f59f14f537b0b11f7a9788'
URL = f'https://codeload.github.com/alfu32/ttyd/tar.gz/{REVISION}'
SHA256 = '5b3092f1de2c9e472cf85ea32ae8bab3d91f04945134429478224dea13968524'


def run(*args, env=None):
    subprocess.run([str(value) for value in args], check=True, env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-dir', type=Path, default=ROOT / 'target/ttyd-native')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/ttyd-bundle')
    parser.add_argument('--cmake-prefix', type=Path, help='Optional native dependency installation prefix')
    parser.add_argument('--jobs', type=int, default=min(os.cpu_count() or 1, 8))
    args = parser.parse_args()
    build = args.build_dir.resolve()
    output = args.output.resolve()
    build.mkdir(parents=True, exist_ok=True)
    archive = build / f'ttyd-{REVISION}.tar.gz'
    if not archive.exists():
        with urllib.request.urlopen(URL, timeout=60) as response:
            archive.write_bytes(response.read())
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise SystemExit('ttyd source checksum mismatch')
    source = build / f'ttyd-{REVISION}'
    if not source.exists():
        with tarfile.open(archive) as tar:
            tar.extractall(build, filter='data')
    configure = ['cmake', '-S', source, '-B', build / 'cmake', '-DCMAKE_BUILD_TYPE=Release',
                 '-DTTYD_BUILD_SHARED=OFF', '-DTTYD_BUILD_JNI=OFF', '-DCMAKE_INSTALL_PREFIX=' + str(output)]
    if args.cmake_prefix:
        configure.append('-DCMAKE_PREFIX_PATH=' + str(args.cmake_prefix.resolve()))
    # An extracted source below target/ must not borrow Kode's ancestor Git tag.
    run(*configure, env=dict(os.environ, GIT_CEILING_DIRECTORIES=str(source.parent)))
    run('cmake', '--build', build / 'cmake', '--parallel', args.jobs)
    run('cmake', '--install', build / 'cmake')
    executable = output / 'bin' / ('ttyd.exe' if platform.system() == 'Windows' else 'ttyd')
    version = subprocess.check_output([str(executable), '--version'], text=True).strip()
    notices = output / 'share/licenses/ttyd'
    notices.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source / 'LICENSE', notices / 'LICENSE')
    manifest = {
        'component': 'ttyd', 'version_output': version, 'source_url': URL,
        'source_revision': REVISION, 'source_sha256': SHA256,
        'executable_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
        'host': {'os': platform.system(), 'architecture': platform.machine()},
        'kind': 'standalone-executable', 'jni': False, 'shared_launcher': False,
        'runtime_dependencies': 'Native libwebsockets event library, json-c, zlib, OpenSSL and C runtime as linked; qualify release bundle separately.',
    }
    (output / 'ttyd-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'Built {version}; executable and provenance at {output}')


if __name__ == '__main__':
    main()
