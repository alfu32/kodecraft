# Finalized UAT tracking

The user finalized [this batch](../UAT/UAT.md) on 2026-10-16 and included it in the current implementation goal. The source UAT document is preserved; this table tracks implementation status from the task cards.

B002's final mapping supersedes earlier discussion: **Ctrl+F file search; Alt+F project search**. Ctrl+Shift+F can remain an alias but cannot be the required project shortcut because the terminal host intercepts it.

M002 uses a selectable/copyable authorization URL for completing login in a GUI browser, as requested in the finalized UAT. M003 keeps the optional Java runtime outside the main application package and preserves current JDBC driver management.

| Request | Requirement | Status | Implementation |
|---------|-------------|--------|----------------|
| M001 | Horizontal sidebar tabs in source order | **done** | [Z02](tasks/10-delivery.md) |
| B001 | Ctrl+T shell suspension and return | **done** | [Z01](tasks/10-delivery.md) |
| B002 | Ctrl+F file search; Alt+F project search | **done** | [U01](tasks/11-uat.md) |
| M002 | Authentication protocols and OAuth browser workflow | **done** | [R09](tasks/08-rest.md), [R10](tasks/08-rest.md) |
| B003 | Readable multiline database error details | **done** | [U02](tasks/11-uat.md) |
| M003 | Separate downloadable jlink runtime; retain flexible drivers | **done** | [U03](tasks/11-uat.md) |
| B004 | Multiple cursors and selections | **in-progress** | [U04](tasks/11-uat.md) |
| M004 | 8/16/32-column hex layouts and text/binary find | **done** | [U05](tasks/11-uat.md) |
| M005 | Double-click and Alt+double-click word selection | **todo** | [U06](tasks/11-uat.md) |
| M006 | File-tree picker buttons for path inputs | **todo** | [U07](tasks/11-uat.md) |
| B005 | Project-relative open-file labels with optional full paths | **done** | [U08](tasks/11-uat.md) |
| B006 | Repair missing/failing language server installations | **todo** | [U09](tasks/11-uat.md) |
| B007 | Alt+1..8 follow the current visual tab order | **done** | [U10](tasks/11-uat.md) |
| M007 | Connection-type JDBC URL template dropdown | **done** | [U11](tasks/11-uat.md) |
| M008 | Applicable database package listing and DDL | **todo** | [U12](tasks/11-uat.md) |
| B008 | Ephemeral local OAuth callback instead of Postman redirect | **done** | [R10](tasks/08-rest.md) |

E13 includes the new shared-editor interactions, Z03 includes the separate runtime artifact, and Z04 requires all UAT implementation tasks and OAuth acquisition. Completed earlier work retains its acceptance evidence; a new request does not imply reopening unrelated finished tasks.

Regenerate and verify with `python3 scripts/tasks.py` / `python3 scripts/tasks.py --check`.
