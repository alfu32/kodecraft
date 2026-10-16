#!/usr/bin/env python3
"""Actual AppImage launch, no-Java editing and external-Java JDBC in real PTYs."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from distribution_notices import ROOT, sha


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--appimage",type=Path,required=True)
    parser.add_argument("--repro",type=Path)
    args=parser.parse_args()
    artifact=args.appimage.resolve(strict=True)
    manifest=json.loads(artifact.with_name(artifact.name+'.manifest.json').read_text())
    assert sha(artifact.read_bytes())==manifest['sha256']
    if args.repro:assert sha(args.repro.read_bytes())==manifest['sha256']
    spec=importlib.util.spec_from_file_location('db_terminal',ROOT/'scripts/test-database-terminal.py')
    database=importlib.util.module_from_spec(spec);spec.loader.exec_module(database)
    cases=[]
    with tempfile.TemporaryDirectory(prefix='kode-appimage-pty-') as directory:
        base=Path(directory)
        installed=base/'installation with spaces';installed.mkdir()
        executable=installed/'Kode.AppImage';shutil.copy2(artifact,executable)
        database.terminal.BINARY=executable
        # Source helper overrides are excluded. Discovery must find the JAR
        # inside the actual mounted/extracted image beside its native executable.
        os.environ.pop('KODE_JDBC_HELPER',None)
        os.environ.pop('APPIMAGE_EXTRACT_AND_RUN',None)
        for extracted in (False,True):
            for has_java in (False,True):
                case=base/f'{extracted}-{has_java}'
                project=case/'project with spaces';project.mkdir(parents=True)
                note=project/'Note.txt';note.write_text('image core original\n')
                db=case/'db';db.mkdir()
                (db/'datasources-v1.json').write_text(json.dumps({'version':1,'dataSources':[{'id':'fixture','name':'Image SQL','driver':{'id':'h2','driverClass':'org.h2.Driver'},'jdbcUrl':'jdbc:h2:mem:image_sql','username':'sa'}]}))
                catalog=case/'lsp.json';catalog.write_text('{"servers":[]}')
                env={'KODE_JAVA':shutil.which('java') if has_java else str(case/'absent-java'),'KODE_RS_DATABASE_HOME':str(db),'KODE_RS_JDBC_HOME':str(case/'plugins'),'KODE_LSP_CATALOG':str(catalog),'KODE_LSP_HOME':str(case/'lsp')}
                if extracted:env['APPIMAGE_EXTRACT_AND_RUN']='1'
                actions=[(b'Ready',.15,b'\x1b4'),(b'Image SQL',.15,b'c')]
                if has_java:
                    actions += [(b'IMAGE_SQL',.15,b's'),(b'Execute',.15,b'\x01'+database.paste('select 42 as APPIMAGE_PROOF;')),(b'APPIMAGE_PROOF',.15,b'\x1b[13;5u'),(b'Completed',.15,b'\x11')]
                    expected=[b'Completed']
                else:
                    actions += [(b'F5 Get Java',.15,b'\x1b1\x1b[17~\x01'+database.paste('image core saved\n')+b'\x13'),(b'image core saved',.15,b'\x11')]
                    expected=[b'F5 Get Java',b'image core saved']
                # F6 is CSI 17~. The sidebar remains focused after Alt+1.
                result=database.terminal.run_case(note,case/'config',(100,34),actions,expected=expected,env_overrides=env,timeout_seconds=20)
                assert note.read_text()==('image core original\n' if has_java else 'image core saved\n')
                assert not (db/'credentials-v1.json').exists() and not (db/'java-runtime').exists()
                cases.append({'case':'external-java-h2' if has_java else 'no-java-edit-save','launch':'extract-and-run' if extracted else 'fuse','adjacentHelper':has_java,'installationPathWithSpaces':True,**result})
                print('Passed AppImage PTY',cases[-1]['launch'],cases[-1]['case'],flush=True)
    report={'status':'passed','artifact':artifact.name,'sha256':manifest['sha256'],'reproducible':bool(args.repro),'runtimeVersion':manifest['runtimeVersion'],'cases':cases,'passed':len(cases)}
    (ROOT/'docs/parity/Z03-appimage-pty.json').write_text(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':main()
