#!/usr/bin/env python3
"""Record reviewable source/compiler/native inputs for one native distribution build."""
import argparse
import json
from pathlib import Path
import subprocess
from distribution_notices import ROOT, sha
from native_bundle import glibc


def output(command):
    return subprocess.check_output(command,cwd=ROOT,text=True).strip()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=ROOT/'target/release/kode')
    parser.add_argument('--native-bundle',type=Path,default=ROOT/'target/native-release-bundle')
    parser.add_argument('--native-build-root',type=Path,default=ROOT/'target')
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    native=json.loads((args.native_bundle/'native-manifest.json').read_text())
    revision=output(['git','rev-parse','HEAD'])
    changed=subprocess.check_output(['git','status','--porcelain=v1','-z','--untracked-files=normal'],cwd=ROOT,text=True)
    # Generated acceptance reports do not change shipped implementation. Source
    # edits still make the review ineligible for automatic draft creation.
    records=iter(changed.split('\0'))
    changed_paths=[]
    for record in records:
        if not record:continue
        paths=[record[3:]]
        if 'R' in record[:2] or 'C' in record[:2]:paths.append(next(records))
        changed_paths.extend(path for path in paths if not path.startswith(('docs/parity/','DASHBOARD.md','docs/migration/')))
    libraries={}
    cache=output(['ldconfig','-p'])
    import re
    paths={match[1]:Path(match[2]).resolve() for line in cache.splitlines() if (match:=re.match(r'\s*(\S+) \(.*x86-64.*\) => (.*)',line))}
    for entry in native['elf']:
        name=Path(entry['file']).name
        if name not in paths:continue
        if sha(paths[name].read_bytes())!=entry['originalSha256']:
            raise ValueError('Native host library changed since bundle creation: '+name)
        package=output(['dpkg-query','-S',str(paths[name])]).split(': ',1)[0]
        fields=output(['dpkg-query','--show','--showformat=${binary:Package}\t${Version}\t${source:Package}\t${source:Version}',package]).split('\t')
        libraries[name]={'binaryPackage':fields[0],'version':fields[1],'sourcePackage':fields[2],'sourceVersion':fields[3],'originalSha256':entry['originalSha256']}
    document={'schemaVersion':1,'sourceRevision':revision,'sourceDirty':bool(changed_paths),'changedSourcePaths':changed_paths,'rustc':output(['rustc','+stable','--version','--verbose']),'cargo':output(['cargo','+stable','--version']),'cc':output(['cc','--version']).splitlines()[0],'target':'x86_64-unknown-linux-gnu','minimumGlibc':max([glibc(args.binary),native['minimumGlibc']],key=lambda value:tuple(map(int,value.split('.')))),'binarySha256':sha(args.binary.read_bytes()),'cargoLockSha256':sha((ROOT/'Cargo.lock').read_bytes()),'nativeManifestSha256':sha((args.native_bundle/'native-manifest.json').read_bytes()),'nativeLibraries':libraries,'ttyd':json.loads((args.native_bundle/'notices/native/ttyd/build-manifest.json').read_text())}
    cache=(args.native_build_root/'ttyd-native-built-in/cmake/CMakeCache.txt').read_text()
    def cached(key):
        match=re.search(r'^'+re.escape(key)+r':[^=]+=(.+)$',cache,re.MULTILINE)
        if not match:raise ValueError('Missing native build provenance: '+key)
        return Path(match[1])
    header=(cached('LIBUV_INCLUDE_DIR')/'uv/version.h').read_text()
    version='.'.join(re.search(r'^#define UV_VERSION_'+part+r'\s+(\d+)',header,re.MULTILINE)[1] for part in ('MAJOR','MINOR','PATCH'))
    library=cached('LIBUV_LIBRARY').resolve(strict=True)
    document['libuv']={'version':version,'linkedLibrarySha256':sha(library.read_bytes()),'linkage':'static' if library.suffix=='.a' else 'shared','sourceArchive':'https://dist.libuv.org/dist/v'+version+'/libuv-v'+version+'.tar.gz'}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(document,sort_keys=True,indent=2)+'\n')
    print('Recorded build source/compiler provenance; sourceDirty='+str(document['sourceDirty']),flush=True)


if __name__=='__main__':main()
