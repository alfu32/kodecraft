#!/usr/bin/env python3
"""Build the locked stable Rust executable with portable diagnostic source paths."""
import os
from pathlib import Path
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[1]
environment = dict(os.environ)
flags = environment.get("CARGO_ENCODED_RUSTFLAGS", "").split("\x1f") if environment.get("CARGO_ENCODED_RUSTFLAGS") else shlex.split(environment.get("RUSTFLAGS", ""))
# Apply broad mapping first, then the project-specific one. No developer build
# directory or Cargo/Rustup home is part of the installed runtime contract.
flags.extend(["--remap-path-prefix=" + str(Path.home()) + "=/build/home", "--remap-path-prefix=" + str(ROOT) + "=/src/kode"])
environment.pop("RUSTFLAGS", None)
environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
if os.name != "nt":
    c_flags = shlex.join(["-ffile-prefix-map=" + str(Path.home()) + "=/build/home", "-ffile-prefix-map=" + str(ROOT) + "=/src/kode"])
    for variable in ("CFLAGS", "CXXFLAGS"):
        environment[variable] = (environment.get(variable, "") + " " + c_flags).strip()
subprocess.run(["cargo", "+stable", "build", "--release", "--locked", "-p", "kode", "--bin", "kode"], cwd=ROOT, env=environment, check=True)
