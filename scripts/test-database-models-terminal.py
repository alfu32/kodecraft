#!/usr/bin/env python3
"""The native editor starts with no Java/tools and does not read/init DB config."""
import importlib.util
import json
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("app_terminal", ROOT / "scripts/test-app-terminal.py")
terminal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(terminal)

with tempfile.TemporaryDirectory(prefix="kode-db-models-pty-") as directory:
    base = Path(directory)
    project = base / "project"
    project.mkdir()
    path = project / "Note.txt"
    path.write_text("native editor without Java\n")
    catalog = base / "empty-lsp.json"
    catalog.write_text('{"servers":[]}')
    config = base / "config"
    result = terminal.run_case(path, config, (100, 30), [(b"Ready", 0.2, b"\x11")],
        expected=[b"native editor"], env_overrides={"PATH": str(base / "no-tools"),
        "JAVA_HOME": str(base / "no-java"), "KODE_JAVA": str(base / "no-java/bin/java"),
        "KODE_LSP_CATALOG": str(catalog), "KODE_RS_DATABASE_HOME": str(base / "unused-rust-db"),
        "KODE_CONFIG_HOME": str(base / "unused-legacy-config"), "KODE_PLUGINS_HOME": str(base / "unused-plugins")})
    assert path.read_text() == "native editor without Java\n"
    assert not (base / "unused-rust-db").exists()
    assert not (base / "unused-legacy-config").exists()
    assert not (base / "unused-plugins").exists()
    state = json.loads((project / ".kode-rs.json").read_text())
    assert state["database"] == {"dataSources": [], "sessions": []}
    cases = [{"case": "native-editor-without-java-or-database-initialization", **result}]
(ROOT / "docs/parity/D01-pty.json").write_text(json.dumps({"cases": cases, "passed": len(cases)}, indent=2) + "\n")
print("Passed native editor startup with Java unavailable and database paths untouched")
