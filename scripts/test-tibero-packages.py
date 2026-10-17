#!/usr/bin/env python3
"""Read-only Tibero package qualification through the external JDBC helper.

Requires an owned endpoint, a local Tibero JDBC JAR and an existing package with
both specification and body. No database object is created, changed or dropped.
Set KODE_TEST_PACKAGE_TIBERO_OWNED=1 and the same prefix's URL, USER, PASSWORD,
JAR, SCHEMA, PACKAGE, MEMBER, MEMBER_TYPE and SOURCE_MARKER variables. MEMBER_TYPE
is FUNCTION or PROCEDURE. A passing report qualifies the helper only, not the TUI.
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
PREFIX = "KODE_TEST_PACKAGE_TIBERO_"
REQUIRED = ("URL", "USER", "PASSWORD", "JAR", "SCHEMA", "PACKAGE",
            "MEMBER", "MEMBER_TYPE", "SOURCE_MARKER")


def configuration():
    absent = [name for name in REQUIRED if PREFIX + name not in os.environ]
    if absent:
        raise SystemExit("Missing Tibero test configuration: " + ", ".join(absent))
    if os.environ.get(PREFIX + "OWNED") != "1":
        raise SystemExit("Set KODE_TEST_PACKAGE_TIBERO_OWNED=1 for an owned test endpoint")
    values = {name: os.environ[PREFIX + name] for name in REQUIRED}
    if any(not value for value in values.values()):
        raise SystemExit("Tibero test configuration has an empty required value")
    if values["MEMBER_TYPE"] not in ("FUNCTION", "PROCEDURE"):
        raise SystemExit("Tibero MEMBER_TYPE must be FUNCTION or PROCEDURE")
    if not Path(values["JAR"]).is_file():
        raise SystemExit("Tibero JAR must name an existing local file")
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path,
                        default=ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar")
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    cfg = configuration()
    if not args.helper.is_file():
        raise SystemExit("Build the JDBC helper before Tibero qualification")
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="kode-tibero-packages-") as directory:
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
                    raise TimeoutError("Tibero helper request timed out")
                data = os.read(child.stdout.fileno(), size - len(output))
                if not data:
                    raise RuntimeError("Tibero helper closed its protocol")
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
                    raise RuntimeError("Tibero helper sent an invalid frame size")
                frame = json.loads(exact(size, deadline))
                if frame.get("id") == identity:
                    if "error" in frame:
                        # JDBC error details may contain connection secrets.
                        raise RuntimeError(f"Tibero helper {method} failed with RPC code "
                                           f"{frame['error'].get('code')}")
                    return frame["result"]
                if frame.get("method") not in ("driver.progress", "query.event"):
                    raise RuntimeError("Tibero helper sent an unexpected notification")

        connection = None
        try:
            hello = call("hello", dict(protocolVersion=1,
                                       clientVersion="tibero-packages-qualification"))
            if not {"metadata.source", "metadata.cancel"}.issubset(hello["capabilities"]):
                raise AssertionError("Package source/cancellation capability missing")
            driver = call("driver.resolve", dict(resolutionId="tibero-package-driver",
                driver=dict(id="tibero", providerId="tibero",
                    driverClass="com.tmax.tibero.jdbc.TbDriver", jarPath=cfg["JAR"],
                    artifact=dict(repositoryType="LOCAL_FILE"))))
            inspected = call("driver.inspect", dict(providerId="tibero",
                classpath=driver["classpath"],
                driverClass="com.tmax.tibero.jdbc.TbDriver"))
            opened = call("connection.open", dict(dataSourceId="owned-tibero-package-schema",
                loaderId=inspected["loaderId"],
                driverClass="com.tmax.tibero.jdbc.TbDriver", jdbcUrl=cfg["URL"],
                properties=dict(user=cfg["USER"], password=cfg["PASSWORD"])))
            connection = opened["connectionId"]
            if "TIBERO" not in opened["productName"].upper():
                raise AssertionError("Endpoint did not report Tibero")
            request = dict(kind="OBJECT", schema=cfg["SCHEMA"],
                           objectName=cfg["PACKAGE"], objectType="PACKAGE")
            listing = call("metadata.list", dict(connectionId=connection,
                metadataJobId="tibero-package-list", request=dict(kind="CATEGORY",
                    schema=cfg["SCHEMA"], category="PACKAGE")))
            capabilities = listing["capabilities"]
            if not all(capabilities[name] for name in
                       ("packages", "packageMembers", "packageSource", "packageDdl")):
                raise AssertionError("Tibero package capability flags differ")
            candidates = [node for node in listing["objects"]
                          if node["name"] == cfg["PACKAGE"] and
                          node["schema"] == cfg["SCHEMA"]]
            if len(candidates) != 1:
                raise AssertionError("Expected exactly one matching Tibero package")
            package = candidates[0]
            if json.loads(package["id"]) != ["tibero-package", cfg["SCHEMA"],
                                              cfg["PACKAGE"]]:
                raise AssertionError("Listed Tibero package identity is inconsistent")
            if not package["hasChildren"] or any(
                package["attributes"].get(name) != expected
                for name, expected in (("sourceAvailable", "true"),
                                       ("ddlAvailable", "true"),
                                       ("packageProvider", "tibero"))
            ):
                raise AssertionError("Tibero package actions were not advertised")
            members = call("metadata.list", dict(connectionId=connection,
                metadataJobId="tibero-package-members", request=request))["objects"]
            if not members or len({node["id"] for node in members}) != len(members):
                raise AssertionError("Tibero package members are absent or not distinct")
            if not any(node["name"] == cfg["MEMBER"] and
                       node["objectType"] == cfg["MEMBER_TYPE"] for node in members):
                raise AssertionError("Expected Tibero package member was absent")
            if any(node["schema"] != cfg["SCHEMA"] or
                   node["attributes"].get("packageName") != cfg["PACKAGE"] or
                   node["attributes"].get("packageProvider") != "tibero"
                   for node in members):
                raise AssertionError("Tibero member ownership differs")
            source = call("metadata.source", dict(connectionId=connection,
                metadataJobId="tibero-package-source", request=request))
            documents = source["documents"]
            if len(documents) != 2 or any(doc["kind"] != "DDL" for doc in documents):
                raise AssertionError("Tibero specification and body DDL are both required")
            titles = {doc["title"].rsplit(" — ", 1)[-1] for doc in documents}
            if titles != {"PACKAGE", "PACKAGE BODY"}:
                raise AssertionError("Tibero source document titles differ")
            if not any(cfg["SOURCE_MARKER"] in doc["text"] for doc in documents):
                raise AssertionError("Expected existing Tibero source marker was absent")
            if any(cfg["PACKAGE"] not in doc["text"] for doc in documents):
                raise AssertionError("Tibero source does not name the selected package")
            closing, connection = connection, None
            call("connection.close", dict(connectionId=closing))
            call("shutdown", {})
            child.stdin.close()
            child.wait(timeout=5)
            if child.returncode != 0:
                raise RuntimeError("Tibero helper did not shut down cleanly")
            report = dict(status="passed", mode="native-tibero-helper",
                nativeQualified=True, tuiQualified=False,
                product=opened["productName"], productVersion=opened["productVersion"],
                driverVersion=opened["driverVersion"],
                driverClasspathDigest=inspected["classpathDigest"],
                helperSha256=hashlib.sha256(args.helper.read_bytes()).hexdigest(),
                packageSchema=cfg["SCHEMA"], packageName=cfg["PACKAGE"],
                memberName=cfg["MEMBER"], memberType=cfg["MEMBER_TYPE"],
                sourceContextRevision=source["contextRevision"],
                ddlDocuments=[dict(title=doc["title"],
                    utf8Bytes=len(doc["text"].encode()),
                    sha256=hashlib.sha256(doc["text"].encode()).hexdigest())
                    for doc in documents], sourceMarkerVerified=True,
                packageMembersVerified=True, helperReaped=True,
                durationSeconds=round(time.monotonic() - started, 2))
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
            print("Passed native Tibero package members and stored DDL through external helper")
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
