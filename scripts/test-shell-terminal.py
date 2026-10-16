#!/usr/bin/env python3
"""Qualify Ctrl+T using controlled shells and the real application PTY."""
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("app_terminal", ROOT / "scripts/test-app-terminal.py")
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)


def main():
    cases = []
    with tempfile.TemporaryDirectory(prefix="kode-shell-") as directory:
        base = Path(directory)
        shell = base / "fixture shell"
        shell.write_text("""#!/usr/bin/python3
import fcntl, json, os, struct, termios
from pathlib import Path
attrs = termios.tcgetattr(0)
assert attrs[3] & termios.ICANON and attrs[3] & termios.ECHO
Path(os.environ['SHELL_TRACE']).write_text(json.dumps({'cwd': os.getcwd(), 'cooked': True}))
print('SHELL_READY', flush=True)
assert input() == 'fixture command'
fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', 26, 77, 0, 0))
print('SHELL_FINISHED', flush=True)
""")
        shell.chmod(0o755)
        for width in [100, 60]:
            project = base / f"project with spaces {width}"
            project.mkdir()
            file = project / "code.rs"
            file.write_text("original\n")
            trace = base / f"trace-{width}.json"
            result = terminal.run_case(file, base / "config", (width, 30), [
                (b"original", 0.15, b"\x1b[200~draft \x1b[201~"),
                (b"draft", 0.1, b"\x14"),
                (b"SHELL_READY", 0.1, b"fixture command\n"),
                (b"SHELL_FINISHED", 0.3, b"\x1b[200~resumed \x1b[201~\x13"),
                (b"resumed", 0.2, b"\x11"),
            ], expected=(b"SHELL_READY", b"SHELL_FINISHED", b"resumed"),
                env_overrides={"SHELL": str(shell), "SHELL_TRACE": str(trace)})
            assert json.loads(trace.read_text()) == {"cwd": str(project), "cooked": True}
            assert file.read_text() == "draft resumed original\n", file.read_text()
            cases.append({"case": "shell-cooked-cwd-resume-save", **result})
        for label, executable in [("missing", base / "missing shell"), ("nonzero", "/bin/false")]:
            project = base / label
            project.mkdir()
            file = project / "code.rs"
            file.write_text("original\n")
            marker = b"Cannot launch workspace shell" if label == "missing" else b"Shell returned"
            result = terminal.run_case(file, base / "config", (100, 30), [
                (b"original", 0.15, b"\x14"),
                (marker, 0.15, b"\x1b[200~recovered \x1b[201~\x13"),
                (b"recovered", 0.2, b"\x11"),
            ], expected=(marker,), env_overrides={"SHELL": str(executable)})
            assert file.read_text() == "recovered original\n"
            cases.append({"case": f"shell-{label}-recovery", **result})
        bash=base / "controlled interactive bash"
        bash.write_text("#!/bin/sh\nexec /bin/bash --noprofile --norc -i\n")
        bash.chmod(0o755)
        project=base / "bash workspace with spaces"
        project.mkdir()
        file=project / "note.txt"
        file.write_text("original\n")
        result=terminal.run_case(file,base / "config",(100,30),[
            (b"original",.15,b"\x14"),
            (b"KODE_BASH_READY> ",.15,b"printf '__KODE_PWD__%s\\n' \"$PWD\"; exit\n"),
            (b"__KODE_PWD__"+str(project).encode(),.3,b"\x1b[200~after bash \x1b[201~\x13"),
            (b"after bash",.2,b"\x11"),
        ],env_overrides={"SHELL":str(bash),"PS1":"KODE_BASH_READY> ","PROMPT_COMMAND":"","BASH_ENV":str(base / "no-bash-env"),"ENV":str(base / "no-shell-env")})
        assert file.read_text()=="after bash original\n"
        cases.append({"case":"real-interactive-bash-job-control-resume-save",**result})
    (ROOT / "docs/parity/Z01-shell-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
    print(f"Passed {len(cases)} real shell suspension PTYs")


if __name__ == "__main__":
    main()
