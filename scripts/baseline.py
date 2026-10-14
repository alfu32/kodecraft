#!/usr/bin/env python3
"""Capture licensed reference inventories and create only synthetic fixtures."""
import argparse
import csv
import hashlib
import json
import re
import struct
import subprocess
import zlib
from pathlib import Path
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT.parent / "kode"
BASELINE = "7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68"
PARITY = ROOT / "docs/parity"
FIXTURES = ROOT / "fixtures"


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def inventory():
    commit = subprocess.check_output(["git", "-C", str(SOURCE), "rev-parse", "HEAD"], text=True).strip()
    if commit != BASELINE:
        raise SystemExit(f"Unexpected source commit: {commit}")
    ids = set(re.findall(r"^\| ([A-Z]\d{2}) \|", (ROOT / "docs/migration/TASKS.md").read_text(), re.M))
    maps = {}
    for name, folder, key in [("SOURCE-MAP.tsv", "src/main/kotlin", "source_path"),
                               ("TEST-MAP.tsv", "src/test/kotlin", "source_test_path")]:
        with (ROOT / "docs/migration" / name).open() as stream:
            entries = {row[key]: row for row in csv.DictReader(stream, delimiter="\t")}
        actual = {p.relative_to(SOURCE).as_posix() for p in (SOURCE / folder).rglob("*.kt")}
        if actual != entries.keys():
            raise SystemExit(f"{name}: missing {actual - entries.keys()}, obsolete {entries.keys() - actual}")
        for entry in entries.values():
            tasks = [entry["primary_task"]] + entry["additional_tasks"].split(",")
            if any(task and task not in ids for task in tasks):
                raise SystemExit(f"Unknown owner: {entry}")
        maps.update(entries)
    records = []
    branches = []
    tests = []
    ux = []
    for line in (ROOT / "docs/migration/EDITOR-UX.md").read_text().splitlines():
        if line.startswith("| UX-"):
            columns = [p.strip() for p in line.split("|")[1:-1]]
            ux.append({"id": columns[0], "input": columns[1], "expected": columns[2], "owners": columns[3].split("/")})
    for relative, owner in sorted(maps.items()):
        path = SOURCE / relative
        raw = path.read_bytes()
        text = raw.decode()
        records.append({"path": relative, "sha256": hashlib.sha256(raw).hexdigest(),
                        "lines": len(text.splitlines()), "owner": owner["primary_task"],
                        "additional": list(filter(None, owner["additional_tasks"].split(",")))})
        if relative.startswith("src/test/"):
            for match in re.finditer(r"@Test[\s\S]*?fun\s+(`[^`]+`|\w+)\s*\(", text):
                tests.append({"path": relative, "line": text[:match.start()].count("\n") + 1,
                              "test": match[1].strip("`"), "owner": owner["primary_task"]})
        elif "UIEvent" in text:
            applicable = [u["id"] for u in ux if owner["primary_task"] in u["owners"]]
            handler = ""
            for number, line in enumerate(text.splitlines(), 1):
                function = re.search(r"fun\s+(\w+)\s*\(", line)
                if function:
                    handler = function[1]
                # Conservatively retain all decision sites in UI-event components,
                # including helper branches not spelled as event.kind/key checks.
                if re.search(r"\b(if|when)\s*\(|->", line) and not line.lstrip().startswith(("//", "*")):
                    branches.append({"case": f"BEH-{path.stem}-{number}", "path": relative,
                                     "line": number, "function": handler,
                                     "condition": line.strip(), "owner": owner["primary_task"],
                                     "editor_acceptance": applicable})
    resources = []
    paths = list((SOURCE / "src/main/resources").rglob("*"))
    paths += [SOURCE / p for p in ["styles/app.css", "keyword-patterns.txt", "token-colors.txt", "codeintel/definitions.json", "lsp/servers.json"]]
    for path in sorted(p for p in paths if p.is_file()):
        raw = path.read_bytes()
        resources.append({"path": path.relative_to(SOURCE).as_posix(), "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()})
    write_json(PARITY / "source-inventory.json", {"commit": commit, "files": records})
    write_json(PARITY / "test-inventory.json", tests)
    write_json(PARITY / "event-branches.json", branches)
    write_json(PARITY / "editor-cases.json", ux)
    write_json(PARITY / "resource-inventory.json", resources)
    print(f"Verified {len(records)} source/test owners; {len(tests)} named tests; {len(branches)} UI decision sites; {len(ux)} editor acceptance cases")


def fixtures():
    manifest = []
    def put(relative, data, expected, origin="Synthetic migration fixture; CC0-1.0", license="CC0-1.0"):
        path = FIXTURES / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(data, str):
            data = data.encode()
        path.write_bytes(data)
        manifest.append({"path": relative, "origin": origin, "license": license,
                         "sha256": hashlib.sha256(data).hexdigest(), "expected": expected})
    for name, data in {"empty.txt": b"", "no-final-newline.txt": b"one\ntwo", "lf.txt": b"one\ntwo\n",
                       "crlf.txt": b"one\r\ntwo\r\n", "bom.txt": b"\xef\xbb\xbfhello\n",
                       "unicode.txt": "a\t\x01😀中e\u0301\n", "long-line.txt": "x" * 100000,
                       "many-lines.txt": "line\n" * 100000}.items():
        put("editor/" + name, data, "Exact original bytes survive load/save; scalar-boundary navigation; immutable snapshots")
    put("editor/display.json", json.dumps({"input": "a\t\u0001😀中é", "display": "a|-->\\x01\\x1F600\\x4E2De\\x0301", "tab_width": 4}), "Escaped compatibility display; one logical step per scalar")
    state = {"version": 6, "recentFiles": [], "sourceRoots": ["src"], "scanExclusions": ["src/generated"],
             "openEditors": [{"path": "src/example.kt", "buffer": {"lines": ["a😀b", "dirty"],
              "cursor": {"line": 0, "column": 3}, "anchor": {"line": 0, "column": 1},
              "undo": [{"lines": ["a😀b"], "cursor": {"line": 0, "column": 3}, "dirty": False}],
              "redo": [], "dirty": True}, "scrollTop": 0, "language": "kotlin", "lastModifiedMillis": 1}],
             "database": {}, "restApi": {}, "futureExtension": {"keep": True}}
    put("sessions/legacy-v6.json", json.dumps(state), "UTF-16 cursor 3 → UTF-8 byte 5; reverse selection, dirty history, unknown fields preserved")
    postman = {"info": {"name": "Synthetic", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
               "variable": [{"key": "base", "value": "http://127.0.0.1", "disabled": False}],
               "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "synthetic-token"}]},
               "item": [{"name": "Folder", "x-kode-headers": [{"key": "X-Folder", "value": "yes"}],
                         "item": [{"name": "Request", "event": [{"listen": "test", "script": {"exec": ["// retained"]}}],
                                   "request": {"method": "POST", "url": "{{base}}/items/:id", "header": [{"key": "X-A", "value": "one"}, {"key": "X-A", "value": "two"}, {"key": "X-Disabled", "value": "skip", "disabled": True}],
                                               "body": {"mode": "raw", "raw": "{\"a\":1}", "options": {"raw": {"language": "json"}}}},
                                   "response": [{"name": "Example", "code": 200}], "future": {"number": 123456789012345678901234567890}}]}],
               "future": [True, None, "keep"]}
    put("rest/collection-v21.json", json.dumps(postman), "Semantic lossless round trip including order, duplicates, scripts, unknown numbers and examples")
    put("rest/environment.json", json.dumps({"name": "Synthetic", "values": [{"key": "base", "value": "http://127.0.0.1", "enabled": True}, {"key": "disabled", "value": "no", "enabled": False}], "future": "keep"}), "Enabled flags and unknown fields preserved")
    for version in ["2.0", "3.0.3", "3.1.0"]:
        doc = {"info": {"title": "Synthetic", "version": "1"}, "paths": {"/items/{id}": {"get": {"operationId": "getItem", "parameters": [{"name": "id", "in": "path", "required": True, "type": "string"}] if version == "2.0" else [{"name": "id", "in": "path", "required": True, "schema": {"type": "string"}}], "responses": {"200": {"description": "ok"}}}}}}
        doc["swagger" if version == "2.0" else "openapi"] = version
        if version == "2.0":
            doc.update({"host": "127.0.0.1", "schemes": ["http"], "basePath": "/api"})
        else:
            doc["servers"] = [{"url": "http://127.0.0.1/api"}]
        put(f"rest/openapi-{version}.json", json.dumps(doc), "Convert GET operation and required path parameter into Postman request")
        put(f"rest/openapi-{version}.yaml", ('swagger: "2.0"\n' if version == "2.0" else f'openapi: "{version}"\n') + 'info: {title: Synthetic, version: "1"}\npaths:\n  /items:\n    get:\n      responses:\n        "200": {description: ok}\n', "JSON/YAML import parity; source schema retained")
    put("languages/kotlin/model.kt", "package demo\nopen class Parent { val inherited: String = \"x\" }\nclass Customer(val name: String) : Parent()\nfun create(): Customer = Customer(\"Ada\")\n", "Cross-file return inference, constructor fields and inherited members")
    put("languages/kotlin/use.kt", "package demo\nfun use() { val customer = create(); println(customer.name); println(customer.inherited) }\n", "Receiver lookup; usages resolve to declarations independent of parse order")
    put("languages/c/model.h", "typedef struct Customer { int value; } Customer;\nCustomer *create(void);\n", "Tag/typedef identity and pointer members")
    put("languages/c/use.c", '#include "model.h"\nint use(Customer *customer) { return customer->value; }\n', "Cross-file pointer member resolution")
    put("jdbc/statements.sql", "select ';' as quoted; -- comment ;\nselect 2;\n/* ; */ select 3;", "SQL locator ignores quoted/comment semicolons")
    put("viewers/binary.bin", bytes(range(256)), "Byte viewer covers all values without lossy text decoding")
    put("viewers/rgba.svg", '<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red"/></svg>', "SVG rasterizes to four red pixels")
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(b"\x00" + bytes([255, 0, 0, 255]) * 2 + b"\x00" + bytes([0, 0, 255, 255]) * 2)) + chunk(b"IEND", b"")
    put("viewers/rgba.png", png, "2×2; red top row, blue bottom row; alpha retained")
    for relative in ["src/main/resources/database-jdbc-url-templates.json", "keyword-patterns.txt", "token-colors.txt", "codeintel/definitions.json", "lsp/servers.json", "styles/app.css"]:
        target = "reference/" + relative.removeprefix("src/main/resources/")
        put(target, (SOURCE / relative).read_bytes(), "All shipped entries retained; same source baseline", f"../kode/{relative} at {BASELINE}", "MIT")
    put("reference/LICENSE-KODE.txt", (SOURCE / "LICENSE.md").read_bytes(), "Retain source license with copied catalogs", f"../kode/LICENSE.md at {BASELINE}", "MIT")
    sequences = [
        {"name": "newline-aliases", "ux": ["UX-003"], "text": "first", "cursor_byte": 5, "events": ["enter", "RETURN", "CR"], "expected_text": "first\n\n\n"},
        {"name": "reverse-selection-paste", "ux": ["UX-004", "UX-012", "UX-015"], "text": "abc\ndef", "selection": {"anchor": 6, "head": 1}, "events": [{"paste": "X\nY"}, "undo", "redo"], "expected_text": "aX\nYf"},
        {"name": "modal-focus", "ux": ["UX-105"], "events": ["open-dialog", "type:a", "tab", "tab", "escape"], "expected": "Field contains a; field→OK→Cancel; underlying editor unchanged; previous focus restored"},
        {"name": "popup-outside-click", "ux": ["UX-084", "UX-097"], "events": ["open-popup", "mouse-down:outside"], "expected": "Popup immediately dismissed; event consumed; underlying caret unchanged"},
        {"name": "wrapped-pointer-selection", "ux": ["UX-033", "UX-044"], "text": "\t" * 20, "events": ["render:20x6", "mouse-down:text", "mouse-drag:wrapped-row", "mouse-up:outside"], "expected": "Cell-to-byte hit map shared with renderer; drag capture released"},
    ]
    put("editor/event-sequences.json", json.dumps(sequences), "Named reference sequences; owners add precise state/cell assertions during implementation")
    write_json(FIXTURES / "manifest.json", {"baseline": BASELINE, "fixtures": manifest})
    print(f"Created {len(manifest)} synthetic/licensed fixtures; no live session or credentials read")


def results():
    suites = []
    for path in sorted((SOURCE / "build/test-results/test").glob("TEST-*.xml")):
        root = ET.parse(path).getroot()
        tests = []
        for case in root.findall("testcase"):
            failure = case.find("failure")
            error = case.find("error")
            problem = failure if failure is not None else error
            tests.append({"name": case.get("name"), "status": "failed" if problem is not None else "skipped" if case.find("skipped") is not None else "passed", "failure": problem.get("message") if problem is not None else None})
        suites.append({"name": root.get("name"), "timestamp": root.get("timestamp"), "tests": tests,
                       "failures": int(root.get("failures", 0)), "errors": int(root.get("errors", 0))})
    write_json(PARITY / "kotlin-results.json", {"baseline": BASELINE, "suites": suites})
    print(f"Baseline: {len(suites)} suites, {sum(len(s['tests']) for s in suites)} tests, {sum(s['failures'] + s['errors'] for s in suites)} failures/errors")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["inventory", "fixtures", "results"])
    globals()[parser.parse_args().action]()
