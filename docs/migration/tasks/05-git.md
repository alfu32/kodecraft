# Git migration tasks

The source uses JGit through `IGitService` and `JGitService`. Rust uses git2/libgit2;
JGit is not retained in the JDBC helper.

## G01 — Repository discovery and read operations

**Status:** in-progress

**Depends:** F03. **Owns:** `kode-git/src/{model,repository,read,status}.rs`.
**Read:** `editor/lib/IGitService.kt`, `GitPanelView.kt` error handling;
JGitServiceTest, GitPanelViewTest.

1. Port repository discovery from nested workspaces and normalized models for
   porcelain-like staged/unstaged/untracked/conflicted status and current branch.
2. Implement branch/commit/tag listing, files for commit, content at commit and
   staged/worktree diff contents. Cover tag ordering semantics in the Kotlin code.
3. Own git2 handles in blocking workers; refresh asynchronously and preserve error
   context. Do not fetch whole large blobs for a status-only request.

**Accept:** temporary repo fixtures with unborn/detached HEAD, nested directories,
   deleted/renamed files, staged plus unstaged changes, binary/large files, and
   missing/corrupt repository. Original large-blob regression and graceful panel
   failure reproduced. Read operations never modify the repository.

## G02 — Stage, unstage, commit, checkout and tags

**Status:** todo

**Depends:** G01, E12, F05. **Owns:** `kode-git/src/write.rs`,
`kode-tui/src/panels/git/` excluding diff renderer.
**Read:** IGitService write methods and implementations, all GitPanelView handlers.

1. Implement stage/unstage for individual and multiple paths, including deleted
   files and unborn HEAD. Refresh status only after operation completion.
2. Implement commit with configured identity/message, branch selection/checkout,
   tag creation and existing-tag move behavior. Respect local changes and surface
   conflicts/errors; never force checkout as an invisible default.
3. Port status rows/staging controls, embedded commit-message editor, commit action,
   expandable history/files, branch/tag menus, separators/scroll/drag and focus.
   Preserve action timing so mouse down/up does not double-submit commits.

**Accept:** repository content/index/HEAD assertions after each operation; dirty
   checkout refusal, missing identity, empty message handling, existing tags and
   failure feedback. Commit editor uses common text/clipboard behavior; status
   refresh never clears typed commit text.

## G03 — Diff views and restoring worktree chunks

**Status:** todo

**Depends:** G02, E05, E12, V01. **Owns:** `kode-git/src/diff.rs`,
`kode-tui/src/panels/git/diff/`.
**Read:** `DiffView.kt`, `SideBySideDiffView.kt`, Main's `showDiffInMain`,
`restoreDiffChunk`, `clearDiffViewer` and tab-change handlers.

1. Port side-by-side and active single-diff behavior: line numbers, insertion/
   deletion/change colors, wrapping, synchronized scroll and context toggle (`c`).
   Distinguish historical, staged and unstaged comparisons.
2. Compute stable hunk data once for rendering and actions. Restore only selected
   unstaged chunk after checking file/document revision; refresh Git and open
   editor safely. Do not recompute a different algorithm then reuse an old index.
3. Handle binary/missing content, selection/copy in read-only previews, resize and
   focus; leaving Git tab restores the appropriate ordinary viewer.

**Accept:** insert/delete/replace chunks, repeated lines, final-newline changes,
   staged restore disabled, external file change and dirty open buffer conflicts.
   Restore never overwrites unrelated changes. Diff scroll/context and read-only
   UX-103 pass. Document algorithm differences from JGit HistogramDiff with fixtures.
