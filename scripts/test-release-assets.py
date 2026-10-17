#!/usr/bin/env python3
"""Exercise release integrity and draft-only CLI behavior without a remote write.

Uses actual assembled archives/installers/runtime. Clean-source metadata for the
positive cases is explicitly synthetic; it is never eligible for publication.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from distribution_notices import ROOT, sha


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive-directory',type=Path,required=True)
    parser.add_argument('--installer-directory',type=Path,help='Omit when installers already share the archive directory')
    parser.add_argument('--runtime-directory',type=Path,required=True)
    parser.add_argument('--provenance',type=Path,required=True)
    args=parser.parse_args()
    spec=importlib.util.spec_from_file_location('release_assets',ROOT/'scripts/release-assets.py')
    release=importlib.util.module_from_spec(spec);spec.loader.exec_module(release)
    revision=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()
    cases=[]
    with tempfile.TemporaryDirectory(prefix='kode-release-tests-') as directory:
        root=Path(directory);inputs=root/'incoming';inputs.mkdir()
        for source in (args.archive_directory,args.installer_directory,args.runtime_directory):
            if source is None:continue
            for path in source.iterdir():
                if path.is_file():shutil.copyfile(path,inputs/path.name)
        provenance=json.loads(args.provenance.read_text())
        version=next(inputs.glob('*.AppImage')).name.removeprefix('kode-').removesuffix('-x86_64.AppImage')
        tag=version
        # Real local dirty provenance must be rejected. The harness's positive
        # path changes this field in a temporary copy only, with no upload.
        provenance['sourceDirty']=True
        source=inputs/'build-source.json';source.write_text(json.dumps(provenance))
        def rejected(name,reason,output=None,release_tag=tag):
            destination=output or root/('review-'+name)
            try:release.prepare(inputs,destination,release_tag,revision)
            except ValueError as error:
                assert reason in str(error),(name,str(error))
            else:raise AssertionError('Accepted invalid release: '+name)
            cases.append({'case':name,'status':'passed'})
        rejected('modified-source','modified source')
        provenance.update(sourceDirty=False,sourceRevision=revision,changedSourcePaths=[])
        source.write_text(json.dumps(provenance))
        review=release.prepare(inputs,root/'valid-review',tag,revision)
        assert len(review['assets'])==13 and len(list((root/'valid-review').iterdir()))==16
        cases.append({'case':'complete-actual-assets-synthetic-clean-source','status':'passed','assets':13})
        rejected('wrong-version','compiled package version',release_tag='99.0.0')
        provenance['sourceRevision']='0'*40;source.write_text(json.dumps(provenance))
        rejected('different-source','source revision differs')
        provenance['sourceRevision']=revision;source.write_text(json.dumps(provenance))
        checksum=next(inputs.glob('*.AppImage.sha256'));original=checksum.read_bytes()
        checksum.write_text('0'*64+'  '+checksum.name.removesuffix('.sha256')+'\n')
        rejected('bad-checksum','checksum differs');checksum.write_bytes(original)
        runtime=next(inputs.glob('*.zip'));runtime.rename(root/runtime.name)
        rejected('missing-runtime','one main archive');(root/runtime.name).rename(runtime)
        duplicate=inputs/'duplicate';duplicate.mkdir();shutil.copyfile(checksum,duplicate/checksum.name)
        rejected('duplicate-name','duplicate release asset');shutil.rmtree(duplicate)
        stale=root/'stale-review';stale.mkdir();(stale/'unexpected.txt').write_text('fixture')
        rejected('stale-output','new or empty',output=stale)
        rejected('nested-output','outside the asset',output=inputs/'output')
        # PATH contains an owned gh stub. It records argv and models only the
        # responses required by the publication adapter; no real gh/network runs.
        tools=root/'bin';tools.mkdir();calls=root/'gh-calls.jsonl'
        fake=tools/'gh'
        fake.write_text('#!'+sys.executable+'''\nimport json,os,sys
from pathlib import Path
args=sys.argv[1:]
with Path(os.environ['FAKE_GH_CALLS']).open('a') as log:log.write(json.dumps(args)+'\\n')
mode=os.environ['FAKE_GH_MODE']
if args[:1]==['api']:
 if '/commits/' in args[1]:
  print(json.dumps({'sha':'0'*40 if mode=='wrong-tag' else os.environ['FAKE_GH_REVISION']}));sys.exit(0)
 if mode=='existing':print('{}');sys.exit(0)
 print('HTTP 403' if mode=='forbidden' else 'HTTP 404',file=sys.stderr);sys.exit(1)
if args[:2]==['release','create']:sys.exit(0)
raise SystemExit('Unexpected fake gh operation')
''');fake.chmod(0o700)
        for mode in ('wrong-tag','existing','forbidden','new-draft'):
            calls.write_text('')
            env=dict(os.environ,PATH=str(tools)+os.pathsep+os.environ['PATH'],GH_TOKEN='synthetic-fixture-only',FAKE_GH_CALLS=str(calls),FAKE_GH_MODE=mode,FAKE_GH_REVISION=revision)
            result=subprocess.run([sys.executable,str(ROOT/'scripts/publish-draft-release.py'),'--directory',str(inputs),'--output',str(root/('draft-'+mode)),'--tag',tag,'--repository','fixture/owned-test'],env=env,capture_output=True,text=True)
            recorded=[json.loads(line) for line in calls.read_text().splitlines()]
            mutations=[call for call in recorded if call[:2]==['release','create']]
            if mode=='new-draft':
                assert result.returncode==0,result.stderr
                assert len(mutations)==1
                command=mutations[0]
                assert all(flag in command for flag in ('--draft','--verify-tag','--notes-file','--target'))
                assert command[command.index('--target')+1]==revision
                assert '--clobber' not in command
            else:
                assert result.returncode!=0 and not mutations,(mode,result.stdout,result.stderr,recorded)
            cases.append({'case':'mock-gh-'+mode,'status':'passed','realRemoteWrites':0})
    evidence={'status':'passed','actualArtifactIntegrity':True,'syntheticCleanSourceForPositiveCases':True,'realRemoteWrites':0,'passed':len(cases),'cases':cases}
    (ROOT/'docs/parity/Z03-release-preflight.json').write_text(json.dumps(evidence,indent=2)+'\n')
    print(json.dumps(evidence,indent=2))


if __name__=='__main__':main()
