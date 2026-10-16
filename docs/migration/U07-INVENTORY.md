# U07 path-field and picker inventory

Status: done. All ten rows below are implemented and qualified: 385 ordinary
core/workspace/TUI tests, four explicitly enabled Java/H2 cases and fourteen real
CLI PTYs pass. See [acceptance evidence](../parity/U07.md) and the
[fresh terminal matrix](../parity/U07-pty.json).

## Path inputs and existing directory selectors

| # | Surface / owner | Actual field or operation | Required chooser semantics | Status |
|---|---|---|---|---|
| 1 | REST sidebar (`panels/rest/sidebar/actions.rs`) | Import Postman collection | Existing file; project-relative base or absolute/outside-root path; preserve import confirmation/model ownership | qualified |
| 2 | REST sidebar | Import OpenAPI | Existing file; retain destination collection folder and modal identity | qualified |
| 3 | REST sidebar | Export Postman collection | Save file; allow a new filename and keep the export's existing write guards | qualified |
| 4 | SQL session dialog (`panels/database/session_dialog.rs`, `app/sql.rs`) | `Mode::Save`, file name/path | Save file; current saved path or generated `.sql` filename; do not confuse Catalog/Schema text with paths | qualified |
| 5 | Database connection dialog (`connection_dialog.rs`) | `JAR = 10`, Local JAR(s) | Existing file(s); preserve the rest of the classpath and connection draft | qualified |
| 6 | REST request form (`form/mod.rs`, `form/input.rs`) | `Field::File`, binary body `body.file.src` | Existing file; use request project base and the actual source consumed by body resolution | qualified |
| 7 | REST form row modal (`EntryDialog`) | FormData Value when Type is `file` | Existing file(s); string or JSON path array; preserve row/type/collection identity and other sources | qualified |
| 8 | Files Change workspace | Existing `PickerKind::Workspace` | Existing directory; retain workspace-switch semantics and dirty-state persistence | qualified |
| 9 | Project source roots | Existing `PickerKind::Source` | Existing directory; retain configuration worker and generation guards | qualified |
| 10 | Project scan exclusions | Existing `PickerKind::Exclusion` | Existing file or directory; retain configuration worker and generation guards | qualified |

`TextDialog.path_mode` adds a distinct Browse target and Alt+P to path dialogs.
Local JAR and REST binary fields have adjacent full/compact buttons. Multipart
rows enable choosing only when Type is file. Text entry and cancellation retain
the original field/caret/focus; focused controls remain reachable at 1×1.

`FilePicker` reuses the existing worker-backed FolderPicker tree rows, with
open-file/save-file/directory/existing-path modes, typed-path Go and F4 parent
navigation. Workers prepare the filesystem-volume browser and validate candidates.
Browsing does not switch workspace or load another project's session/configuration.
Selection does not write content; each owner keeps its actual operation's guards.

## Composite sources that must not be flattened

- Local JAR(s) is one string passed to the Java helper. `DriverSpec.java` splits
  it with the native `File.pathSeparator` and accepts at most 256 paths. Preserve
  other entries when adding/replacing a selected component, use the platform's
  delimiter, and never silently turn a legal selected filename into two different
  classpath entries. Driver class, Maven coordinates/repository and unrelated
  datasource values stay unchanged.
- Multipart file Value may be a single path or a JSON array of paths. The
  `editing::entry_value` / `editing::edit_row` pair reads `src`, retains unchanged
  imported arrays and writes explicit edits back to the actual source field.
  Preserve other array entries and unknown row metadata; the ordinary Type field
  can change between text/file and must change picker availability accordingly.
- Binary body `Field::File` is a separate single-source operation. Do not assume
  that every REST Value, path variable or URL is a filesystem path.

## Reviewed text fields with different semantics

The complete `TextField::new` call-site scan includes editor/hex find, project
search, generic dialogs, Git dialogs, REST filter/form/entry dialogs and database
connection/session dialogs. These distinctions are required when integrating:

- File create/folder create/rename dialogs collect **one name** under an already
  selected parent. `kode-workspace/src/tree.rs::filename` rejects separators and
  multi-component paths. They are not general filesystem-path inputs; retain
  their existing tree-selected parent and filename/no-overwrite guards.
- Ordinary file open uses the Files tree and recent list; ordinary file save uses
  the document's registered path. There is currently no separate open-path or
  text-editor Save As input to attach a button to. The general widget must still
  support open-file/save-file modes for the real fields above.
- SQL Catalog/Schema, datasource name/username/password, JDBC URL, Maven
  coordinates/repository URL, credentials/properties JSON, REST URL/path/query/
  auth/environment text, names/descriptions and Git branch/tag/author/commit text
  retain their own semantics. Arbitrary values inside JSON or a JDBC URL are not
  dedicated filesystem-path fields.
- Project-search file filters are patterns rather than selected filesystem paths.
  LSP/Java paths configured through environment/catalog data are not currently
  editable TUI path fields. Do not invent new settings inputs to satisfy this card.

## Qualified coverage

1. **done** — Worker-backed file/directory modes, explicit project base and typed
   value, current-folder expansion, parent/Go navigation and new save names.
2. **done** — Seven path-field sites and three existing selector operations use
   the shared chooser. Real import/export/SQL/datasource/configuration operations
   and HTTP uploads consume the chosen paths while keeping the original guards.
3. **done** — Workspace/modal/item/type/driver/field/caret/focus and exact request
   proofs reject late results. Slow workers retain scheduler ownership; cancellation,
   focus loss, reopening and workspace departure cannot affect another chooser.
4. **done** — Outside paths, Unicode/spaces/exact filename whitespace, new targets,
   errors, non-UTF-8/separator rejection, clipboard, cancel/focus preservation and
   tiny/narrow surfaces are covered. The terminal matrix checks all ten rows at
   both widths and resizes an active SQL chooser to 1×1 before completing a save.

Local qualification is Linux. Z03 retains its full platform/architecture artifact
matrix and resumes after the remaining Uxx work, as requested.
