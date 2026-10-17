#!/usr/bin/env python3
"""Run the real CLI in synthetic PTYs; never open the developer's session."""
import fcntl
import importlib.util
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
AVAILABLE_BINARIES = [
    path for path in (ROOT / "target/debug/kode", ROOT / "target/release/kode")
    if path.is_file()
]
if not AVAILABLE_BINARIES and "KODE_TERMINAL_BINARY" not in os.environ:
    raise SystemExit("Build kode or set KODE_TERMINAL_BINARY to the application binary")
BINARY = (Path(os.environ["KODE_TERMINAL_BINARY"]).resolve()
          if "KODE_TERMINAL_BINARY" in os.environ
          else max(AVAILABLE_BINARIES, key=lambda path: path.stat().st_mtime))
screen_spec = importlib.util.spec_from_file_location("terminal_screen", ROOT / "scripts/terminal-screen.py")
screen_module = importlib.util.module_from_spec(screen_spec)
screen_spec.loader.exec_module(screen_module)


def run_case(target, config, size, actions, expected=(), env_overrides=None, timeout_seconds=12):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", size[1], size[0], 0, 0))
    before = termios.tcgetattr(slave)
    env = dict(os.environ, TERM="xterm-256color", XDG_CONFIG_HOME=str(config), KODE_RS_HOME=str(config / "resources"), KODE_HOME=str(config / "legacy-resources"))
    env.pop("KODE_CODEINTEL_PATH", None)
    env.pop("KODE_LSP_HOME", None)
    env.pop("KODE_LSP_CATALOG", None)
    env.update(env_overrides or {})
    if expected:
        env.pop("NO_COLOR", None)
        env["COLORTERM"] = "truecolor"
    child = subprocess.Popen([str(BINARY), str(target)], stdin=slave, stdout=slave, stderr=slave,
                             start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0), env=env)
    output = bytearray()
    screen = screen_module.Screen(*size)
    seen_expected = set()
    next_action = 0
    last_action = time.monotonic()
    deadline = last_action + timeout_seconds
    queried = False
    resizes = []
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.01)[0]:
                chunk = os.read(master, 65536)
                output.extend(chunk)
                screen.feed(chunk)
                for marker in expected:
                    if marker in output or marker in screen.text():
                        seen_expected.add(marker)
                if b"\x1b[?u" in output and not queried:
                    os.write(master, b"\x1b[?0u\x1b[?62;1;2c")
                    queried = True
            if next_action < len(actions):
                trigger, delay, payload = actions[next_action]
                visible = re.sub(rb"\[[0-?]*[ -/]*[@-~]", b"", output)
                if isinstance(trigger, tuple):
                    marker, occurrence = trigger
                    matched = output.count(marker) >= occurrence
                else:
                    matched = trigger in output or trigger in visible or trigger in screen.text()
                if matched and time.monotonic() - last_action >= delay:
                    selected = payload(screen) if callable(payload) else payload
                    if selected is not None:
                        if isinstance(selected, dict) and "resize" in selected:
                            columns, rows = selected["resize"]
                            assert columns > 0 and rows > 0
                            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
                            screen = screen_module.Screen(columns, rows)
                            os.kill(child.pid, signal.SIGWINCH)
                            resizes.append([columns, rows])
                        else:
                            os.write(master, selected)
                        next_action += 1
                        last_action = time.monotonic()
            if child.poll() is not None:
                break
        assert child.poll() is not None, f"application timed out after action {next_action}:\n{screen.text().decode('utf-8')}"
        assert child.wait() == 0, f"application failed: {bytes(output[-4000:])!r}"
        assert next_action == len(actions), "application exited before all input was sent"
        assert termios.tcgetattr(slave) == before, "termios state not restored"
        for marker in [b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?2004h", b"\x1b[?2004l", b"\x1b[?25h"]:
            assert marker in output, f"missing terminal restoration: {marker!r}"
        for marker in expected:
            assert marker in seen_expected, f"missing rendered output: {marker!r}"
        result = {"size": list(size), "exit": 0, "termios_restored": True, "modes_restored": True, "status": "passed"}
        if resizes:
            result["resizes"] = resizes
        return result
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
        os.close(master)
        os.close(slave)


def run_tmux_case(target, config, size, timeout_seconds=12):
    """Run the CLI inside a real tmux pane and send its quit key through tmux."""
    server = f"kode-app-terminal-{os.getpid()}"
    with tempfile.TemporaryDirectory(prefix="kode-app-tmux-") as directory:
        directory = Path(directory)
        config_file = directory / "tmux.conf"
        config_file.write_text(
            "set -g prefix C-b\n"
            "set -g status off\n"
            "set -g default-terminal tmux-256color\n"
        )
        wrapper = directory / "check-term-and-exec"
        wrapper.write_text(
            "#!/bin/sh\n"
            "test \"$TERM\" = tmux-256color || exit 93\n"
            f"exec {BINARY} \"$@\"\n"
        )
        wrapper.chmod(0o755)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", size[1], size[0], 0, 0))
        before = termios.tcgetattr(slave)
        env = dict(
            os.environ,
            TERM="xterm-256color",
            XDG_CONFIG_HOME=str(config),
            KODE_RS_HOME=str(config / "resources"),
            KODE_HOME=str(config / "legacy-resources"),
        )
        command = [
            "tmux", "-L", server, "-f", str(config_file), "new-session",
            "-s", "kode-app", "-x", str(size[0]), "-y", str(size[1]), "--",
            str(wrapper), str(target),
        ]
        child = subprocess.Popen(
            command,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            start_new_session=True,
            preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0),
            env=env,
        )
        output = bytearray()
        deadline = time.monotonic() + timeout_seconds
        sent_quit = False
        try:
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.02)[0]:
                    try:
                        output.extend(os.read(master, 65536))
                    except OSError:
                        break
                if not sent_quit and b"tmux app probe" in output:
                    time.sleep(0.2)
                    result = subprocess.run(
                        ["tmux", "-L", server, "send-keys", "-t", "kode-app:0.0", "C-q"],
                        capture_output=True,
                        timeout=3,
                    )
                    assert result.returncode == 0, f"tmux could not send Ctrl+Q: {result.stderr!r}"
                    sent_quit = True
                if child.poll() is not None:
                    break
            assert child.poll() is not None, "application timed out inside tmux"
            assert child.wait() == 0, f"tmux/application failed: {bytes(output[-4000:])!r}"
            assert sent_quit, "application did not render the tmux probe before quit"
            assert termios.tcgetattr(slave) == before, "outer PTY termios state not restored"
            for marker in [b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?2004h", b"\x1b[?2004l"]:
                assert marker in output, f"missing tmux terminal transition: {marker!r}"
            return {
                "case": "application-exits-cleanly-inside-tmux",
                "size": list(size),
                "terminal": "tmux-256color",
                "pane_term_verified_by_wrapper": True,
                "quit_delivered_by": "tmux send-keys C-q",
                "exit": 0,
                "outer_termios_restored": True,
                "modes_restored": True,
                "status": "passed",
            }
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
            subprocess.run(["tmux", "-L", server, "kill-server"], capture_output=True, timeout=3)
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
        find_file = project / "find resize.rs"
        find_file.write_text("first marker\nsecond marker\n")
        find_case = run_case(find_file, config, (100, 30), [
            (b"first marker", 0.15, b"\x06"),
            (b"Find:", 0.15, b"\x1b[200~marker\x1b[201~"),
            (b"Find:", 0.15, lambda screen: {"resize": (60, 18)}
             if b"Find: marker" in screen.text() else None),
            (b"Find:", 0.15, lambda screen: {"resize": (100, 30)}
             if b"Find: marker" in screen.text() else None),
            (b"Find:", 0.15, lambda screen: b"\x1b"
             if b"Find: marker" in screen.text() else None),
            (entered, 0.15, lambda screen: b"\x1b[200~edited \x1b[201~"
             if b"Find:" not in screen.text() and b"first marker" in screen.text() else None),
            (b"edited", 0.15, b"\x13"),
            (entered, 0.3, b"\x11"),
        ], expected=[b"Find: marker", b"edited first marker"])
        assert find_file.read_text() == "edited first marker\nsecond marker\n", "find/resize changed the editor buffer or save target"
        find_case.update({"case": "file-find-resize-edit-save", "find_query_rendered": True,
                          "search_focus_restored": True, "saved_editor_buffer_verified": True})
        cases.append(find_case)
        unicode_file = project / "unicode control.rs"
        original_bytes = "a\t界😀e\u0301\x01z".encode()
        unicode_file.write_bytes(original_bytes)
        inserted = "Ω-".encode()
        unicode_case = run_case(unicode_file, config, (100, 30), [
            (b"|-->", 0.15, lambda screen: b"\x1b[200~" + inserted + b"\x1b[201~"
             if b"\\x754C" in screen.text() and b"\\x1F600" in screen.text() else None),
            (inserted, 0.15, b"\x13"),
            (entered, 0.2, b"\x13"),
            (entered, 0.2, b"\x11"),
        ], expected=[b"|-->", b"\\x754C", b"\\x1F600"])
        assert unicode_file.read_bytes() == inserted + original_bytes, \
            "Unicode, control, tab or missing final newline changed during repeated terminal saves"
        unicode_case.update({"case": "file-unicode-control-exact-save",
                             "bracketed_utf8_paste_verified": True,
                             "repeated_save_exact_bytes_verified": True})
        cases.append(unicode_case)
        fallback_file = project / "clipboard fallback.rs"
        fallback_file.write_text("copy-source")
        fallback_case = run_case(fallback_file, config, (100, 30), [
            (b"copy-source", 0.15, b"\x01\x03"),
            (entered, 0.2, b"\x1b[F\x16"),
            (b"copy-sourcecopy-source", 0.15, b"\x13"),
            (entered, 0.2, b"\x11"),
        ], expected=[b"copy-sourcecopy-source"],
            env_overrides={"DISPLAY": "", "WAYLAND_DISPLAY": ""})
        assert fallback_file.read_text() == "copy-sourcecopy-source", \
            "clipboard fallback failed without a desktop display"
        fallback_case.update({"case": "file-clipboard-without-desktop",
                              "desktop_clipboard_unavailable": True,
                              "internal_copy_paste_verified": True})
        cases.append(fallback_case)
        shell_file = project / "shell roundtrip.rs"
        shell_file.write_text("original\n")
        shell_marker = "KODE_SHELL_HOP_READY_7B"
        shell_format = "".join(f"\\{ord(char):03o}" for char in shell_marker) + "\\n"
        shell_print = f"printf '{shell_format}'\npwd\n".encode()
        resumed = (entered, 2)
        shell_case = run_case(
            shell_file,
            config,
            (100, 30),
            [
                (b"original", 0.15, b"\x1b[200~before-\x1b[201~"),
                (b"before-", 0.15, b"\x14"),
                (b"[ kode ] Workspace shell.", 0.1, shell_print),
                (shell_marker.encode(), 0.1, b"exit\n"),
                (resumed, 0.3, b"\x1b[200~after-\x1b[201~"),
                (resumed, 0.15, b"\x13"),
                (resumed, 0.2, b"\x11"),
            ],
            expected=[shell_marker.encode(), str(project).encode()],
            env_overrides={"SHELL": "/bin/sh"},
        )
        assert shell_file.read_text() == "before-after-original\n", "editor buffer changed across shell suspend/resume"
        shell_case.update({
            "case": "editor-shell-suspend-resume-preserves-buffer",
            "shell": "/bin/sh",
            "shell_marker_verified": shell_marker,
            "workspace_root_verified": True,
            "saved_editor_buffer": "before-after-original\n",
        })
        cases.append(shell_case)
        for size in [(20, 6), (1, 1)]:
            empty = base / f"empty-{size[0]}"
            empty.mkdir()
            cases.append({"case": "empty-folder-tiny-quit", **run_case(empty, config, size, [(entered, 0.3, b"\x11")])})
        # Exercise the actual app lifecycle with several installed terminfo
        # profiles. The real tmux pane below separately verifies tmux's TERM.
        for terminal in ["screen-256color", "tmux-256color", "vt100", "ansi"]:
            target = project / f"terminal-{terminal}.rs"
            target.write_text(f"{terminal} app probe\n")
            result = run_case(
                target,
                config,
                (100, 30),
                [(entered, 0.2, b"\x11")],
                env_overrides={"TERM": terminal},
            )
            result.update({"case": "application-startup-quit-term-profile", "terminal": terminal})
            cases.append(result)
        tmux_probe = project / "tmux probe.rs"
        tmux_probe.write_text("tmux app probe\n")
        cases.append(run_tmux_case(tmux_probe, config, (100, 30)))
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
