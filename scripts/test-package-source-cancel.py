#!/usr/bin/env python3
"""Exercise active package-source cancellation against an owned live vendor schema."""
import argparse
from datetime import date
import hashlib
import json
import os
from pathlib import Path
import queue
import struct
import subprocess
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]


class Helper:
    def __init__(self, jar, cwd, env):
        self.child = subprocess.Popen(['java', '-jar', str(jar.resolve())], cwd=cwd, env=env,
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, start_new_session=True)
        self.write_lock = threading.Lock()
        self.pending_lock = threading.Lock()
        self.pending = {}
        self.sequence = 0
        self.stderr = bytearray()
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._drain_stderr, daemon=True).start()

    @staticmethod
    def _exact(stream, size):
        result = bytearray()
        while len(result) < size:
            chunk = stream.read(size - len(result))
            if not chunk:
                raise RuntimeError('JDBC helper closed its protocol')
            result.extend(chunk)
        return bytes(result)

    def _read(self):
        try:
            while True:
                size = struct.unpack('>I', self._exact(self.child.stdout, 4))[0]
                if not 0 < size <= 8 * 1024 * 1024:
                    raise RuntimeError('Invalid JDBC helper frame size')
                frame = json.loads(self._exact(self.child.stdout, size))
                identity = frame.get('id')
                if identity is not None:
                    with self.pending_lock:
                        waiter = self.pending.get(identity)
                    if waiter is not None:
                        waiter.put(frame)
        except BaseException as failure:
            with self.pending_lock:
                pending = list(self.pending.values())
            for waiter in pending:
                waiter.put(failure)

    def _drain_stderr(self):
        while chunk := self.child.stderr.read(8192):
            self.stderr.extend(chunk)
            if len(self.stderr) > 65536:
                del self.stderr[:-65536]

    def send(self, method, params):
        with self.pending_lock:
            self.sequence += 1
            identity = str(self.sequence)
            waiter = queue.Queue(maxsize=1)
            self.pending[identity] = waiter
        body = json.dumps(dict(jsonrpc='2.0', id=identity, method=method, params=params)).encode()
        with self.write_lock:
            self.child.stdin.write(struct.pack('>I', len(body)) + body)
            self.child.stdin.flush()
        return identity, waiter

    def receive(self, identity, waiter, timeout=180):
        try:
            frame = waiter.get(timeout=timeout)
        except queue.Empty as failure:
            raise TimeoutError('JDBC helper request timed out') from failure
        finally:
            with self.pending_lock:
                self.pending.pop(identity, None)
        if isinstance(frame, BaseException):
            raise frame
        if 'error' in frame:
            error = frame['error']
            sql = error.get('data', {}).get('sqlError', {})
            raise RuntimeError(f"RPC {error.get('code')}: {error.get('message', '')}; SQLSTATE={sql.get('sqlState')}")
        return frame['result']

    def call(self, method, params):
        identity, waiter = self.send(method, params)
        return self.receive(identity, waiter)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--vendor', choices=('oracle', 'mariadb'), required=True)
    parser.add_argument('--helper', type=Path, default=ROOT / 'jdbc-bridge/target/kode-jdbc-bridge.jar')
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--attempts', type=int, default=8)
    parser.add_argument('--cancel-after-ms', type=int, default=5)
    args = parser.parse_args()
    prefix = 'KODE_TEST_PACKAGE_' + args.vendor.upper() + '_'
    url, user, password = [os.environ[prefix + name] for name in ('URL', 'USER', 'PASSWORD')]
    if args.vendor == 'oracle':
        provider, driver, coordinates = 'oracle', 'oracle.jdbc.OracleDriver', 'com.oracle.database.jdbc:ojdbc11:23.26.3.0.0'
    else:
        provider, driver, coordinates = 'mariadb', 'org.mariadb.jdbc.Driver', 'org.mariadb.jdbc:mariadb-java-client:3.5.10'
    job_id = 'u12-package-cancel-' + str(uuid.uuid4())
    package = 'KODE_RS_CANCEL_' + uuid.uuid4().hex[:16].upper()
    large_comment = '\n'.join('/* ' + ('x' * 1020) + ' */' for _ in range(1400))
    if args.vendor == 'oracle':
        specification = f'CREATE PACKAGE "{package}" AS FUNCTION f RETURN NUMBER; END;'
        body = f'CREATE PACKAGE BODY "{package}" AS\n{large_comment}\nFUNCTION f RETURN NUMBER IS BEGIN RETURN 42; END;\nEND;'
    else:
        specification = f'CREATE PACKAGE "{package}" AS FUNCTION f RETURN INT; END;'
        body = f'CREATE PACKAGE BODY "{package}" AS\n{large_comment}\nFUNCTION f RETURN INT AS BEGIN RETURN 42; END;\nEND;'
    started = time.monotonic()
    with __import__('tempfile').TemporaryDirectory(prefix='kode-package-cancel-') as directory:
        env = os.environ.copy()
        for name in ('JAVA_TOOL_OPTIONS', '_JAVA_OPTIONS', 'JDK_JAVA_OPTIONS'):
            env[name] = ''
        env.update(KODE_RS_JDBC_HOME=str(Path(directory) / 'plugins'), KODE_LEGACY_JDBC_HOME='')
        child = Helper(args.helper, Path(directory), env)
        connection = None
        created = False
        try:
            hello = child.call('hello', dict(protocolVersion=1, clientVersion='u12-package-cancel'))
            assert 'metadata.cancel' in hello['capabilities']
            resolved = child.call('driver.resolve', dict(resolutionId='u12-cancel-driver', driver=dict(
                id=provider, providerId=provider, driverClass=driver,
                artifact=dict(repositoryType='MAVEN_CENTRAL', groupId=coordinates.split(':')[0],
                              artifactId=coordinates.split(':')[1], version=coordinates.split(':')[2]))))
            inspected = child.call('driver.inspect', dict(providerId=provider, classpath=resolved['classpath'], driverClass=driver))
            opened = child.call('connection.open', dict(dataSourceId='u12-cancel-owned-schema', loaderId=inspected['loaderId'],
                driverClass=driver, jdbcUrl=url, properties=dict(user=user, password=password)))
            connection = opened['connectionId']
            if args.vendor == 'mariadb':
                execute = child.call('query.execute', dict(connectionId=connection, executionId='u12-cancel-sql-mode', sql="SET SESSION sql_mode='ORACLE'"))
                assert execute['outcome'] == 'completed'
            listing = dict(kind='CATEGORY', schema=opened.get('schema') or opened.get('catalog'), category='PACKAGE')
            assert all(item['name'] != package for item in child.call('metadata.list', dict(
                connectionId=connection, metadataJobId=str(uuid.uuid4()), request=listing))['objects'])
            for phase, sql in (('specification', specification), ('body', body)):
                execute = child.call('query.execute', dict(connectionId=connection,
                    executionId='u12-cancel-create-' + str(uuid.uuid4()), sql=sql))
                if execute['outcome'] != 'completed':
                    failure = execute.get('error') or {}
                    raise RuntimeError(f"Could not create the owned large package {phase}; SQLSTATE={failure.get('sqlState')}, vendor={failure.get('vendorCode')}")
                created = True
            request = dict(kind='OBJECT', schema=listing['schema'], objectName=package, objectType='PACKAGE')
            active_cancel = None
            source_error = None
            for attempt in range(1, args.attempts + 1):
                identity, source_waiter = child.send('metadata.source', dict(
                    connectionId=connection, metadataJobId=job_id + '-' + str(attempt), request=request))
                time.sleep(max(0, args.cancel_after_ms) / 1000)
                cancel = child.call('metadata.cancel', dict(metadataJobId=job_id + '-' + str(attempt)))
                source_frame = None
                try:
                    source_frame = child.receive(identity, source_waiter, timeout=20)
                except RuntimeError as failure:
                    source_error = failure
                if cancel.get('statementAttached') and cancel.get('status') == 'requested':
                    active_cancel = dict(attempt=attempt, statementAttached=True, status=cancel['status'])
                    if source_error is None and source_frame is not None:
                        raise RuntimeError('Source request completed despite cancellation of its attached statement')
                    break
            if active_cancel is None:
                raise RuntimeError('Could not observe cancellation while a JDBC statement was attached')
            if source_error is None or 'SQLSTATE=57014' not in str(source_error):
                raise RuntimeError(f'Active package-source cancellation returned unexpected result: {source_error}')
            child.call('query.execute', dict(connectionId=connection, executionId='u12-cancel-drop', sql=f'DROP PACKAGE "{package}"'))
            created = False
            remaining = child.call('metadata.list', dict(connectionId=connection, metadataJobId=str(uuid.uuid4()), request=listing))['objects']
            if any(item['name'] == package for item in remaining):
                raise RuntimeError('Owned package fixture remained after cleanup')
            child.call('connection.close', dict(connectionId=connection))
            connection = None
            child.call('shutdown', {})
            child.child.stdin.close()
            child.child.wait(timeout=10)
            if child.child.returncode != 0:
                raise RuntimeError('JDBC helper did not shut down cleanly')
            report = dict(status='passed', vendor=args.vendor, mode='native-active-package-source-cancel',
                product=opened['productName'], productVersion=opened['productVersion'],
                helperSha256=hashlib.sha256(args.helper.read_bytes()).hexdigest(),
                packageBytes=len(body.encode()), activeStatementCancellation=active_cancel,
                resultSqlState='57014', ownedPackageRemoved=True, helperReaped=True,
                durationSeconds=round(time.monotonic() - started, 2), verifiedOn=date.today().isoformat())
        except BaseException:
            if created and connection is not None:
                try:
                    child.call('query.execute', dict(connectionId=connection, executionId='u12-cancel-cleanup', sql=f'DROP PACKAGE "{package}"'))
                except BaseException:
                    pass
            if connection is not None:
                try:
                    child.call('connection.close', dict(connectionId=connection))
                except BaseException:
                    pass
            if child.child.poll() is None:
                try:
                    child.call('shutdown', {})
                except BaseException:
                    pass
                child.child.kill()
                child.child.wait(timeout=5)
            raise
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{args.vendor} package-source cancellation: {report['status']}; fixture removed; helper reaped")


if __name__ == '__main__':
    main()
