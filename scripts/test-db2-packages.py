#!/usr/bin/env python3
"""Qualify Db2 LUW package source through the external JDBC helper.

This read-only check requires an owned Db2 test endpoint and an existing package.
Set KODE_TEST_PACKAGE_DB2_OWNED=1 and the URL, USER, PASSWORD, SCHEMA, PACKAGE,
VERSION, SOURCE_MARKER and JCC_VERSION variables with the same prefix. VERSION
may be the empty string for an unversioned package. Set UNIQUE_ID as well when
the package name and version have more than one bound identity.
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


ROOT = Path(__file__).resolve().parents[1]
PREFIX = "KODE_TEST_PACKAGE_DB2_"


def configuration():
    required = ("URL", "USER", "PASSWORD", "SCHEMA", "PACKAGE", "VERSION",
                "SOURCE_MARKER", "JCC_VERSION")
    absent = [name for name in required if PREFIX + name not in os.environ]
    if absent:
        raise SystemExit("Missing Db2 test configuration: " + ", ".join(absent))
    if os.environ.get(PREFIX + "OWNED") != "1":
        raise SystemExit("Set KODE_TEST_PACKAGE_DB2_OWNED=1 for an owned test endpoint")
    values = {name: os.environ[PREFIX + name] for name in required}
    if any(not values[name] for name in required if name != "VERSION"):
        raise SystemExit("Db2 test configuration has an empty required value")
    values["UNIQUE_ID"] = os.environ.get(PREFIX + "UNIQUE_ID", "").upper()
    if values["UNIQUE_ID"] and (len(values["UNIQUE_ID"]) != 16 or
                                any(c not in "0123456789ABCDEF" for c in values["UNIQUE_ID"])):
        raise SystemExit("Db2 UNIQUE_ID must be 16 hexadecimal characters")
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path,
                        default=ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar")
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    cfg = configuration()
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="kode-db2-packages-") as directory:
        base = Path(directory)
        env = {name: value for name, value in os.environ.items()
               if not name.startswith(PREFIX)}
        for name in ("JAVA_TOOL_OPTIONS", "_JAVA_OPTIONS", "JDK_JAVA_OPTIONS"):
            env[name] = ""
        env.update(KODE_RS_JDBC_HOME=str(base / "plugins"), KODE_LEGACY_JDBC_HOME="")
        child = subprocess.Popen(["java", "-jar", str(args.helper.resolve())],
                                 cwd=base, env=env, stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE,
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
                    raise TimeoutError("Db2 helper request timed out")
                data = os.read(child.stdout.fileno(), size - len(output))
                if not data:
                    raise RuntimeError("Db2 helper closed its protocol")
                output.extend(data)
            return bytes(output)

        def call(method, params):
            nonlocal number
            number += 1
            identity = str(number)
            body = json.dumps(dict(jsonrpc="2.0", id=identity, method=method,
                                   params=params)).encode()
            child.stdin.write(struct.pack(">I", len(body)) + body)
            child.stdin.flush()
            deadline = time.monotonic() + 180
            while True:
                size = struct.unpack(">I", exact(4, deadline))[0]
                if not 0 < size <= 8388608:
                    raise RuntimeError("Db2 helper sent an invalid frame size")
                frame = json.loads(exact(size, deadline))
                if frame.get("id") == identity:
                    if "error" in frame:
                        # JDBC error details may contain connection secrets.
                        raise RuntimeError(f"Db2 helper {method} failed with RPC code "
                                           f"{frame['error'].get('code')}")
                    return frame["result"]
                if frame.get("method") not in ("driver.progress", "query.event"):
                    raise RuntimeError("Db2 helper sent an unexpected notification")

        connection = None
        try:
            hello = call("hello", dict(protocolVersion=1,
                                       clientVersion="db2-packages-qualification"))
            if "metadata.source" not in hello["capabilities"]:
                raise AssertionError("Package source capability missing")
            driver = call("driver.resolve", dict(resolutionId="db2-package-driver",
                driver=dict(id="db2", providerId="db2",
                    driverClass="com.ibm.db2.jcc.DB2Driver",
                    artifact=dict(repositoryType="MAVEN_CENTRAL", groupId="com.ibm.db2",
                                  artifactId="jcc", version=cfg["JCC_VERSION"]))))
            inspected = call("driver.inspect", dict(providerId="db2",
                classpath=driver["classpath"], driverClass="com.ibm.db2.jcc.DB2Driver"))
            opened = call("connection.open", dict(dataSourceId="owned-db2-package-schema",
                loaderId=inspected["loaderId"], driverClass="com.ibm.db2.jcc.DB2Driver",
                jdbcUrl=cfg["URL"], properties=dict(user=cfg["USER"],
                                                    password=cfg["PASSWORD"])))
            connection = opened["connectionId"]
            if "DB2" not in opened["productName"].upper():
                raise AssertionError("Endpoint did not report Db2")
            listing = call("metadata.list", dict(connectionId=connection,
                metadataJobId="db2-package-list", request=dict(kind="CATEGORY",
                    schema=cfg["SCHEMA"], category="PACKAGE")))
            capabilities = listing["capabilities"]
            if not (capabilities["packages"] and capabilities["packageSource"] and
                    not capabilities["packageDdl"] and not capabilities["packageMembers"]):
                raise AssertionError("Db2 package capability flags differ")
            candidates = [node for node in listing["objects"]
                          if node["name"] == cfg["PACKAGE"] and
                          node["schema"] == cfg["SCHEMA"] and
                          node["attributes"]["packageVersion"] == cfg["VERSION"] and
                          (not cfg["UNIQUE_ID"] or
                           node["attributes"]["packageUniqueId"].upper() == cfg["UNIQUE_ID"])]
            if len(candidates) != 1:
                raise AssertionError("Expected exactly one matching owned Db2 package")
            package = candidates[0]
            unique_id = package["attributes"]["packageUniqueId"].upper()
            opaque_id = json.loads(package["id"])
            if opaque_id != ["db2-package", cfg["SCHEMA"], cfg["PACKAGE"],
                             cfg["VERSION"], unique_id]:
                raise AssertionError("Listed Db2 package identity is inconsistent")
            if package["hasChildren"] or package["attributes"]["ddlAvailable"] != "false":
                raise AssertionError("Db2 package incorrectly advertises routine DDL")
            source = call("metadata.source", dict(connectionId=connection,
                metadataJobId="db2-package-source", request=dict(kind="OBJECT",
                    schema=cfg["SCHEMA"], objectName=cfg["PACKAGE"],
                    objectType="PACKAGE", objectId=package["id"])))
            documents = source["documents"]
            if not documents or any(doc["kind"] != "SQL_SOURCE" for doc in documents):
                raise AssertionError("Db2 did not return SQL statement source")
            if not any(cfg["SOURCE_MARKER"] in doc["text"] for doc in documents):
                raise AssertionError("Expected owned statement marker was absent")
            if cfg["VERSION"] == "" and any("(no version)" not in doc["title"]
                                             for doc in documents):
                raise AssertionError("Unversioned Db2 source title is incorrect")
            closing, connection = connection, None
            call("connection.close", dict(connectionId=closing))
            call("shutdown", {})
            child.stdin.close()
            child.wait(timeout=5)
            if child.returncode != 0:
                raise RuntimeError("Db2 helper did not shut down cleanly")
            report = dict(status="passed", mode="native-db2-helper",
                nativeQualified=True, tuiQualified=False,
                product=opened["productName"], productVersion=opened["productVersion"],
                driverVersion=opened["driverVersion"],
                driverClasspathDigest=inspected["classpathDigest"],
                helperSha256=hashlib.sha256(args.helper.read_bytes()).hexdigest(),
                packageSchema=cfg["SCHEMA"], packageName=cfg["PACKAGE"],
                packageVersion=cfg["VERSION"], packageUniqueId=unique_id,
                sourceContextRevision=source["contextRevision"],
                sqlSourceDocuments=[dict(utf8Bytes=len(doc["text"].encode()),
                    sha256=hashlib.sha256(doc["text"].encode()).hexdigest())
                    for doc in documents], sourceMarkerVerified=True,
                packageDdlAdvertised=False, packageMembersAdvertised=False,
                helperReaped=True, durationSeconds=round(time.monotonic() - started, 2))
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
            print("Passed native Db2 package identity and SQL source through external helper")
        finally:
            try:
                if connection is not None and child.poll() is None:
                    call("connection.close", dict(connectionId=connection))
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)
                selector.close()
                reader.join(timeout=2)


if __name__ == "__main__":
    main()
