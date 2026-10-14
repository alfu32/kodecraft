#!/usr/bin/env python3
"""Exercise the actual terminal backend and verify the original termios state."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
PROBE = ROOT / "target/debug/examples/terminal_probe"


def run_case(mode, size, payload=None, expected=None):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", size[1], size[0], 0, 0))
    before = termios.tcgetattr(slave)
    env = dict(os.environ, TERM="xterm-256color")
    process = subprocess.Popen([str(PROBE), mode], stdin=slave, stdout=slave, stderr=slave,
                               start_new_session=True,
                               preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0),
                               env=env)
    output = bytearray()
    sent = False
    deadline = time.monotonic() + 5
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.01)[0]:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    chunk = b""
                output.extend(chunk)
                if mode == "enhanced" and b"\x1b[?u" in chunk:
                    os.write(master, b"\x1b[?1u\x1b[?62;1;2c")
            trigger = b"\x1b[?1049l" if mode == "suspend" else b"\x1b[?1049h"
            if payload is not None and not sent and trigger in output:
                os.write(master, payload)
                sent = True
            if process.poll() is not None:
                while select.select([master], [], [], 0)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    output.extend(chunk)
                break
        else:
            raise AssertionError(f"PTY probe timed out: {mode}, {size}, {bytes(output)!r}")
        code = process.wait(timeout=1)
        after = termios.tcgetattr(slave)
        assert after == before, f"termios not restored for {mode} at {size}"
        assert b"\x1b[?1049h" in output and b"\x1b[?1049l" in output, "alternate screen not restored"
        assert b"\x1b[?2004h" in output and b"\x1b[?2004l" in output, "bracketed paste not restored"
        assert b"\x1b[?25h" in output, "cursor not restored"
        if expected:
            assert expected.encode() in output, f"missing {expected!r}: {bytes(output)!r}"
        assert (code != 0) if mode in ("error", "panic") else (code == 0), f"unexpected exit {code}: {bytes(output)!r}"
        return {"mode": mode, "size": list(size), "exit": code, "original_termios_restored": True,
                "screen_cursor_paste_restored": True, "expected": expected, "status": "passed"}
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        os.close(master)
        os.close(slave)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", default="docs/parity/F04-pty.json")
    args = parser.parse_args()
    cases = []
    for size in [(80, 24), (120, 40), (20, 6), (1, 1)]:
        for mode in ["normal", "error", "panic"]:
            cases.append(run_case(mode, size))
    cases.append(run_case("input", (80, 24), b"\x1b[6~", "PageDown"))
    cases.append(run_case("input", (80, 24), b"\x00", "ctrl: true"))
    cases.append(run_case("input", (80, 24), b"\x1b[200~paste\xf0\x9f\x98\x80\x1b[201~", "Paste(\"paste😀\")"))
    cases.append(run_case("suspend", (80, 24), b"hello-shell\n", "suspended-and-resumed"))
    cases.append(run_case("enhanced", (80, 24), expected="enhanced:true"))
    target = ROOT / args.report
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} PTY checks: restoration/error/panic/tiny viewport, PageDown, NUL, paste and shell suspension")
