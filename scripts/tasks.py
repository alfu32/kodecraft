#!/usr/bin/env python3
"""Keep the task cards, execution ledger and implementation dashboard in sync."""
import argparse
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INDEX = ROOT / "docs/migration/TASKS.md"
CARD_RE = re.compile(r"^## ([A-Z]\d{2}) — (.+)$", re.M)
ROW_RE = re.compile(r"^\| ([A-Z]\d{2}) \| (.+?) \| (.+?) \| (.+?) \|$", re.M)
STATES = ("todo", "in-progress", "done", "blocked", "suspend")
UAT_REQUESTS = [
    ("M001", "Horizontal sidebar tabs in source order", ("Z02",)),
    ("B001", "Ctrl+T shell suspension and return", ("Z01",)),
    ("B002", "Ctrl+F file search; Alt+F project search", ("U01",)),
    ("M002", "Authentication protocols and OAuth browser workflow", ("R09", "R10")),
    ("B003", "Readable multiline database error details", ("U02",)),
    ("M003", "Separate downloadable jlink runtime; retain flexible drivers", ("U03",)),
    ("B004", "Multiple cursors and selections", ("U04",)),
    ("M004", "8/16/32-column hex layouts and text/binary find", ("U05",)),
    ("M005", "Double-click and Alt+double-click word selection", ("U06",)),
    ("M006", "File-tree picker buttons for path inputs", ("U07",)),
    ("B005", "Project-relative open-file labels with optional full paths", ("U08",)),
    ("B006", "Repair missing/failing language server installations", ("U09",)),
    ("B007", "Alt+1..8 follow the current visual tab order", ("U10",)),
    ("M007", "Connection-type JDBC URL template dropdown", ("U11",)),
    ("M008", "Applicable database package listing and DDL", ("U12",)),
    ("B008", "Ephemeral local OAuth callback instead of Postman redirect", ("R10",)),
    ("B009", "Oracle explorer refresh and selected/script statement terminators", ("U13",)),
]


def cards():
    result = {}
    for path in sorted((ROOT / "docs/migration/tasks").glob("*.md")):
        content = path.read_text()
        matches = list(CARD_RE.finditer(content))
        for i, match in enumerate(matches):
            end = matches[i + 1].start() if i + 1 < len(matches) else len(content)
            body = content[match.end():end]
            state = re.search(r"\*\*Status:\*\* (\S+)", body)
            result[match[1]] = (path, match[2], state[1] if state else "todo", body)
    return result


def synchronize(task=None, status=None, check=False):
    entries = cards()
    index = INDEX.read_text()
    rows = list(ROW_RE.finditer(index))
    if set(entries) != {row[1] for row in rows}:
        raise SystemExit("Task card IDs and ledger IDs differ")
    if task:
        if task not in entries:
            raise SystemExit(f"Unknown task: {task}")
        row = next(row for row in rows if row[1] == task)
        if status in ("in-progress", "done"):
            missing = [p for p in re.findall(r"[A-Z]\d{2}", row[3])
                       if entries[p][2] != "done"]
            if missing:
                raise SystemExit(f"Prerequisites are unfinished: {', '.join(missing)}")
        path, title, _, body = entries[task]
        if status == "done" and not (ROOT / f"docs/parity/{task}.md").is_file():
            raise SystemExit("Completion requires a task evidence report")
        body = re.sub(r"\n\*\*Status:\*\* \S+\.?\n", "\n", body, count=1)
        entries[task] = (path, title, status, body)
    changed = []
    by_path = {}
    for task_id, (path, title, state, body) in entries.items():
        if state not in STATES:
            raise SystemExit(f"Invalid status for {task_id}: {state}")
        body = re.sub(r"\n\*\*Status:\*\* \S+\.?\n", "\n", body, count=1)
        section = f"## {task_id} — {title}\n\n**Status:** {state}\n\n" + body.lstrip("\n")
        by_path.setdefault(path, []).append(section.rstrip() + "\n")
    for path, sections in by_path.items():
        old = path.read_text()
        prefix = old[:CARD_RE.search(old).start()]
        new = prefix + "\n".join(sections)
        if old != new:
            changed.append(path)
            if not check:
                path.write_text(new)
    index = re.sub(r"\d+ implementation tasks\..*?\n",
                   f"{len(entries)} implementation tasks. Status is recorded in each task definition and synchronized with [the dashboard](../../DASHBOARD.md).\n", index, count=1)
    index = index.replace("replace `planned` with", "update `todo` to")
    def row_update(match):
        task_id = match[1]
        state = entries[task_id][2]
        report = ROOT / f"docs/parity/{task_id}.md"
        value = f"[{state}](../parity/{task_id}.md)" if report.exists() else state
        return f"| {task_id} | {match[2]} | {match[3]} | {value} |"
    index = ROW_RE.sub(row_update, index)
    if INDEX.read_text() != index:
        changed.append(INDEX)
        if not check:
            INDEX.write_text(index)
    counts = {state: sum(item[2] == state for item in entries.values()) for state in STATES}
    dashboard = ["# Implementation dashboard", "",
                 "Full Kode → Rust migration, including the [finalized UAT batch](docs/migration/UAT.md). Counts are derived from the status in every task definition.", "",
                 "| To do | In progress | Done | Blocked | Suspend | Total |",
                 "|------:|------------:|-----:|--------:|--------:|------:|",
                 f"| {counts['todo']:>5} | {counts['in-progress']:>11} | {counts['done']:>4} | {counts['blocked']:>7} | {counts['suspend']:>7} | {len(entries):>5} |", "",
                 "Suspend means work is paused at the user's request; these tasks have not passed their remaining acceptance gates.", "",
                 "## Ongoing tasks", ""]
    active = [(k, v) for k, v in entries.items() if v[2] == "in-progress"]
    for task_id, (path, title, _, _) in active:
        dashboard.append(f"- **{task_id}** — [{title}]({path.relative_to(ROOT).as_posix()}#{task_id.lower()}--{re.sub(r'[^a-z0-9 -]', '', title.lower()).replace(' ', '-')})")
    if not active:
        dashboard.append("No task currently in progress.")
    suspended = [(k, v) for k, v in entries.items() if v[2] == "suspend"]
    if suspended:
        dashboard += ["", "## Suspended tasks", ""]
        for task_id, (path, title, _, _) in suspended:
            dashboard.append(f"- **{task_id}** — [{title}]({path.relative_to(ROOT).as_posix()}#{task_id.lower()}--{re.sub(r'[^a-z0-9 -]', '', title.lower()).replace(' ', '-')})")
    task_rows = []
    for task_id in [row[1] for row in rows]:
        path, title, state, _ = entries[task_id]
        report = ROOT / f"docs/parity/{task_id}.md"
        evidence = f"[Report]({report.relative_to(ROOT).as_posix()})" if report.exists() else "—"
        definition = f"[{title}]({path.relative_to(ROOT).as_posix()})"
        task_rows.append([task_id, definition, state, evidence])
    headings = ["Task", "Definition", "Status", "Evidence"]
    widths = [max(len(row[column]) for row in [headings] + task_rows) for column in range(4)]
    def aligned_row(row):
        return "| " + " | ".join(value.ljust(width) for value, width in zip(row, widths)) + " |"
    dashboard += ["", "## Task status", "", aligned_row(headings),
                  "|" + "|".join("-" * (width + 2) for width in widths) + "|"]
    dashboard.extend(aligned_row(row) for row in task_rows)
    dashboard += ["", "Regenerate with `python3 scripts/tasks.py`; verify with `python3 scripts/tasks.py --check`.", "",
                  "Change a task with `python3 scripts/tasks.py F01 in-progress`. Prerequisites must be done before starting or completing dependent work.", ""]
    target = ROOT / "DASHBOARD.md"
    rendered = "\n".join(dashboard)
    if not target.exists() or target.read_text() != rendered:
        changed.append(target)
        if not check:
            target.write_text(rendered)
    uat = ["# Finalized UAT tracking", "",
           "The user finalized [this batch](../UAT/UAT.md) on 2026-10-16 and included it in the current implementation goal. The source UAT document is preserved; this table tracks implementation status from the task cards.", "",
           "B002's final mapping supersedes earlier discussion: **Ctrl+F file search; Alt+F project search**. Ctrl+Shift+F can remain an alias but cannot be the required project shortcut because the terminal host intercepts it.", "",
           "M002 uses a selectable/copyable authorization URL for completing login in a GUI browser, as requested in the finalized UAT. M003 keeps the optional Java runtime outside the main application package and preserves current JDBC driver management.", "",
           "| Request | Requirement | Status | Implementation |",
           "|---------|-------------|--------|----------------|"]
    for request_id, title, owners in UAT_REQUESTS:
        states = [entries[owner][2] for owner in owners]
        state = "done" if all(s == "done" for s in states) else next((s for s in ("in-progress", "blocked", "suspend") if s in states), "todo")
        links = ", ".join(f"[{owner}]({entries[owner][0].relative_to(ROOT / 'docs/migration').as_posix()})" for owner in owners)
        uat.append(f"| {request_id} | {title} | **{state}** | {links} |")
    uat += ["", "E13 includes the new shared-editor interactions, Z03 includes the separate runtime artifact, and Z04 requires all UAT implementation tasks and OAuth acquisition. Completed earlier work retains its acceptance evidence; a new request does not imply reopening unrelated finished tasks.", "",
            "Regenerate and verify with `python3 scripts/tasks.py` / `python3 scripts/tasks.py --check`.", ""]
    target = ROOT / "docs/migration/UAT.md"
    rendered = "\n".join(uat)
    if not target.exists() or target.read_text() != rendered:
        changed.append(target)
        if not check:
            target.write_text(rendered)
    if check and changed:
        raise SystemExit("Out of date: " + ", ".join(str(p.relative_to(ROOT)) for p in changed))
    print(f"Tasks: {counts['todo']} to do / {counts['in-progress']} in progress / {counts['done']} done / {counts['blocked']} blocked / {counts['suspend']} suspend / {len(entries)} total")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", nargs="?")
    parser.add_argument("status", nargs="?", choices=STATES)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if bool(args.task) != bool(args.status):
        parser.error("Supply both task and status")
    synchronize(args.task, args.status, args.check)
