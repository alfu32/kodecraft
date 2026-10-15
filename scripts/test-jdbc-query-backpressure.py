#!/usr/bin/env python3
"""Actual shaded helper: fill its OS output pipe, cancel, drain and reuse H2.

No network/database/user configuration is used. Large row bodies and raw stderr
are never printed or retained as evidence. Linux pipe capacity is measured.
"""
import array
import fcntl
import json
import os
from pathlib import Path
import select
import signal
import struct
import subprocess
import tempfile
import termios
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
JAR = Path(os.environ.get("KODE_TEST_JDBC_HELPER", ROOT / "jdbc-bridge/target/kode-jdbc-bridge.jar")).resolve()
assert JAR.is_file(), "Build the JDBC helper first"

with tempfile.TemporaryDirectory(prefix="kode-query-pressure-") as directory:
    env = os.environ.copy()
    for name in ("JAVA_TOOL_OPTIONS", "_JAVA_OPTIONS", "JDK_JAVA_OPTIONS"):
        env.pop(name, None)
    env["KODE_RS_JDBC_HOME"] = str(Path(directory) / "unused-driver-installs")
    env["KODE_LEGACY_JDBC_HOME"] = ""
    pipe_read, pipe_write = os.pipe()
    output = os.fdopen(pipe_read, "rb", buffering=0)
    child = subprocess.Popen([os.environ.get("KODE_TEST_JAVA", "java"), "-Xmx256m", "-jar", str(JAR)],
        stdin=subprocess.PIPE, stdout=pipe_write, stderr=subprocess.PIPE,
        cwd=directory, env=env, start_new_session=True, bufsize=0)
    tail = bytearray()

    def drain_stderr():
        while True:
            data = os.read(child.stderr.fileno(), 8192)
            if not data:
                return
            tail.extend(data)
            if len(tail) > 65536:
                del tail[:-65536]

    stderr_worker = threading.Thread(target=drain_stderr, daemon=True)
    stderr_worker.start()
    buffered = bytearray()

    def send(request_id, method, params):
        data = json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}, ensure_ascii=False).encode()
        frame = memoryview(struct.pack(">I", len(data)) + data)
        while frame:
            written = os.write(child.stdin.fileno(), frame)
            frame = frame[written:]

    def exact(count, deadline):
        while len(buffered) < count:
            remaining = deadline - time.monotonic()
            assert remaining > 0, "JDBC fixture read timed out"
            ready, _, _ = select.select([output], [], [], remaining)
            assert ready, "JDBC fixture read timed out"
            data = os.read(output.fileno(), 65536)
            assert data, "JDBC helper exited before its expected response"
            buffered.extend(data)
        result = bytes(buffered[:count])
        del buffered[:count]
        return result

    def read():
        deadline = time.monotonic() + 20
        length = struct.unpack(">I", exact(4, deadline))[0]
        assert 0 < length <= 8388608, "Invalid JDBC fixture frame size"
        return json.loads(exact(length, deadline))

    def rpc(request_id, method, params):
        send(request_id, method, params)
        value = read()
        assert value.get("id") == request_id and "result" in value, "Unexpected JDBC fixture response"
        return value["result"]

    try:
        hello = rpc("hello", "hello", {"protocolVersion": 1})
        assert "query.execute" in hello["capabilities"]
        driver = rpc("driver", "driver.inspect", {"driver": {"id": "h2", "driverClass": "org.h2.Driver", "coordinates": "com.h2database:h2:2.2.224"}})
        connection = rpc("open", "connection.open", {"dataSourceId": "synthetic-pressure", "attemptId": "pressure-open", "loaderId": driver["loaderId"], "driverClass": driver["selectedDriverClass"], "jdbcUrl": "jdbc:h2:mem:pressure", "properties": {"user": "sa", "password": "synthetic-pressure-password"}})
        assert not buffered
        send("execute", "query.execute", {"connectionId": connection["connectionId"], "executionId": "pressure-query", "sql": "select repeat('x', 65536) from system_range(1,100000)", "maxRows": 100000, "fetchSize": 200})
        capacity = fcntl.fcntl(output.fileno(), fcntl.F_GETPIPE_SZ)
        deadline = time.monotonic() + 5
        while True:
            available = array.array("i", [0])
            fcntl.ioctl(output.fileno(), termios.FIONREAD, available)
            writable = bool(select.select([], [pipe_write], [], 0)[1])
            # Pipe ring slots can be full while a partially occupied page leaves
            # FIONREAD below nominal capacity. Poll the actual writer readiness.
            if available[0] > 0 and not writable:
                break
            assert time.monotonic() < deadline, "Fixture did not fill the OS output pipe"
            assert child.poll() is None, "Helper exited while filling its output pipe"
            time.sleep(0.01)
        time.sleep(0.3)
        send("cancel", "query.cancel", {"executionId": "pressure-query"})
        # Leave the OS pipe full while input/control/cancellation process this
        # request. Control output can queue independently behind the current frame.
        time.sleep(0.2)
        os.close(pipe_write)
        pipe_write = None
        events = 0
        rows = 0
        acknowledgement = None
        final = None
        frames_before_ack = 0
        while final is None or acknowledgement is None:
            value = read()
            if acknowledgement is None:
                frames_before_ack += 1
            if value.get("method") == "query.event":
                params = value["params"]
                assert params["executionId"] == "pressure-query" and params["sequence"] == events
                events += 1
                if params["event"]["type"] == "rows":
                    batch = params["event"]["rows"]
                    assert len(batch) <= 200
                    rows += len(batch)
            elif value.get("id") == "cancel":
                acknowledgement = value["result"]
                assert final is None, "Control acknowledgement did not retain priority"
            elif value.get("id") == "execute":
                final = value["result"]
                assert final["eventCount"] == events, "Query final overtook its own events"
            else:
                raise AssertionError("Unexpected JDBC pressure fixture frame")
        assert acknowledgement["status"] == "requested"
        assert final["outcome"] == "cancelled" and final["previewIncomplete"]
        assert rows > 0
        send("next", "query.execute", {"connectionId": connection["connectionId"], "executionId": "next-query", "sql": "select 42"})
        next_events = 0
        next_value = None
        while True:
            value = read()
            if value.get("method") == "query.event":
                params = value["params"]
                assert params["executionId"] == "next-query" and params["sequence"] == next_events
                next_events += 1
                if params["event"]["type"] == "rows":
                    next_value = params["event"]["rows"][0][0]["text"]
            else:
                assert value.get("id") == "next" and value["result"]["outcome"] == "completed"
                assert value["result"]["eventCount"] == next_events and next_value == "42"
                break
        assert rpc("shutdown", "shutdown", {})["closing"]
        child.stdin.close()
        assert child.wait(timeout=5) == 0
        stderr_worker.join(timeout=1)
        evidence = {"engine": "H2 2.2.224", "protocol": hello["protocolVersion"], "osPipeCapacity": capacity,
            "bufferedBytesBeforeCancel": available[0], "osPipeWritableBeforeCancel": writable, "pipeWasFull": True, "cancelStatus": acknowledgement["status"],
            "framesThroughCancelAck": frames_before_ack, "eventsBeforeFinal": events, "finalEventCount": final["eventCount"],
            "receivedRows": rows, "outcome": final["outcome"], "previewIncomplete": final["previewIncomplete"],
            "nextQueryCompleted": True, "childReaped": True, "exitCode": child.returncode, "rawStderrSuppressed": True, "passed": 1}
        (ROOT / "docs/parity/D05-backpressure.json").write_text(json.dumps(evidence, indent=2) + "\n")
        print("Passed actual JDBC full-pipe cancellation, event/final ordering, connection reuse and child reaping")
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait(timeout=5)
        stderr_worker.join(timeout=1)
        if pipe_write is not None:
            os.close(pipe_write)
        output.close()
