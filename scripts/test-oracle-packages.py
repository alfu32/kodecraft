#!/usr/bin/env python3
"""Explicit Oracle package qualification through the actual external JDBC helper.

Requires KODE_TEST_PACKAGE_ORACLE_URL, _USER and _PASSWORD for an owned test schema.
Creates a fresh named package and drops only that package afterward. Existing
objects are never replaced. This qualifies the helper, not the TUI or other vendors.
"""
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

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = 'KODE_RS_U12_界_%'
SPECIFICATION = f'''CREATE PACKAGE "{PACKAGE}" AS
  FUNCTION calc(n NUMBER) RETURN NUMBER;
  FUNCTION calc(n VARCHAR2) RETURN VARCHAR2;
  PROCEDURE increment_value(n IN OUT NUMBER);
END;
'''
BODY = f'''CREATE PACKAGE BODY "{PACKAGE}" AS
  FUNCTION calc(n NUMBER) RETURN NUMBER IS BEGIN RETURN n + 1; END;
  FUNCTION calc(n VARCHAR2) RETURN VARCHAR2 IS BEGIN RETURN 'native-source-界:' || n; END;
  PROCEDURE increment_value(n IN OUT NUMBER) IS BEGIN n := n + 1; END;
END;
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--helper', type=Path, default=ROOT / 'jdbc-bridge/target/kode-jdbc-bridge.jar')
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    url, user, password = [os.environ['KODE_TEST_PACKAGE_ORACLE_' + name] for name in ('URL', 'USER', 'PASSWORD')]
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix='kode-oracle-packages-') as directory:
        base = Path(directory)
        env = os.environ.copy()
        for name in ('JAVA_TOOL_OPTIONS', '_JAVA_OPTIONS', 'JDK_JAVA_OPTIONS'):
            env[name] = ''
        env.update(KODE_RS_JDBC_HOME=str(base / 'plugins'), KODE_LEGACY_JDBC_HOME='')
        child = subprocess.Popen(['java', '-jar', str(args.helper.resolve())], cwd=base, env=env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 start_new_session=True)
        tail = bytearray()

        def drain():
            while data := child.stderr.read(8192):
                tail.extend(data)
                if len(tail) > 65536:
                    del tail[:-65536]

        reader = threading.Thread(target=drain, daemon=True)
        reader.start()
        selector = selectors.DefaultSelector()
        selector.register(child.stdout, selectors.EVENT_READ)
        number = 0

        def exact(size, deadline):
            output = bytearray()
            while len(output) < size:
                if not selector.select(max(0, deadline - time.monotonic())):
                    raise TimeoutError('Oracle package qualification timed out')
                data = os.read(child.stdout.fileno(), size - len(output))
                if not data:
                    raise RuntimeError('JDBC helper closed its protocol')
                output.extend(data)
            return bytes(output)

        def call(method, params):
            nonlocal number
            number += 1
            identity = str(number)
            value = dict(jsonrpc='2.0', id=identity, method=method, params=params)
            body = json.dumps(value).encode()
            child.stdin.write(struct.pack('>I', len(body)) + body)
            child.stdin.flush()
            deadline = time.monotonic() + 180
            while True:
                size = struct.unpack('>I', exact(4, deadline))[0]
                assert 0 < size <= 8388608
                frame = json.loads(exact(size, deadline))
                if frame.get('id') == identity:
                    if 'error' in frame:
                        # Request bodies/credentials are never copied into diagnostics.
                        sql_error = frame['error'].get('data', {}).get('sqlError', {})
                        raise RuntimeError(f"{method}: RPC error {frame['error']['code']}: {frame['error'].get('message', '')}; SQLSTATE={sql_error.get('sqlState')}, vendor={sql_error.get('vendorCode')}, detail={sql_error.get('message', '')}")
                    return frame['result']
                assert frame.get('method') in ('driver.progress', 'query.event')

        connection = None
        created = False

        def execute(sql):
            result = call('query.execute', dict(connectionId=connection, executionId=f'oracle-package-{number}', sql=sql))
            if result['outcome'] != 'completed':
                problem = result.get('error') or {}
                raise RuntimeError(f"Owned fixture SQL failed: SQLSTATE={problem.get('sqlState')}, vendor={problem.get('vendorCode')}")

        def metadata(request):
            return call('metadata.list', dict(connectionId=connection, metadataJobId=str(uuid.uuid4()), request=request))

        try:
            hello = call('hello', dict(protocolVersion=1, clientVersion='oracle-packages-qualification'))
            assert 'metadata.source' in hello['capabilities']
            assert 'metadata.cancel' in hello['capabilities']
            driver = call('driver.resolve', dict(resolutionId='oracle-package-driver', driver=dict(
                id='oracle', providerId='oracle', driverClass='oracle.jdbc.OracleDriver', artifact=dict(
                    repositoryType='MAVEN_CENTRAL', groupId='com.oracle.database.jdbc', artifactId='ojdbc11', version='23.26.3.0.0'))))
            inspected = call('driver.inspect', dict(providerId='oracle', classpath=driver['classpath'], driverClass='oracle.jdbc.OracleDriver'))
            opened = call('connection.open', dict(dataSourceId='owned-oracle-package-schema', loaderId=inspected['loaderId'],
                driverClass='oracle.jdbc.OracleDriver', jdbcUrl=url, properties=dict(user=user, password=password)))
            connection = opened['connectionId']
            schema = opened['schema']
            listing = dict(kind='CATEGORY', schema=schema, category='PACKAGE')
            assert all(node['name'] != PACKAGE for node in metadata(listing)['objects']), 'Fixture package already exists; preserving it'
            execute(SPECIFICATION)
            created = True
            execute(BODY)
            packages = metadata(listing)
            assert packages['capabilities']['packages'] and packages['capabilities']['packageDdl']
            package = next(node for node in packages['objects'] if node['name'] == PACKAGE)
            assert package['schema'] == schema and package['hasChildren']
            request = dict(kind='OBJECT', schema=schema, objectName=PACKAGE, objectType='PACKAGE')
            members = metadata(request)['objects']
            functions = [node for node in members if node['objectType'] == 'FUNCTION']
            procedures = [node for node in members if node['objectType'] == 'PROCEDURE']
            assert len(functions) == 2 and {node['name'] for node in functions} == {'CALC'}
            assert len(procedures) == 1 and procedures[0]['name'] == 'INCREMENT_VALUE'
            assert len({node['id'] for node in members}) == 3
            assert all(node['schema'] == schema and node['attributes']['packageName'] == PACKAGE for node in members)
            definitions = call('metadata.source', dict(connectionId=connection, metadataJobId='oracle-package-source-1', request=request))
            documents = definitions['documents']
            assert len(documents) == 2 and all(doc['kind'] == 'DDL' for doc in documents)
            combined = '\n'.join(doc['text'] for doc in documents)
            assert 'native-source-界:' in combined and 'RETURN n + 1' in combined
            assert all('CREATE' in doc['text'].upper() and PACKAGE in doc['text'] for doc in documents)
            report = dict(status='passed', mode='native-oracle-helper', tuiQualified=False,
                product=opened['productName'], productVersion=opened['productVersion'],
                driverVersion=opened['driverVersion'], driverClasspathDigest=inspected['classpathDigest'],
                helperSha256=hashlib.sha256(args.helper.read_bytes()).hexdigest(), packageListing=True,
                exactUnicodeWildcardName=True, schemaOwnershipVerified=True, overloadedFunctions=2,
                procedures=1, distinctMemberIds=True, ddlDocuments=[dict(kind=doc['kind'],
                    utf8Bytes=len(doc['text'].encode()), sha256=hashlib.sha256(doc['text'].encode()).hexdigest()) for doc in documents],
                sourceContentsVerified=True, sourceContextRevision=definitions['contextRevision'])
            execute(f'DROP PACKAGE "{PACKAGE}"')
            created = False
            assert all(node['name'] != PACKAGE for node in metadata(listing)['objects'])
            report['ownedPackageRemoved'] = True
            closing, connection = connection, None
            call('connection.close', dict(connectionId=closing))
            call('shutdown', {})
            child.stdin.close()
            child.wait(timeout=5)
            assert child.returncode == 0
            report.update(helperReaped=True, durationSeconds=round(time.monotonic() - started, 2))
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + '\n')
            print('Passed actual Oracle package listing, overloads and complete DDL through external helper')
        finally:
            try:
                if created and child.poll() is None:
                    execute(f'DROP PACKAGE "{PACKAGE}"')
            finally:
                try:
                    if connection is not None and child.poll() is None:
                        call('connection.close', dict(connectionId=connection))
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=5)
                    selector.close()
                    reader.join(timeout=2)


if __name__ == '__main__':
    main()
