# Instructions for an implementation agent

## Standing user authorization

For this implementation, the user has already authorized `cargo fmt`,
`cargo test`, and Bash commands needed to build, verify and complete the work.
Do not ask for confirmation again for these routine commands. Keep commands
simple and use existing execution permissions; shell redirections and wrappers
can accidentally trigger redundant approval requests. This does not authorize
unrelated destructive actions or publishing to external services.

## Before editing

1. Read `README.md`, `PLAN.md`, `CONTRACTS.md`, and your task card.
2. Check the source commit and working tree. Read the source paths named in the
   card, then the matching original tests. Source paths in cards are relative to
   `../kode/src/main/kotlin/` unless they begin with `src/test/` or another explicit
   repository path. Destination paths are relative to `kode-rs/`.
3. Check every prerequisite in `TASKS.md`. A prerequisite is complete only when its
   implemented API and evidence exist. Do not silently mock a prerequisite in a
   production path. Test fakes are explicitly allowed.
4. Inspect the actual selected crate version's docs/source. `CRATES.md` records
   research, not a promise that arbitrary latest releases compile together.
5. For editor or embedded-editor changes, select the relevant `UX-*` rows in
   `EDITOR-UX.md`. For JDBC, read `JDBC.md` and its wire examples.

## Implementation boundaries

- Implement one card at a time. Keep shared contract changes small, documented,
  and reflected in dependent cards. Do not independently rename shared DTOs.
- Prefer the selected library's capabilities. Port Kode-specific behavior that
  libraries do not supply. Do not mechanically reproduce JVM object graphs.
- Keep all terminal drawing and interactive state mutation on the application
  thread. Cancel jobs on owner disposal and workspace changes.
- Preserve unknown persisted fields and owned user files. Work with synthetic
  fixtures; do not import `.secrets`, live credentials, or the developer's session
  into tests or documentation.
- Keep `../kode` as the behavioral reference. Migration code belongs in `kode-rs`.
- A successful build, an empty trait implementation, or a visible panel alone does
  not satisfy a task's functional acceptance criteria.

## Evidence and completion

Run the card's focused tests, plus `cargo fmt --all --check`, the affected package's
`cargo check`, and relevant clippy checks. Once the complete workspace exists, the
release task runs the complete suite. JDBC cards additionally run the helper's
Maven tests and Rust/Java protocol fixtures. External-service tests need explicit
fixtures; unavailable vendor databases are recorded as untested, not passed.

Record in the task index or a linked task report:

```text
Task: E06
Status: done | in-progress | blocked
Source baseline: <commit>
Changed paths: ...
Public contracts used/changed: ...
Acceptance IDs and test names: ...
Commands and results: ...
Manual terminal checks: ...
Remaining limitations: ...
Next ready task: ...
```

Use a Conventional Commit summary, for example
`feat(editor): preserve wrapped mouse selection and page navigation`.
No Git commit or publication is required merely to complete a task card.

## Assignment prompt

> Implement task **ID** from `docs/migration/TASKS.md`. Read its linked card,
> `CONTRACTS.md`, and the applicable UX/protocol/compatibility sections. Confirm
> prerequisites from the repository state. Implement only its owned component,
> using the chosen crates and the original Kotlin source/tests as references.
> Complete its acceptance checks, record evidence and remaining limitations, and
> update the ledger. Do not mark partial or stubbed behavior complete.
