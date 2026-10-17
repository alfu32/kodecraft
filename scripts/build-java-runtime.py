#!/usr/bin/env python3
"""Build a standalone, checksummed JDBC JVM image; never bundle app/driver JARs."""
import argparse
from datetime import datetime,timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import stat
import subprocess
import tempfile
import zipfile

# Start with jlink-build/bundle.sh's JDBC/auth/TLS baseline. Keep optional driver
# installation separate; these additional JDK modules serve common driver needs.
MODULES=sorted(['java.sql','java.sql.rowset','java.naming','java.logging','java.management','java.xml','java.security.sasl','java.security.jgss','java.transaction.xa','java.net.http','java.instrument','jdk.crypto.ec','jdk.charsets','jdk.unsupported','jdk.naming.dns'])


def target():
    arch={'x86_64':'x86_64','AMD64':'x86_64','aarch64':'aarch64','arm64':'aarch64','ARM64':'aarch64'}.get(platform.machine())
    suffix={'Linux':'unknown-linux-gnu','Darwin':'apple-darwin','Windows':'pc-windows-msvc'}.get(platform.system())
    if not arch or not suffix:raise SystemExit('Unsupported native JVM artifact target')
    return arch+'-'+suffix


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--jdk',type=Path,help='JDK home (defaults to the jlink executable on PATH)')
    parser.add_argument('--output',type=Path,default=Path('target/java-runtime-artifacts'))
    parser.add_argument('--target',default=target())
    args=parser.parse_args()
    if args.target!=target():raise SystemExit('Build the runtime on its native target; cross-labeling is not supported')
    jlink=shutil.which('jlink')
    if not args.jdk and not jlink:raise SystemExit('A Java 21+ JDK with jlink is required')
    jdk=(args.jdk or Path(jlink).resolve().parent.parent).resolve()
    extension='.exe' if platform.system()=='Windows' else ''
    jlink=jdk/'bin'/('jlink'+extension)
    if not jlink.is_file() or not (jdk/'jmods').is_dir():raise SystemExit('The selected JDK needs bin/jlink and jmods')
    env=dict(os.environ)
    for name in ['JAVA_TOOL_OPTIONS','_JAVA_OPTIONS','JDK_JAVA_OPTIONS','CLASSPATH']:env.pop(name,None)
    version=subprocess.check_output([str(jlink),'--version'],text=True,env=env).strip()
    major=int(version.split('.')[0].split('-')[0])
    if major<21:raise SystemExit('Java 21 or newer is required')
    args.output.mkdir(parents=True,exist_ok=True)
    timestamp=datetime.fromtimestamp(int(os.environ.get('SOURCE_DATE_EPOCH','1767225600')),timezone.utc)
    if not 1980<=timestamp.year<=2107:raise SystemExit('SOURCE_DATE_EPOCH must be in the ZIP timestamp range')
    stamp=(timestamp.year,timestamp.month,timestamp.day,timestamp.hour,timestamp.minute,timestamp.second)
    name=f'kode-jdbc-runtime-{args.target}'
    with tempfile.TemporaryDirectory(prefix='kode-jlink-') as directory:
        runtime=Path(directory)/'runtime'
        subprocess.run([str(jlink),'--add-modules',','.join(MODULES),'--strip-debug','--no-header-files','--no-man-pages','--compress=2','--output',str(runtime)],env=env,check=True)
        java=runtime/'bin'/('java'+extension)
        subprocess.run([str(java),'-version'],env=env,check=True,capture_output=True)
        modules=sorted(line.split('@')[0] for line in subprocess.check_output([str(java),'--list-modules'],text=True,env=env).splitlines())
        if not set(MODULES)<=set(modules):raise SystemExit('Generated runtime is missing requested modules')
        files={}
        for path in sorted(runtime.rglob('*')):
            if not path.is_file():continue
            relative=path.relative_to(runtime).as_posix()
            data=path.read_bytes()
            files[relative]={'sha256':hashlib.sha256(data).hexdigest(),'size':len(data),'executable':bool(path.stat().st_mode&0o111)}
        if not any(path.startswith('legal/') for path in files):raise SystemExit('Generated runtime is missing JDK legal notices')
        manifest={'schemaVersion':1,'target':args.target,'javaMajor':major,'javaVersion':version,'contents':'JDBC Java runtime only; application and driver artifacts are separate','modules':modules,'files':files}
        encoded=(json.dumps(manifest,indent=2,sort_keys=True)+'\n').encode()
        archive=args.output/(name+'.zip')
        with zipfile.ZipFile(archive,'w',compression=zipfile.ZIP_DEFLATED,compresslevel=6) as output:
            for relative,entry in files.items():
                info=zipfile.ZipInfo('runtime/'+relative,stamp)
                info.create_system=3
                info.external_attr=(stat.S_IFREG|(0o755 if entry['executable'] else 0o644))<<16
                info.compress_type=zipfile.ZIP_DEFLATED
                # Dereference trusted JDK legal-file symlinks into ordinary ZIP
                # files. The installer never needs to create links from an archive.
                output.writestr(info,(runtime/relative).read_bytes(),compresslevel=6)
            info=zipfile.ZipInfo('runtime-manifest.json',stamp)
            info.create_system=3;info.external_attr=(stat.S_IFREG|0o644)<<16
            info.compress_type=zipfile.ZIP_DEFLATED
            output.writestr(info,encoded,compresslevel=6)
        checksum=hashlib.sha256(archive.read_bytes()).hexdigest()
        (args.output/(name+'.zip.sha256')).write_text(checksum+'  '+archive.name+'\n')
        (args.output/(name+'.manifest.json')).write_bytes(encoded)
        print(json.dumps({'archive':str(archive),'target':args.target,'sha256':checksum,'compressedBytes':archive.stat().st_size,'runtimeBytes':sum(entry['size'] for entry in files.values()),'files':len(files),'javaVersion':version}))


if __name__=='__main__':main()
