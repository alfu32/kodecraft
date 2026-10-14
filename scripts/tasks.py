#!/usr/bin/env python3
"""Keep the task cards, execution ledger and implementation dashboard in sync."""
import argparse
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INDEX = ROOT / "docs/migration/TASKS.md"
CARD_RE = re.compile(r"^## ([A-Z]\d{2}) — (.+)$", re.M)
ROW_RE = re.compile(r"^\| ([A-Z]\d{2}) \| (.+?) \| (.+?) \| (.+?) \|$", re.M)
STATES = ("todo", "in-progress", "done", "blocked")


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
    index = re.sub(r"65 implementation tasks\..*?\n", 
                   "65 implementation tasks. Status is recorded in each task definition and synchronized with [the dashboard](../../DASHBOARD.md).\n", index, count=1)
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
                 "Full Kode → Rust migration. Counts are derived from the status in every task definition.", "",
                 "| To do | In progress | Done | Blocked | Total |",
                 "|------:|------------:|-----:|--------:|------:|",
                 f"| {counts['todo']:>5} | {counts['in-progress']:>11} | {counts['done']:>4} | {counts['blocked']:>7} | {len(entries):>5} |", "",
                 "## Ongoing tasks", ""]
    active = [(k, v) for k, v in entries.items() if v[2] == "in-progress"]
    for task_id, (path, title, _, _) in active:
        dashboard.append(f"- **{task_id}** — [{title}]({path.relative_to(ROOT).as_posix()}#{task_id.lower()}--{re.sub(r'[^a-z0-9 -]', '', title.lower()).replace(' ', '-')})")
    if not active:
        dashboard.append("No task currently in progress.")
    dashboard += ["", "## Task status", "", "| Task | Definition | Status | Evidence |", "|------|---------------------------------------------------------------------------------------------------|-------------|------------------------------|"]
    for task_id in [row[1] for row in rows]:
        path, title, state, _ = entries[task_id]
        report = ROOT / f"docs/parity/{task_id}.md"
        evidence = f"[Report]({report.relative_to(ROOT).as_posix()})" if report.exists() else "—"
        dashboard.append(f"| {task_id:>4} | [{(title + path.relative_to(ROOT).as_posix()):>95}) | {state:>11} | {evidence:>28} |")
    dashboard += ["", "Regenerate with `python3 scripts/tasks.py`; verify with `python3 scripts/tasks.py --check`.", "",
                  "Change a task with `python3 scripts/tasks.py F01 in-progress`. Prerequisites must be done before starting or completing dependent work.", ""]
    target = ROOT / "DASHBOARD.md"
    rendered = "\n".join(dashboard)
    if not target.exists() or target.read_text() != rendered:
        changed.append(target)
        if not check:
            target.write_text(rendered)
    if check and changed:
        raise SystemExit("Out of date: " + ", ".join(str(p.relative_to(ROOT)) for p in changed))
    print(f"Tasks: {counts['todo']} to do / {counts['in-progress']} in progress / {counts['done']} done / {counts['blocked']} blocked / {len(entries)} total")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", nargs="?")
    parser.add_argument("status", nargs="?", choices=STATES)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if bool(args.task) != bool(args.status):
        parser.error("Supply both task and status")
    synchronize(args.task, args.status, args.check)
