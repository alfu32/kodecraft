#!/usr/bin/env python3
"""Compile all workspace targets in an owned Ubuntu container with no JDK/Maven."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
from distribution_notices import ROOT

IMAGE='ubuntu@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55'


def main():
    sysroot=Path(subprocess.check_output(['rustc','+stable','--print','sysroot'],text=True).strip()).resolve()
    registry=Path(os.environ.get('CARGO_HOME',str(Path.home()/'.cargo')))/'registry'
    script='''set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends build-essential pkg-config cmake libssl-dev zlib1g-dev ca-certificates
if command -v java || command -v javac || command -v mvn; then exit 1; fi
rustc --version
cargo check --locked --workspace --all-targets
'''
    command=['docker','run','--rm','--network','host','--mount',f'type=bind,source={ROOT},target=/src,readonly','--mount',f'type=bind,source={sysroot},target=/opt/rust,readonly','--mount',f'type=bind,source={registry},target=/tmp/cargo/registry,readonly','--workdir','/src','--env','PATH=/opt/rust/bin:/usr/sbin:/usr/bin:/sbin:/bin','--env','CARGO_HOME=/tmp/cargo','--env','CARGO_TARGET_DIR=/tmp/target','--env','CARGO_NET_OFFLINE=true',IMAGE,'/bin/sh','-c',script]
    log=Path(tempfile.gettempdir())/('kode-rs-no-java-build-'+str(os.getuid())+'.log')
    print('Checking all Rust targets in a clean image without Java; log: '+str(log),flush=True)
    with log.open('w') as output:
        result=subprocess.run(command,stdout=output,stderr=subprocess.STDOUT)
    if result.returncode:
        print(log.read_text()[-5000:]);raise SystemExit(result.returncode)
    evidence={'status':'passed','image':IMAGE,'rustc':subprocess.check_output(['rustc','+stable','--version'],text=True).strip(),'command':'cargo check --locked --workspace --all-targets','javaAbsent':True,'javacAbsent':True,'mavenAbsent':True,'sourceReadOnly':True,'cargoRegistryReadOnly':True,'userCargoCredentialsMounted':False}
    (ROOT/'docs/parity/Z03-no-java-build.json').write_text(json.dumps(evidence,indent=2)+'\n')
    print('All Rust workspace targets compile without Java or Maven',flush=True)


if __name__=='__main__':main()
