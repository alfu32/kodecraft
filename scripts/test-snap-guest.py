#!/usr/bin/env python3
"""Install/test the actual strict Snap in an owned KVM guest, never on the host."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import socket
import subprocess
import tempfile
from distribution_notices import ROOT, sha
from snap_guest import Guest, IMAGE_URL, IMAGE_SHA256


def load(name,file):
    spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/file)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    return module


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--snap',type=Path,required=True)
    parser.add_argument('--runtime',type=Path,required=True)
    parser.add_argument('--chrome',default=shutil.which('google-chrome'))
    args=parser.parse_args()
    artifact=args.snap.resolve(strict=True)
    metadata=json.loads(artifact.with_name(artifact.name+'.manifest.json').read_text())
    assert metadata['sha256']==sha(artifact.read_bytes()) and metadata['confinement']=='strict'
    database=load('db_terminal','test-database-terminal.py')
    browser=load('browser_terminal','test-serve-browser.py')
    cases=[]
    with Guest() as guest, tempfile.TemporaryDirectory(prefix='kode-snap-acceptance-') as directory:
        base=Path(directory)
        guest.install(artifact)
        assert guest.run(['snap','debug','confinement']).stdout.strip()=='strict'
        assert artifact.name.split('_')[1] in guest.run(['/snap/bin/kode-rs','--version']).stdout
        versions={'snapd':guest.run(['snap','version']).stdout,'kernel':guest.run(['uname','-r']).stdout.strip()}
        project='/home/kode-uat/Project with spaces'
        note=project+'/Note.txt'
        db='/home/kode-uat/snap/kode-rs/common/database'
        guest.run(['mkdir','-p',project,db])
        local_note=base/'Note.txt';local_note.write_text('Snap core original\n')
        source={'version':1,'dataSources':[{'id':'fixture','name':'Snap SQL','driver':{'id':'h2','driverClass':'org.h2.Driver'},'jdbcUrl':'jdbc:h2:mem:snap_sql','username':'sa'}]}
        datasource=base/'datasources.json';datasource.write_text(json.dumps(source))
        guest.put(local_note,'/tmp/Note.txt');guest.put(datasource,'/tmp/datasources.json')
        guest.run(['cp','/tmp/Note.txt',note]);guest.run(['cp','/tmp/datasources.json',db+'/datasources-v1.json'])
        launch=base/'launch-snap'
        def wrapper(java):
            command=guest.argv(['env','KODE_JAVA='+java,'/snap/bin/kode-rs',note],tty=True)
            launch.write_text('#!'+__import__('sys').executable+'\nimport os\nos.execvp("ssh",'+repr(command)+')\n')
            launch.chmod(0o700)
            database.terminal.BINARY=launch
        wrapper('/not-installed/java')
        actions=[(b'Ready',.15,b'\x01'+database.paste('Snap core saved\n')+b'\x13'),(b'Snap core saved',.15,b'\x1b4'),(b'Snap SQL',.15,b'c'),(b'F5 Get Java',.15,b'\x11')]
        result=database.terminal.run_case(base/'dummy',base/'config-no-java',(100,34),actions,expected=[b'F5 Get Java'],timeout_seconds=30)
        assert guest.run(['cat',note]).stdout=='Snap core saved\n'
        cases.append({'case':'strict-snap-no-java-edit-save-and-offer',**result})
        print('Snap no-Java editor acceptance passed',flush=True)
        # A real separately generated JVM is copied only into the test user's
        # writable app data. No JVM is added to the Snap or its immutable mount.
        guest.put(args.runtime.resolve(),'/tmp/java-runtime.zip')
        runtime_root='/home/kode-uat/snap/kode-rs/common/external-java'
        extract="import zipfile,pathlib,sys; root=pathlib.Path(sys.argv[2]); root.mkdir(parents=True,exist_ok=True); z=zipfile.ZipFile(sys.argv[1]); z.extractall(root); [(root/i.filename).chmod(0o755 if (i.external_attr>>16)&0o111 else 0o644) for i in z.infolist() if not i.is_dir()]"
        guest.run(['python3','-c',extract,'/tmp/java-runtime.zip',runtime_root])
        java=runtime_root+'/runtime/bin/java'
        wrapper(java)
        actions=[(b'Ready',.15,b'\x1b4'),(b'Snap SQL',.15,b'c'),(b'SNAP_SQL',.15,b's'),(b'Execute',.15,b'\x01'+database.paste('select 57 as SNAP_PROOF;')),(b'SNAP_PROOF',.15,b'\x1b[13;5u'),(b'Completed',.15,b'\x11')]
        result=database.terminal.run_case(base/'dummy',base/'config-java',(100,34),actions,expected=[b'Completed'],timeout_seconds=30)
        check="import pathlib,sys; target=sys.argv[1].encode(); found=[]\nfor path in pathlib.Path('/proc').glob('[0-9]*/cmdline'):\n try:\n  if path.read_bytes().split(b'\\0')[0]==target: found.append(str(path))\n except (FileNotFoundError,PermissionError,ProcessLookupError): pass\nassert not found,found"
        guest.run(['python3','-c',check,java])
        guest.run(['test','!','-e',db+'/credentials-v1.json'])
        cases.append({'case':'strict-snap-external-java-real-h2-sql','helperReaped':True,'javaOutsideSnap':True,**result})
        print('Snap external-Java SQL acceptance passed',flush=True)
        # Actual packaged browser mode through an SSH forward into the guest.
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1',0));port=reservation.getsockname()[1]
        pidfile='/tmp/kode-uat-serve.pid'
        server_command=['env','KODE_JAVA=/not-installed/java','/snap/bin/kode-rs','serve','--host','127.0.0.1','--port','11045',project]
        remote=['sh','-c','echo $$ > '+pidfile+'; exec '+shlex.join(server_command)]
        log=(base/'server.log').open('wb')
        server=subprocess.Popen(guest.argv(remote),stdout=log,stderr=log)
        forward=subprocess.Popen(['ssh',*guest.options(),'-N','-L',f'127.0.0.1:{port}:127.0.0.1:11045','kode-uat@127.0.0.1'],stdout=log,stderr=log)
        try:
            from playwright.sync_api import sync_playwright
            with sync_playwright() as playwright:
                chrome=playwright.chromium.launch(executable_path=args.chrome,headless=True,args=['--no-sandbox'])
                try:
                    page=chrome.new_page(viewport={'width':1100,'height':650})
                    page.on('dialog',lambda dialog:dialog.accept())
                    import urllib.request
                    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
                    def ready():
                        try:
                            with opener.open(f'http://127.0.0.1:{port}',timeout=.2) as response:return response.status==200
                        except OSError:return False
                    browser.until(ready,'strict Snap browser gateway',timeout=20)
                    page.goto(f'http://127.0.0.1:{port}')
                    # The preceding real terminal sessions persisted the open
                    # SQL document and active database tab. Open the real file.
                    browser.visible(page,'Ready');page.keyboard.press('Alt+1')
                    browser.click_text(page,'Note.txt');browser.visible(page,'Snap core saved')
                    browser.click_text(page,'Snap core saved');page.keyboard.press('Control+a');page.evaluate("term.paste('Snap browser saved\\n')");page.keyboard.press('Control+s')
                    browser.until(lambda:guest.run(['cat',note]).stdout=='Snap browser saved\n','Snap browser physical save')
                    page.close()
                except Exception as error:
                    screen=page.evaluate(browser.SCREEN) if not page.is_closed() else '<closed>'
                    raise AssertionError('Snap browser acceptance failed; terminal screen:\n'+screen+'\nGateway log:\n'+(base/'server.log').read_text(errors='replace')[-2500:]) from error
                finally:chrome.close()
            cases.append({'case':'strict-snap-packaged-browser-over-ssh','status':'passed','physicalSave':True})
        finally:
            guest.run(['sh','-c','test ! -e '+pidfile+' || kill -TERM "$(cat '+pidfile+')"'],check=False)
            forward.terminate();forward.wait(timeout=10)
            try:server.wait(timeout=10)
            except subprocess.TimeoutExpired:server.terminate();server.wait(timeout=10)
            log.close()
        # The direct and browser editor processes must both be gone in the VM.
        process_check="import pathlib\nfound=[]\nfor p in pathlib.Path('/proc').glob('[0-9]*/comm'):\n try:\n  if p.read_text().strip() in ('kode','ttyd'): found.append(str(p))\n except (FileNotFoundError,PermissionError,ProcessLookupError): pass\nassert not found,found"
        guest.run(['python3','-c',process_check])
        cases[-1]['childrenReaped']=True
    evidence={'status':'passed','artifact':artifact.name,'sha256':metadata['sha256'],'confinement':'strict','base':'core24','vmImage':IMAGE_URL,'vmImageSha256':IMAGE_SHA256,'vmVersions':versions,'hostSnapInstallationChanged':False,'javaRuntimeBundled':False,'cases':cases,'passed':len(cases)}
    (ROOT/'docs/parity/Z03-snap-vm.json').write_text(json.dumps(evidence,indent=2)+'\n')
    print(json.dumps(evidence,indent=2))


if __name__=='__main__':main()
