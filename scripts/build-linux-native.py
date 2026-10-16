#!/usr/bin/env python3
"""Build pinned libwebsockets with built-in libuv, then standalone ttyd for release.

Requires native CMake/compiler/OpenSSL/json-c/zlib/libuv/libcap development inputs.
An extracted development-package prefix may be supplied without OS installation.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from distribution_notices import ROOT, sha

VERSION = "4.3.3"
URL = "https://codeload.github.com/warmcat/libwebsockets/tar.gz/refs/tags/v4.3.3"
SHA256 = "6fd33527b410a37ebc91bb64ca51bdabab12b076bc99d153d7c5dd405e4bdf90"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dependency-prefix", type=Path)
    parser.add_argument("--build-root", type=Path, default=ROOT / "target", help="Keep native qualification state separate from other Cargo builds")
    parser.add_argument("--output", type=Path, default=ROOT / "target/ttyd-release-bundle")
    parser.add_argument("--jobs", type=int, default=min(os.cpu_count() or 1,8))
    args = parser.parse_args()
    build = args.build_root / "native-release-build"
    dependency = args.build_root / "native-release-deps"
    build.mkdir(parents=True,exist_ok=True)
    archive = args.build_root / "native-sources/libwebsockets-4.3.3.tar.gz"
    archive.parent.mkdir(parents=True,exist_ok=True)
    if not archive.exists():
        with urllib.request.urlopen(URL,timeout=30) as response:
            archive.write_bytes(response.read(32*1024*1024))
    if sha(archive.read_bytes()) != SHA256:
        raise ValueError("libwebsockets source checksum mismatch")
    source = build / ("libwebsockets-" + VERSION)
    if not source.exists():
        with tarfile.open(archive) as payload:
            payload.extractall(build,filter="data")
    log_path = build / "build.log"
    with log_path.open("w") as log:
        def run(command):
            print("Building " + str(command[0]),flush=True)
            log.write(json.dumps(list(map(str,command))) + "\n");log.flush()
            result = subprocess.run(list(map(str,command)),stdout=log,stderr=subprocess.STDOUT,env=dict(os.environ,GIT_CEILING_DIRECTORIES=str(source.parent)))
            if result.returncode:
                raise RuntimeError(f"Native build failed; inspect {log_path}")
        configure = ["cmake","-S",source,"-B",build / "cmake","-DCMAKE_BUILD_TYPE=Release","-DCMAKE_INSTALL_PREFIX="+str(dependency),"-DCMAKE_INSTALL_LIBDIR=lib","-DLWS_WITH_LIBUV=ON","-DLWS_WITH_EVLIB_PLUGINS=OFF","-DLWS_WITH_STATIC=ON","-DLWS_WITH_SHARED=OFF","-DLWS_WITHOUT_TESTAPPS=ON","-DLWS_WITH_LIBEV=OFF","-DLWS_WITH_GLIB=OFF","-DLWS_WITH_LIBEVENT=OFF","-DCMAKE_C_FLAGS=-ffile-prefix-map="+str(source)+"=/src/libwebsockets"]
        if args.dependency_prefix:
            configure.append("-DCMAKE_PREFIX_PATH="+str(args.dependency_prefix.resolve()))
        run(configure)
        run(["cmake","--build",build / "cmake","--parallel",args.jobs])
        run(["cmake","--install",build / "cmake"])
        # libwebsockets 4.3.3's installed config lists both targets even for a
        # static-only build. Select the target that this build actually exports.
        config = dependency / "lib/cmake/libwebsockets/libwebsockets-config.cmake"
        configured = config.read_text()
        old = "set(LIBWEBSOCKETS_LIBRARIES websockets websockets_shared)"
        if old not in configured:
            raise ValueError("Unexpected pinned libwebsockets CMake config")
        config.write_text(configured.replace(old,"set(LIBWEBSOCKETS_LIBRARIES websockets)"))
        command = [sys.executable, ROOT / "scripts/build-ttyd.py","--build-dir",args.build_root / "ttyd-native-built-in","--output",args.output,"--cmake-prefix",dependency,"--builtin-libuv","--jobs",args.jobs]
        if args.dependency_prefix:
            command.extend(["--extra-cmake-prefix",args.dependency_prefix])
        run(command)
    output = args.output.resolve()
    notices = output / "share/licenses/libwebsockets"
    notices.mkdir(parents=True,exist_ok=True)
    shutil.copyfile(source / "LICENSE",notices / "LICENSE")
    if args.dependency_prefix:
        document = args.dependency_prefix / "share/doc/libwebsockets-dev/copyright"
        if document.is_file():
            shutil.copyfile(document,notices / "debian-copyright")
    uv_document = args.dependency_prefix / "share/doc/libuv1-dev/copyright" if args.dependency_prefix else Path("/usr/share/doc/libuv1-dev/copyright")
    if not uv_document.is_file():
        raise ValueError("libuv development copyright text is required")
    uv_notices = output / "share/licenses/libuv"
    uv_notices.mkdir(parents=True,exist_ok=True)
    shutil.copyfile(uv_document,uv_notices / "copyright")
    manifest_path = output / "ttyd-manifest.json"
    manifest = json.loads(manifest_path.read_text())
    manifest["staticLibwebsockets"] = {"version":VERSION,"sourceUrl":URL,"sourceSha256":SHA256,"eventLoopPlugins":False}
    manifest["pathRemapping"] = True
    manifest_path.write_text(json.dumps(manifest,sort_keys=True,indent=2)+"\n")
    print("Built release ttyd with built-in libuv at " + str(output),flush=True)


if __name__ == "__main__":
    main()
