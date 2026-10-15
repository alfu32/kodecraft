#!/usr/bin/env python3
"""Explicit public-artifact smoke test. No database network or user config used.

Run after build-jdbc-helper.sh; ordinary deterministic tests do not require this
network check. H2 and Derby artifacts are downloaded into an isolated temp root.
"""
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
JAR = ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar"

with tempfile.TemporaryDirectory(prefix="kode-driver-network-") as directory:
    base = Path(directory)
    environment = os.environ.copy()
    environment.update(KODE_RS_JDBC_HOME=str(base / "plugins"), KODE_LEGACY_JDBC_HOME="",
                       JAVA_TOOL_OPTIONS="", _JAVA_OPTIONS="", JDK_JAVA_OPTIONS="")
    process = subprocess.Popen(["java", "-jar", str(JAR)], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               cwd=base, env=environment, start_new_session=True)
    tail = bytearray()
    def drain():
        while data := process.stderr.read(8192):
            tail.extend(data)
            if len(tail) > 65536:
                del tail[:-65536]
    stderr = threading.Thread(target=drain, daemon=True)
    stderr.start()
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    number = 0
    def exact(size, deadline):
        output = bytearray()
        while len(output) < size:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not selector.select(remaining):
                raise TimeoutError("JDBC artifact smoke test timed out")
            data = os.read(process.stdout.fileno(), size - len(output))
            if not data:
                raise RuntimeError("JDBC helper closed its protocol")
            output.extend(data)
        return bytes(output)
    def call(method, params):
        global number
        number += 1
        identity = str(number)
        body = json.dumps(dict(jsonrpc="2.0", id=identity, method=method, params=params)).encode()
        process.stdin.write(struct.pack(">I", len(body)) + body)
        process.stdin.flush()
        deadline = time.monotonic() + 180
        while True:
            size = struct.unpack(">I", exact(4, deadline))[0]
            assert 0 < size <= 8388608
            frame = json.loads(exact(size, deadline).decode())
            if frame.get("id") == identity:
                if "error" in frame:
                    raise RuntimeError(f"JDBC artifact operation failed with code {frame['error']['code']}")
                return frame["result"]
            assert frame.get("method") == "driver.progress"
    try:
        assert call("hello", dict(protocolVersion=1, clientVersion="network-fixture"))["protocolVersion"] == 1
        cases = []
        for provider, group, artifact, version, driver_class, minimum in [
                ("h2", "com.h2database", "h2", "2.2.224", "org.h2.Driver", 1),
                ("derby-embedded", "org.apache.derby", "derby", "10.17.1.0", "org.apache.derby.jdbc.EmbeddedDriver", 2)]:
            definition = dict(id=provider, providerId=provider, driverClass=driver_class,
                              artifact=dict(repositoryType="MAVEN_CENTRAL", groupId=group,
                                            artifactId=artifact, version=version))
            installed = call("driver.resolve", dict(resolutionId=f"network-{provider}", driver=definition))
            assert len(installed["classpath"]) >= minimum
            info = call("driver.inspect", dict(providerId=provider, classpath=installed["classpath"], driverClass=driver_class))
            assert info["loaderId"] and info["driverClasses"]
            assert info["classpathDigest"] == installed["classpathDigest"]
            reused = call("driver.resolve", dict(resolutionId=f"reuse-{provider}", driver=definition))
            assert reused["reused"] and reused["classpath"] == installed["classpath"]
            assert not list((base / "plugins").glob(".work-*"))
            cases.append(dict(provider=provider, selectedVersion=installed["resolvedVersion"],
                              jarCount=len(installed["classpath"]), selectedClass=info["selectedDriverClass"],
                              reused=True, status="passed"))
        assert call("shutdown", {})["closing"]
        process.stdin.close()
        assert process.wait(timeout=5) == 0
        stderr.join(timeout=2)
        (ROOT / "docs/parity/D03-network.json").write_text(json.dumps(dict(cases=cases, passed=len(cases)), indent=2) + "\n")
        print(f"Passed {len(cases)} packaged-helper public-artifact resolution/inspection/reuse cases")
    finally:
        selector.close()
        if process.poll() is None:
            import signal
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
        stderr.join(timeout=2)
