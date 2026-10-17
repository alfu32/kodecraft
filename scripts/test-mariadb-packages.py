#!/usr/bin/env python3
"""Qualify MariaDB package metadata through an owned external JDBC helper/schema."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import struct
import subprocess
import tempfile
import threading
import time
import uuid

ROOT=Path(__file__).resolve().parents[1]
PACKAGE=f'KODE_RS_U12_界_{os.getpid()}_%'
SPEC=f'''CREATE PACKAGE "{PACKAGE}" AS
  FUNCTION calc_int(n INT) RETURN INT;
  FUNCTION calc_text(n VARCHAR(32)) RETURN VARCHAR(32);
  PROCEDURE increment_value(n INOUT INT);
END;
'''
BODY=f'''CREATE PACKAGE BODY "{PACKAGE}" AS
  FUNCTION calc_int(n INT) RETURN INT AS BEGIN RETURN n + 1; END;
  FUNCTION calc_text(n VARCHAR(32)) RETURN VARCHAR(32) AS BEGIN RETURN 'native-source-界:' || n; END;
  PROCEDURE increment_value(n INOUT INT) AS BEGIN SET n = n + 1; END;
END;
'''

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--helper',type=Path,default=ROOT/'jdbc-bridge/target/kode-jdbc-bridge.jar')
    parser.add_argument('--report',type=Path,required=True)
    args=parser.parse_args()
    url,user,password=[os.environ['KODE_TEST_PACKAGE_MARIADB_'+name] for name in ('URL','USER','PASSWORD')]
    start=time.monotonic()
    with tempfile.TemporaryDirectory(prefix='kode-mariadb-packages-') as temp:
        base=Path(temp);env=os.environ.copy()
        for name in ('JAVA_TOOL_OPTIONS','_JAVA_OPTIONS','JDK_JAVA_OPTIONS'):env[name]=''
        env.update(KODE_RS_JDBC_HOME=str(base/'plugins'),KODE_LEGACY_JDBC_HOME='')
        child=subprocess.Popen(['java','-jar',str(args.helper.resolve())],cwd=base,env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
        stderr=bytearray()
        def drain():
            while data:=child.stderr.read(8192):
                stderr.extend(data)
                if len(stderr)>65536:del stderr[:-65536]
        threading.Thread(target=drain,daemon=True).start()
        selector=selectors.DefaultSelector();selector.register(child.stdout,selectors.EVENT_READ);sequence=0
        def exact(size,deadline):
            output=bytearray()
            while len(output)<size:
                if not selector.select(max(0,deadline-time.monotonic())):raise TimeoutError('MariaDB package qualification timed out')
                data=os.read(child.stdout.fileno(),size-len(output))
                if not data:raise RuntimeError('JDBC helper closed its protocol')
                output.extend(data)
            return bytes(output)
        def call(method,params):
            nonlocal sequence
            sequence+=1;identity=str(sequence);body=json.dumps(dict(jsonrpc='2.0',id=identity,method=method,params=params)).encode()
            child.stdin.write(struct.pack('>I',len(body))+body);child.stdin.flush();deadline=time.monotonic()+180
            while True:
                size=struct.unpack('>I',exact(4,deadline))[0]
                if not 0<size<=8388608:raise RuntimeError('Invalid helper frame size')
                frame=json.loads(exact(size,deadline))
                if frame.get('id')!=identity:continue
                if 'error' in frame:
                    sql=frame['error'].get('data',{}).get('sqlError',{})
                    raise RuntimeError(f"{method}: RPC {frame['error']['code']} {frame['error'].get('message','')}; SQLSTATE={sql.get('sqlState')} vendor={sql.get('vendorCode')}")
                return frame['result']
        connection=None;created=False
        def execute(sql):
            reply=call('query.execute',dict(connectionId=connection,executionId=f'mariadb-package-{sequence}',sql=sql))
            if reply['outcome']!='completed':
                error=reply.get('error') or {}
                raise RuntimeError(f"Owned fixture SQL failed: {error.get('message','')} SQLSTATE={error.get('sqlState')} vendor={error.get('vendorCode')}")
        def metadata(request):return call('metadata.list',dict(connectionId=connection,metadataJobId=str(uuid.uuid4()),request=request))
        try:
            hello=call('hello',dict(protocolVersion=1,clientVersion='mariadb-packages-qualification'))
            assert {'metadata.source','metadata.cancel'}.issubset(hello['capabilities'])
            driver=call('driver.resolve',dict(resolutionId='mariadb-package-driver',driver=dict(id='mariadb',providerId='mariadb',driverClass='org.mariadb.jdbc.Driver',artifact=dict(repositoryType='MAVEN_CENTRAL',groupId='org.mariadb.jdbc',artifactId='mariadb-java-client',version='3.5.10'))))
            inspected=call('driver.inspect',dict(providerId='mariadb',classpath=driver['classpath'],driverClass='org.mariadb.jdbc.Driver'))
            opened=call('connection.open',dict(dataSourceId='owned-mariadb-package-schema',loaderId=inspected['loaderId'],driverClass='org.mariadb.jdbc.Driver',jdbcUrl=url,properties=dict(user=user,password=password)))
            connection=opened['connectionId'];database=opened.get('schema') or opened.get('catalog')
            assert database
            execute("SET SESSION sql_mode='ORACLE'")
            listing=dict(kind='CATEGORY',schema=database,category='PACKAGE')
            assert all(node['name']!=PACKAGE for node in metadata(listing)['objects']),'Fixture package already exists; preserving it'
            execute(SPEC);created=True;execute(BODY)
            packages=metadata(listing)
            assert packages['capabilities']['packages'] and packages['capabilities']['packageDdl']
            pkg=next(node for node in packages['objects'] if node['name']==PACKAGE)
            assert pkg['schema']==database and pkg['hasChildren']
            request=dict(kind='OBJECT',schema=database,objectName=PACKAGE,objectType='PACKAGE')
            members=metadata(request)['objects'];functions=[x for x in members if x['objectType']=='FUNCTION'];procedures=[x for x in members if x['objectType']=='PROCEDURE']
            assert len(functions)==2 and {x['name'].lower() for x in functions}=={'calc_int','calc_text'}
            assert len(procedures)==1 and procedures[0]['name'].lower()=='increment_value'
            assert len({x['id'] for x in members})==3
            job='mariadb-package-source-1'
            source_result=call('metadata.source',dict(connectionId=connection,metadataJobId=job,request=request));assert source_result['contextRevision']>=0;documents=source_result['documents']
            assert len(documents)==2 and all(x['kind']=='DDL' for x in documents)
            combined='\n'.join(x['text'] for x in documents)
            assert 'native-source-界:' in combined and 'RETURN n + 1' in combined
            assert all(PACKAGE in x['text'] for x in documents)
            execute(f'DROP PACKAGE "{PACKAGE}"');created=False
            assert all(node['name']!=PACKAGE for node in metadata(listing)['objects'])
            call('connection.close',dict(connectionId=connection));connection=None
            report=dict(status='passed',mode='native-mariadb-helper',tuiQualified=False,product=opened['productName'],productVersion=opened['productVersion'],driverVersion=opened['driverVersion'],driverClasspathDigest=inspected['classpathDigest'],helperSha256=hashlib.sha256(args.helper.read_bytes()).hexdigest(),packageListing=True,exactUnicodeWildcardName=True,schemaOwnershipVerified=True,functions=2,procedures=1,distinctMemberIds=True,sourceContextRevision=source_result['contextRevision'],ddlDocuments=[dict(kind=x['kind'],utf8Bytes=len(x['text'].encode()),sha256=hashlib.sha256(x['text'].encode()).hexdigest()) for x in documents],sourceContentsVerified=True,ownedPackageRemoved=True,helperReaped=True,durationSeconds=round(time.monotonic()-start,2))
        except BaseException:
            if created and connection is not None:
                try:execute(f'DROP PACKAGE "{PACKAGE}"')
                except BaseException:pass
            if connection is not None:
                try:call('connection.close',dict(connectionId=connection))
                except BaseException:pass
            raise
        finally:
            try:call('shutdown',{})
            except BaseException:pass
            try:child.stdin.close()
            except OSError:pass
            try:child.wait(timeout=10)
            except subprocess.TimeoutExpired:child.kill();child.wait()
            selector.close()
        report['helperReaped']=child.poll() is not None
        if child.returncode!=0:raise RuntimeError('JDBC helper did not shut down cleanly')
        args.report.parent.mkdir(parents=True,exist_ok=True);args.report.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
        print(f"MariaDB package helper qualification: {report['status']}; fixture removed; helper reaped")

if __name__=='__main__':main()
