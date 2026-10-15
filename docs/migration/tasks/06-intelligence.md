# Syntax and code intelligence tasks

Tree-sitter parses syntax; it does not replace Kode's semantic resolver. LSP is
optional enrichment, so native completion/navigation must remain functional with
every external language server disabled. Read the original semantic tests as
executable examples; preserve their distinctions between declaration identity,
scope, lexical classification and resolved types.

## L01 — Semantic facts, identifiers and immutable snapshots

**Status:** done

**Depends:** F02, E01. **Owns:** `kode-intelligence/src/{model,snapshot,ids}.rs`.
**Read:** `codeintel/model/SemanticModel.kt`, `index/SemanticIndex.kt`,
`EditorIntelligence.kt`; KotlinSemanticVerticalSliceTest snapshot cases.

1. Port typed file/symbol/scope/type IDs and SourceRange with explicit Rust byte
   coordinates. Preserve fields/enums for symbols, occurrences, lexical tokens,
   imports, relations, dependencies, type hints and file deltas.
2. Define TypeRef variants (named/generic/nullable/union/unknown etc. as actually
   present), confidence/knowledge levels and deterministic IDs/public surface hashes.
3. Implement immutable query snapshots with symbol/member/visible-symbol lookup
   interfaces, leaving extraction and resolution to following tasks.

**Accept:** deterministic identity and serialization fixtures, duplicate identity
   handling, valid ranges and old snapshot stability after a new publication.
   Do not keep Kotlin UTF-16 hash offsets while labeling them Rust bytes.

## L02 — Keyword highlighting and incremental parser registry

**Status:** done

**Depends:** E01, F03. **Owns:** `kode-syntax/src/`, the F02
`resources/{keyword-patterns,token-colors}.txt` catalogs, and
`resources/grammars/manifest.json`.
**Read:** `grammars/*.kt`, `lang/{base,TreeSitterSupport}.kt`, keyword-patterns.txt,
token-colors.txt, KeywordSyntaxProviderTest and RegexSyntaxProviderTest.

1. Port keyword/color resources, markup tag/attribute/string and CSS class special
   cases, extension/language aliases and keyword extraction for completion. Validate
   all regexes with explicit incompatibility reports.
2. Select/pin compatible Tree-sitter runtime and bindings, starting Kotlin/C, then
   the active language set. Record grammar node/field/ABI fixtures before adapters
   rely on them. Distinguish bundled dependencies from actively used grammars.
3. Implement parser/tree reuse with InputEdit and byte coordinates, lexical span
   extraction and parse errors. Provide cancellation and bounded background work.

**Accept:** existing grammar tests, source punctuation preserved, C+ and systemd
   mappings, incremental edit versus fresh parse equivalence and Unicode offsets.
   Grammar load failure produces a usable regex fallback with visible status.

## L03 — Kotlin semantic extraction

**Status:** done

**Depends:** L01, L02. **Owns:** `kode-intelligence/src/frontend/kotlin/`.
**Read:** `frontend/KotlinSemanticAdapter.kt`, `lang/adapters/KotlinTsAdapter.kt`,
`lang/kotlin/KotlinLanguageAdapter.kt`, KotlinIdentifierPipelineTest and Kotlin tests.

1. Port declarations, nested scopes, imports, occurrences, member/inheritance
   relations and lexical tokens using the selected grammar's real AST.
2. Extract declared/nullable/generic types, expression ranges and inference hints
   for references, assignments, calls, casts, properties, `this` and `super`.
3. Compute exported semantic surface independently of local implementation edits;
   publish one FileSemanticDelta with no cross-file resolution inside the parser.

**Accept:** extraction fixtures for incomplete code, Unicode identifiers, shadowing,
   declaration order, imports, visibility, nullable/generic types and nested scopes.
   Reused tree and fresh parse yield equivalent facts; no full compiler guarantees.

## L04 — C semantic extraction

**Status:** done

**Depends:** L01, L02. **Owns:** `kode-intelligence/src/frontend/c/`.
**Read:** `frontend/CSemanticAdapter.kt`, CSemanticTest and C semantic token tests.

1. Port tag/typedef/anonymous-struct namespaces, fields, variable declarations,
   pointer types and lexical occurrence spans.
2. Emit field/member relations and expression hints for `.` and `->`; preserve
   alias-to-tag identity and same-spelling typedef/tag behavior.
3. Integrate with normalized deltas and declarations from other files. Keep current
   best-effort behavior; do not add a fake preprocessor/compiler validation layer.

**Accept:** original C cases for cross-file aliases/pointers/anonymous structs,
   reload, moved/changed fields and same-spelling tag aliases. Tokens never include
   adjacent punctuation and unknown receivers do not resolve arbitrary fields.

## L05 — SQLite semantic store and atomic publication

**Status:** done

**Depends:** L01, F03. **Owns:** `kode-intelligence/src/store/`, SQL schema migrations.
**Read:** `index/H2SemanticStore.kt`, `DbCodeIntelStore.kt`, `DbServerManager.kt`;
H2SemanticStoreTest and DbServerManagerTest.

1. Translate both legacy compatibility facts and normalized semantic facts needed
   by current consumers into SQLite tables/indexes. Use explicit schema versioning,
   stable IDs, dependency/generation persistence and transactional replacements.
2. Keep SQLite in a dedicated worker; readers use immutable memory snapshots.
   Publish completed snapshots atomically and deduplicate records consistently.
3. Implement reload/rebuild/clear/in-memory test modes. Old H2 files are cache
   inputs only for reference; rebuild native index from sources without Java.

**Accept:** reload of facts/types/dependencies, duplicate stable symbols/expression
   ranges, reader holds old snapshot during publication, revision reuse, interrupted
   write and incompatible cache rebuild. No network listener or H2 process needed.

## L06 — Names, scopes, imports and symbol resolution

**Status:** done

**Depends:** L03, L04. **Owns:** `kode-intelligence/src/resolver/{mod,names,scopes,imports,relations,declared_types}.rs`.
**Read:** `resolver/SemanticResolver.kt`, SemanticIndex visibleSymbols/isAccessible;
KotlinSemanticVerticalSliceTest and CSemanticTest.

1. Resolve local names with nested scope/declaration order and stable symbol
   identity; separate definitions from occurrences.
2. Resolve cross-file imports/types and inheritance relations independently of
   file indexing order. Preserve visibility and language-specific namespaces.
3. Track unresolved references and dependencies for later invalidation. Expose
   deterministic resolved project data usable by storage and type inference.

**Accept:** same-name symbols in separate scopes/files, forward/local declarations,
   inherited visibility, unresolved names and order-independent cross-file results.
   Definitions/references follow identity, not a global string search.

## L07 — Expression types and receiver member lookup

**Status:** done

**Depends:** L06. **Owns:** `kode-intelligence/src/resolver/{expressions,types,members}.rs`.
**Read:** `ExpressionTypeResolver.kt`, remaining SemanticResolver type-inference
sections; KotlinExpressionSemanticsTest and member-related vertical-slice tests.

1. Implement literals, references, assignments, call results, properties, casts and
   parenthesized receivers. Progress until stable using explicit iteration bounds.
2. Preserve generic and nullable shape, merge conflicting assignments to unions,
   and resolve `this`/`super`, inherited members, visibility and chained access.
3. Support C pointer/tag/alias member lookup via the shared model. Unknown types
   stay unknown; do not invent receiver types from workspace text frequency.

**Accept:** every named KotlinExpressionSemanticsTest scenario, C pointer/member
   fixtures and persisted type reconstruction. Cycles/incomplete programs terminate
   with useful partial facts, not hangs or arbitrary last-assignment types.

## L08 — Dependency invalidation and incremental generations

**Status:** done

**Depends:** L07. **Owns:** `kode-intelligence/src/index/{dependencies,invalidation}.rs`.
**Read:** `DependencyGraph.kt`, `SemanticInvalidationPlanner.kt`, SemanticIndex;
SemanticInvalidationTest and implementation-only persistence cases.

1. Distinguish content change from exported semantic surface change; re-resolve
   edited file only when dependents' facts need no update.
2. Invalidate transitive dependents on changed/removed exports, wake previously
   unresolved files when a matching definition appears, terminate dependency cycles.
3. Track resolution generations and cache keys separately from document versions;
   propagate affected-file notifications to token/completion consumers.

**Accept:** all invalidation fixtures, rename/remove/new definition, cycle closure,
   unchanged implementation dependency relation retention and correct reload behavior.

## L09 — Background index coordinator and editor facade

**Status:** done

**Depends:** L05, L08, L10, L11, W01, F03. **Owns:**
`kode-intelligence/src/{service,facade,tokens}.rs` and UI service wiring.
**Read:** `CodeIntelService.kt`, `EditorIntelligence.kt`, `semantic/SemanticTokens.kt`,
`BlockTracker.kt`, Main's indexing/rescan coordination.

1. Compose scan→parse/extract→resolve→persist→publish pipeline with debounced live
   edits, cancellation, workspace/document version gating and progress phases.
2. Implement definitions/references/completions/tokens facade; use current unsaved
   documents where needed. Include language, declaration ranges and missing-source
   metadata for editor previews.
3. Expose stable snapshot revision/status and explicit rescan/clear/rebuild actions;
   invalidate editor overlays after dependent-file changes. Keep legacy fallback
   available when an adapter cannot provide facts.

**Accept:** CodeIntelServiceTest, semantic token/revision fixtures, edit races and
   workspace switch. Queries do not block UI on publication. UX-045/047/085/110.

## L10 — Other language adapters and heuristic fallback

**Status:** done

**Depends:** L01, L02, L06. **Owns:** `kode-intelligence/src/frontend/{generic,legacy}/`,
`resources/codeintel/definitions.json`, adapter registry.
**Read:** TreeSitterSemanticAdapter(s), LegacySemanticDeltaFactory, lang TypeScript
adapters, `codeintel/definitions.json`; TreeSitterSemanticAdapterTest.

1. Port active generic adapter configurations for C++, Go, JavaScript, TypeScript/
   JSX/TSX, HTML, CSS, JSON, Python, PHP, Ruby, Bash and Swift, including aliases.
2. Port external definition registry precedence, regex extraction, declaration/
   occurrence heuristics and language fallback. Do not advertise Kotlin-level
   semantics for a generic extractor.
3. Inventory remaining Gradle grammar dependencies (SQL, Zig, Markdown, Svelte,
   Perl, D, YAML, Pascal, OCaml, C# etc.) against actual runtime registrations;
   preserve active behavior and record unused dependencies without mandatory ports.

**Accept:** both original adapter batches, keyword exclusion, external definitions
   overriding defaults, TypeScript path fixtures and a minimal parse/extract/query
   fixture for every active language/alias. Unsupported grammar gives fallback.

## L11 — Completion providers and deterministic ranking

**Status:** done

**Depends:** L07, L02. **Owns:** `kode-intelligence/src/completion/`.
**Read:** `completion/CompletionPipeline.kt`, `ml/CompletionRanker.kt`,
BlockTracker and completion-related Kotlin/CodeIntelService tests.

1. Port local/member/type/import/keyword/workspace/snippet providers and context
   checks; preserve symbol identity, insertion text/details and accessible members.
2. Port heuristic scoring (prefix, scope distance, receiver/type, same-file recency,
   workspace frequency), deduplication and deterministic tie-breaks.
3. Return explicit replacement context to E09. Preserve the optional ranking
   interface without introducing ONNX/ML inference: source ML provider is a hook,
   while heuristic ranking is real implemented behavior.

**Accept:** precise receiver results, scope precedence, import/keyword candidates,
   stable ranking under input-order changes, empty prefix, nullable chains and
   unknown receivers. E09 can display/apply results without inspecting ASTs.

## L12 — Language and persistence parity qualification

**Status:** done

**Depends:** L09, L10. **Owns:** `kode-intelligence/tests/parity/`,
`docs/parity/languages.md` and grammar capability matrix.

1. Translate original meaningful Kotlin/C/adapter/store/invalidation fixtures into
   small Rust suites; avoid tests tied to a developer's absolute corpus path.
2. Record each language's MIME mapping, grammar version, lexical fallback,
   semantic capabilities and tested aliases. Compare live edits, fresh index and
   reload results for equivalent answers.
3. Check disabled helpers/proposals: SDK indexing is explicitly disabled in
   Main.indexSdkSources; SCIP/compiler enrichers and learned rankers are hooks.
   Keep these listed as future extensions, without claiming them as migrated features.

**Accept:** complete active-language matrix, native operation with all LSP servers
   disabled, stable restart behavior, no regression in original semantic suites.
