# Native language and persistence qualification

Task: L12. Status: done.
Source baseline: `7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68`.

The [complete machine-readable matrix](language-matrix.json) inventories **286
registrations**: 257 text/explicit language entries and 29 viewer-only entries.
It covers all **281 shipped MIME rows**, **270 effective extension mappings**,
**248 keyword patterns**, **24 heuristic configurations**, and **28 native grammar
variants**. Binary/image mappings are explicitly excluded from semantic indexing.
`python3 scripts/language-matrix.py` regenerates the inventory; `--check` verifies it.
The runtime qualification suite checks the matrix against actual APIs, not only JSON.

Tree-sitter runtime is **0.26.13**. Package versions, ABI, node-types SHA-256,
repositories and licenses appear in every native matrix row and in
[the pinned manifest](../../resources/grammars/manifest.json). All native rows
passed reused-parser live edit → incremental resolution → fresh resolution →
SQLite restart comparisons, with old immutable readers retained. **33 additional
runtime aliases** passed actual extraction, giving **61 language/alias cases**.

| Language | Grammar package version | Semantic extraction | Tested runtime aliases |
|---|---|---|---|
| bash | bash 0.25.1 | common AST | sh, shellscript |
| c | c 0.24.2 | specialized C | — |
| cpp | cpp 0.23.4 | common AST | c++, cc, cxx, hpp, h++, hh, hxx, c+, cplus |
| csharp | c-sharp 0.23.5 | common AST + regex | c#, cs, c-sharp |
| css | css 0.25.0 | common AST | — |
| d | d 0.8.2 | common AST + regex | — |
| go | go 0.25.0 | common AST | golang |
| html | html 0.23.2 | common AST | htm |
| javascript | javascript 0.25.0 | common AST | js |
| json | json 0.24.8 | common AST | — |
| kotlin | kotlin-sg 0.4.1 | specialized Kotlin | kt, kts |
| lua | lua 0.5.0 | common AST + regex | — |
| markdown | md 0.5.3 | common AST + regex | md |
| markdown-inline | md 0.5.3 | common AST + regex | — |
| ocaml | ocaml 0.26.0 | common AST + regex | ml |
| ocaml-interface | ocaml 0.26.0 | common AST + regex | mli |
| pascal | pascal 0.10.2 | common AST + regex | pas, delphi |
| perl | perl 1.1.2 | common AST + regex | pl |
| php | php 0.25.1 | common AST | php3, php4, php5 |
| python | python 0.25.0 | common AST | py |
| ruby | ruby 0.23.1 | common AST | rb |
| sql | sequel 0.3.11 | common AST + regex | — |
| svelte | svelte-ng 1.0.2 | common AST + regex | — |
| swift | swift 0.7.4 | common AST | — |
| tsx | typescript 0.23.2 | common AST | jsx |
| typescript | typescript 0.23.2 | common AST | ts |
| yaml | yaml 0.7.2 | common AST + regex | yml |
| zig | zig 1.1.2 | common AST + regex | — |

Common AST extraction recognizes the grammar's matching declaration/identifier/
scope vocabulary. It supplies validated raw facts, identity queries and conservative
completion candidates. It is not a compiler for these languages. Markup/data and
secondary grammars can legitimately yield only file scopes, lexical tokens and
partial occurrences; the tests do not manufacture declarations to fill the table.
Configured regex extraction supplements secondary grammars as in the source.
Without a registered grammar, keyword/plain lexical handling and configured/default
regex definitions remain available with explicit fallback status. **Rust and Java
currently use that fallback**; there is no bundled native Rust/Java parser claim.

Every matrix MIME row includes its declared mapping and actual last-suffix routing.
Later duplicate suffixes win, preserving the source table. In particular `.html`
routes to `vue-html` with markup keywords/heuristic semantics; the separate `html`
native registration is qualified for explicitly selected HTML. The inline Markdown
and OCaml interface variants likewise require explicit language selection. Runtime
aliases do not silently invent MIME rows for every alias. Compound suffix entries
record their actual last-suffix result. V01 independently qualifies filename,
signature, shebang, image and binary routing.

Kotlin/C retain the specialized behavior documented in [L03](L03.md), [L04](L04.md),
[L06](L06.md), [L07](L07.md), [L08](L08.md) and [L11](L11.md): scope/order/import/
visibility/identity, type and alias/inheritance links, receiver expressions,
nullable/generic/union shapes, conservative unknowns, deterministic completion,
dependency invalidation and unchanged dependent refresh. Compiler preprocessing,
complete overload selection, generic substitution and control-flow analysis are
not promised. Invalid/incomplete source remains explicitly partial.

The qualification suite (`kode-intelligence/tests/parity/`) compares complete
symbol/scope/occurrence/relation/type/expression/dependency facts and public lookup,
reference/member/visibility/token/completion answers. It verifies changed content
hashes and expected renamed declarations, persisted raw deltas and generations,
and unchanged old reader answers. Raw fact/reference sets are canonicalized for
comparison because SQLite bundle order is unspecified; ordered completion and
visibility results are compared without reordering. It never opens an LSP client,
JVM, external corpus, legacy index or network connection.

Original regression ownership is covered by these Rust suites:

| Original meaningful suite/scenarios | Rust evidence |
|---|---|
| KotlinSemanticVerticalSlice: cross-file/order, identity, inheritance, scopes, stable readers/hash | kotlin_extraction, name_resolution, expression_types, completion, model_snapshot |
| KotlinExpressionSemantics: chains/casts/assignments/unions/literals/unknown/nullable/visibility | expression_types, completion |
| CSemantic: moved fields, tags/typedefs/pointers, anonymous/same-spelling aliases and reload | c_extraction, name_resolution, invalidation |
| TreeSitterSemanticAdapter: both original language batches | generic_adapters, parity |
| H2SemanticStore: stable snapshots, duplicate facts/types, cross-file reload, generations and body-only updates | sqlite_store, invalidation, expression_types |
| SemanticInvalidation: body/surface/transitive/new/removed/cycle cases | invalidation |
| SemanticHighlighting: mixed lexical/kinds, soft-keyword property, Unicode/multiline, reused parser, dependency-only change | parity, syntax/parser, service, TUI overlays/app_runtime |
| CodeIntelService: queries, fallback/catalog, lifecycle | service, generic_adapters, completion, TUI app_runtime |
| Keyword/RegexSyntaxProvider: resources, markup/CSS, punctuation/ranges | syntax/keywords, syntax/regex_grammar |
| Kotlin/TypeScript corpus occurrence/path scans | checked-in synthetic parser node/field fixtures, kotlin_extraction, generic_adapters, parity |

Corpus scans in the Kotlin suite chiefly assert nonempty output against a developer
checkout/corpus. Rust uses reproducible synthetic UTF-8 fixtures and checks actual
scope/identifier/range/field facts. It does not depend on that checkout at runtime.
The semantic test harness is test infrastructure, with equivalent Rust builders;
it does not constitute an unported production service. H2's source cache lifecycle
is replaced by native SQLite, including real disk restart/failure/transaction tests.

Qualification fixed two gaps: shell keyword aliases now reach the shipped
`shellscript` pattern, and Kotlin soft-keyword identifier wrappers no longer emit
an extra keyword token for a property named `field`. CRLF comment ranges retain the
actual parser bytes, including CR before LF; no UTF-16/UTF-8 reinterpretation occurs.

Source proposals remain explicit:

- `Main.indexSdkSources()` is disabled in the frozen source. No automatic JDK,
  Kotlin stdlib or SDK ZIP indexing is claimed or fetched.
- Compiler/SCIP/machine-learning enrichment types are interfaces in
  `semantic/SemanticEnricher.kt`, not implementations to recreate as fake services.
- The implemented heuristic ranker is migrated, including deterministic weights
  and ties. Optional learned/ONNX rankers remain score hooks; no model is bundled.
- Optional external LSP installation/lifecycle/enrichment belongs to P01–P03 and
  remains pending. All qualification here runs with all LSP servers absent.

Validation on stable Rust/Cargo 1.99.0:

- `cargo test --offline --workspace`: **314 passed**, zero failures
  (`/tmp/kode-l12-workspace-tests.log`). This includes all three new qualification
  tests and all original scenario translations above.
- `cargo fmt --all --check`, `git diff --check`,
  `cargo clippy --offline --workspace --all-targets -- -D warnings`: passed.
- `cargo build --offline -p kode`: passed.
- `python3 scripts/language-matrix.py --check`: passed; runtime tests validate
  native metadata/aliases, keyword support and all 270 actual MIME routes.
- `python3 scripts/test-highlights-terminal.py`: 2 real CLI/PTY cases passed.
- `python3 scripts/test-intelligence-terminal.py`: 2 real CLI/PTY cases passed.
  Both use synthetic projects, capture actual native output and verify exact save,
  clean shutdown and restored terminal modes; no LSP server or Java is started.

This qualifies native intelligence; full IDE migration and release qualification
remain in progress.
