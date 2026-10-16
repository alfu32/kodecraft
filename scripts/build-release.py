#!/usr/bin/env python3
"""Build the locked stable Rust executable with portable diagnostic source paths."""
import os
import argparse
import platform
from pathlib import Path
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--target-dir", type=Path, help="Isolate qualification builds from a developer's active target directory")
args = parser.parse_args()
environment = dict(os.environ)
if args.target_dir:
    environment["CARGO_TARGET_DIR"] = str(args.target_dir.resolve())
flags = environment.get("CARGO_ENCODED_RUSTFLAGS", "").split("\x1f") if environment.get("CARGO_ENCODED_RUSTFLAGS") else shlex.split(environment.get("RUSTFLAGS", ""))
# Apply broad mapping first, then the project-specific one. No developer build
# directory or Cargo/Rustup home is part of the installed runtime contract.
flags.extend(["--remap-path-prefix=" + str(Path.home()) + "=/build/home", "--remap-path-prefix=" + str(ROOT) + "=/src/kode"])
if args.target_dir:
    flags.append("--remap-path-prefix=" + str(args.target_dir.resolve()) + "=/build/target")
if platform.system()=="Linux" and platform.machine()=="x86_64":
    flags.extend(["-C", "target-cpu=x86-64"])
environment.pop("RUSTFLAGS", None)
environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
if os.name != "nt":
    c_flags = shlex.join(["-ffile-prefix-map=" + str(Path.home()) + "=/build/home", "-ffile-prefix-map=" + str(ROOT) + "=/src/kode"])
    if args.target_dir:
        c_flags += " " + shlex.quote("-ffile-prefix-map=" + str(args.target_dir.resolve()) + "=/build/target")
    if platform.system()=="Linux" and platform.machine()=="x86_64":
        c_flags += " -march=x86-64 -mtune=generic"
    for variable in ("CFLAGS", "CXXFLAGS"):
        environment[variable] = (environment.get(variable, "") + " " + c_flags).strip()
subprocess.run(["cargo", "+stable", "build", "--release", "--locked", "-p", "kode", "--bin", "kode"], cwd=ROOT, env=environment, check=True)
