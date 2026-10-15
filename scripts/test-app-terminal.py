#!/usr/bin/env python3
"""Run the real CLI in synthetic PTYs; never open the developer's session."""
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/debug/kode"


def run_case(target, config, size, actions, expected=()):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", size[1], size[0], 0, 0))
    before = termios.tcgetattr(slave)
    env = dict(os.environ, TERM="xterm-256color", XDG_CONFIG_HOME=str(config), KODE_RS_HOME=str(config / "resources"), KODE_HOME=str(config / "legacy-resources"))
    if expected:
        env.pop("NO_COLOR", None)
        env["COLORTERM"] = "truecolor"
    child = subprocess.Popen([str(BINARY), str(target)], stdin=slave, stdout=slave, stderr=slave,
                             start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0), env=env)
    output = bytearray()
    next_action = 0
    last_action = time.monotonic()
    deadline = last_action + 12
    queried = False
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.01)[0]:
                output.extend(os.read(master, 65536))
                if b"\x1b[?u" in output and not queried:
                    os.write(master, b"\x1b[?0u\x1b[?62;1;2c")
                    queried = True
            if next_action < len(actions):
                trigger, delay, payload = actions[next_action]
                visible = re.sub(rb"\[[0-?]*[ -/]*[@-~]", b"", output)
                if (trigger in output or trigger in visible) and time.monotonic() - last_action >= delay:
                    os.write(master, payload)
                    next_action += 1
                    last_action = time.monotonic()
            if child.poll() is not None:
                break
        assert child.poll() is not None, f"application timed out after action {next_action}: {bytes(output[:6000])!r}"
        assert child.wait() == 0, f"application failed: {bytes(output[-4000:])!r}"
        assert next_action == len(actions), "application exited before all input was sent"
        assert termios.tcgetattr(slave) == before, "termios state not restored"
        for marker in [b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?2004h", b"\x1b[?2004l", b"\x1b[?25h"]:
            assert marker in output, f"missing terminal restoration: {marker!r}"
        for marker in expected:
            assert marker in output, f"missing rendered output: {marker!r}"
        return {"size": list(size), "exit": 0, "termios_restored": True, "modes_restored": True, "status": "passed"}
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
        os.close(master)
        os.close(slave)


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-app-pty-") as directory:
        base = Path(directory)
        config = base / "config"
        project = base / "project with spaces"
        project.mkdir()
        file = project / "code file.rs"
        file.write_text("original\n")
        entered = b"\x1b[?1049h"
        cases.append({"case": "file-edit-save-dirty-quit", **run_case(file, config, (100, 30), [
            (b"original", 0.15, b"\x1b[200~saved \x1b[201~"),
            (entered, 0.1, b"\x13"),
            (entered, 0.2, b"\x1b[200~draft \x1b[201~"),
            (entered, 0.1, b"\x11"),
        ])})
        assert file.read_text() == "saved original\n", "physical save did not use the editor buffer"
        state = json.loads((project / ".kode-rs.json").read_text())
        assert state["openEditors"][0]["history"]["current"]["text"] == "saved draft original\n", "dirty buffer was not flushed"
        cases.append({"case": "folder-restores-dirty-session", **run_case(project, config, (100, 30), [(b"draft", 0.2, b"\x11")])})
        for size in [(20, 6), (1, 1)]:
            empty = base / f"empty-{size[0]}"
            empty.mkdir()
            cases.append({"case": "empty-folder-tiny-quit", **run_case(empty, config, size, [(entered, 0.3, b"\x11")])})
        missing = subprocess.run([str(BINARY), str(base / "missing")], capture_output=True)
        assert missing.returncode == 1 and b"startup target" in missing.stderr
        for args in [[], ["--help"], ["--version"], ["version"]]:
            result = subprocess.run([str(BINARY), *args], capture_output=True)
            assert result.returncode == 0 and result.stdout and b"\x1b[?1049h" not in result.stdout
        cases.append({"case": "missing-target-help-version", "status": "passed"})
    (ROOT / "docs/parity/W05-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} real application CLI/PTY checks")


if __name__ == "__main__":
    main()
