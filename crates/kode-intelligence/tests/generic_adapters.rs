use kode_core::{ByteOffset, DocumentId, DocumentOrigin, WorkspaceGeneration, document::Document};
use kode_intelligence::{
    frontend::{
        generic::GenericSemanticAdapter,
        legacy::{self, DefinitionRegistry},
        registry::{AdapterCapability, SemanticAdapterSession},
    },
    ids::SemanticIds,
    model::*,
    resolver::SemanticResolver,
    snapshot::SemanticSnapshot,
};
use kode_platform::{jobs::CancellationToken, paths::ResourcePaths};
use kode_syntax::{
    keywords::KeywordProvider,
    registry::{GrammarRegistry, canonical_language},
};
use std::{collections::BTreeSet, fs};
fn document(language: &str, text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "adapter-test".into(),
            key: language.into(),
            label: language.into(),
        },
        Some(language.into()),
        text,
    )
}
fn extract(language: &str, text: &str) -> kode_intelligence::frontend::registry::ExtractionReport {
    let doc = document(language, text);
    SemanticAdapterSession::new(&GrammarRegistry::compiled(), language)
        .extract(
            &DefinitionRegistry::compiled().unwrap(),
            &KeywordProvider::compiled().unwrap(),
            &format!("Fixture.{language}"),
            &doc.snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap()
}
fn query(delta: FileSemanticDelta) -> SemanticSnapshot {
    SemanticSnapshot::new(
        1,
        SemanticResolver::resolve([delta], &CancellationToken::new())
            .unwrap()
            .project,
    )
    .unwrap()
}

#[test]
fn both_original_adapter_batches_extract_real_declarations_and_queryable_byte_facts() {
    let fixtures = [
        (
            "c",
            "int add(int left, int right) { return left + right; }",
            Some("add"),
        ),
        (
            "cpp",
            "class Customer { public: int id; };",
            Some("Customer"),
        ),
        (
            "go",
            "package demo\nfunc add(left int, right int) int { return left + right }",
            Some("add"),
        ),
        (
            "javascript",
            "class Customer { save() {} }",
            Some("Customer"),
        ),
        (
            "typescript",
            "interface Customer { id: number }",
            Some("Customer"),
        ),
        ("html", "<main><h1>Hello</h1></main>", None),
        ("css", ".customer { color: red; }", None),
        ("json", "{\"customer\": {\"id\": 1}}", None),
        (
            "python",
            "class Customer:\n    def save(self):\n        return 1",
            Some("Customer"),
        ),
        (
            "php",
            "<?php class Customer { public function save(): void {} }",
            Some("Customer"),
        ),
        (
            "ruby",
            "class Customer\n  def save\n  end\nend",
            Some("Customer"),
        ),
        ("bash", "save() { echo customer; }", Some("save")),
        (
            "swift",
            "class Customer { func save() {} }",
            Some("Customer"),
        ),
    ];
    for (language, text, expected) in fixtures {
        let report = extract(language, text);
        assert_ne!(
            report.capability,
            AdapterCapability::Heuristic,
            "{language}: {:?}",
            report.notes
        );
        assert!(report.notes.is_empty(), "{language}: {:?}", report.notes);
        assert_eq!(report.delta.file.language_id, language);
        report.delta.validate_source(text).unwrap();
        if let Some(name) = expected {
            assert!(
                report.delta.symbols.iter().any(|s| s.name == name),
                "{language}: {:?}",
                report.delta.symbols
            );
        }
        let snapshot = query(report.delta);
        let file = snapshot
            .file_at_path(&format!("Fixture.{language}"))
            .unwrap();
        assert!(
            snapshot
                .project()
                .scopes
                .iter()
                .any(|s| s.file_id == file.id && s.kind == ScopeKind::File)
        );
    }
}

#[test]
fn every_registered_grammar_and_its_runtime_aliases_extract_resolve_and_query_without_lsp() {
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/languages/parsers/node-fields.json"
    ))
    .unwrap();
    let aliases = [
        ("kotlin", vec!["kt", "kts"]),
        (
            "cpp",
            vec!["c++", "cc", "cxx", "hpp", "h++", "hh", "hxx", "c+", "cplus"],
        ),
        ("go", vec!["golang"]),
        ("javascript", vec!["js"]),
        ("typescript", vec!["ts"]),
        ("tsx", vec!["jsx"]),
        ("html", vec!["htm"]),
        ("python", vec!["py"]),
        ("php", vec!["php3", "php4", "php5"]),
        ("ruby", vec!["rb"]),
        ("bash", vec!["sh", "shellscript"]),
        ("markdown", vec!["md"]),
        ("perl", vec!["pl"]),
        ("yaml", vec!["yml"]),
        ("pascal", vec!["pas", "delphi"]),
        ("ocaml", vec!["ml"]),
        ("ocaml-interface", vec!["mli"]),
        ("csharp", vec!["c#", "cs", "c-sharp"]),
    ];
    let registry = GrammarRegistry::compiled();
    let definitions = DefinitionRegistry::compiled().unwrap();
    assert!(
        definitions.diagnostics.is_empty(),
        "{:?}",
        definitions.diagnostics
    );
    let keywords = KeywordProvider::compiled().unwrap();
    let mut count = 0;
    for fixture in fixtures["bindings"].as_array().unwrap() {
        let language = fixture["grammar"]["language"].as_str().unwrap();
        let source = fixture["source"].as_str().unwrap();
        let mut names = vec![language];
        if let Some((_, list)) = aliases.iter().find(|(key, _)| *key == language) {
            names.extend(list);
        }
        for alias in names {
            let doc = document(alias, source);
            let report = SemanticAdapterSession::new(&registry, alias)
                .extract(
                    &definitions,
                    &keywords,
                    alias,
                    &doc.snapshot(),
                    WorkspaceGeneration(1),
                    &CancellationToken::new(),
                )
                .unwrap();
            assert_ne!(
                report.capability,
                AdapterCapability::Heuristic,
                "{alias}: {:?}",
                report.notes
            );
            assert!(report.notes.is_empty(), "{alias}: {:?}", report.notes);
            assert_eq!(report.delta.file.language_id, canonical_language(alias));
            report.delta.validate_source(source).unwrap();
            let snapshot = query(report.delta);
            assert!(snapshot.file_at_path(alias).is_some());
            assert!(
                snapshot
                    .project()
                    .scopes
                    .iter()
                    .any(|s| s.kind == ScopeKind::File)
            );
            count += 1;
        }
    }
    assert_eq!(
        count,
        registry.entries().count() + aliases.iter().map(|(_, list)| list.len()).sum::<usize>()
    );
    let sql = extract("sql", "CREATE TABLE customers (id INTEGER);\n");
    assert!(sql.delta.symbols.iter().any(|s| s.name == "customers"));
}

#[test]
fn typescript_synthetic_paths_keep_top_level_declarations_members_shadowing_and_unicode_ranges() {
    let source = "export interface Customer { id: number; save(): void; }\nexport type Alias = Customer;\nexport enum Mode { Ready }\nexport const café = 1;\nexport let value = café;\nexport class Holder { item: Customer; save() { const local = value; return local; } }\nexport function use(arg: Customer) { const local = café; return arg.id; }\n";
    let report = extract("typescript", source);
    let delta = &report.delta;
    for (name, kind) in [
        ("Customer", SymbolKind::Interface),
        ("Alias", SymbolKind::TypeAlias),
        ("Mode", SymbolKind::Enum),
        ("café", SymbolKind::Constant),
        ("value", SymbolKind::Variable),
        ("Holder", SymbolKind::Class),
        ("use", SymbolKind::Function),
        ("arg", SymbolKind::Parameter),
    ] {
        assert!(
            delta
                .symbols
                .iter()
                .any(|s| s.name == name && s.kind == kind),
            "missing {name}/{kind:?}: {:?}",
            delta.symbols
        );
    }
    assert!(
        !delta
            .symbols
            .iter()
            .any(|s| matches!(s.name.as_str(), "export" | "const" | "return" | "interface"))
    );
    for symbol in &delta.symbols {
        assert_eq!(
            &source[symbol.name_range.start.0..symbol.name_range.end.0],
            symbol.name
        );
    }
    for occurrence in &delta.occurrences {
        assert_eq!(
            &source[occurrence.range.start.0..occurrence.range.end.0],
            occurrence.text
        );
    }
    let snapshot = query(report.delta);
    let locals: Vec<_> = snapshot
        .project()
        .symbols
        .iter()
        .filter(|s| s.name == "local")
        .collect();
    assert_eq!(locals.len(), 2);
    assert_ne!(locals[0].scope_id, locals[1].scope_id);
    assert_ne!(locals[0].id, locals[1].id);
    let café = snapshot
        .project()
        .symbols
        .iter()
        .find(|s| s.name == "café")
        .unwrap();
    assert!(snapshot.references(café.id).count() >= 2);
}

#[test]
fn unsupported_or_missing_grammar_uses_low_confidence_definitions_and_visible_status() {
    let definitions = DefinitionRegistry::compiled().unwrap();
    let keywords = KeywordProvider::compiled().unwrap();
    let source = "pub struct Customer { id: u64 }\nfn save(value: Customer) { let count = 1; }\n";
    let report = extract("rust", source);
    assert_eq!(report.capability, AdapterCapability::Heuristic);
    assert!(!report.notes.is_empty());
    assert!(report.delta.symbols.iter().any(|s| s.name == "Customer"));
    assert!(
        report.delta.symbols.iter().all(
            |s| s.confidence.source == SemanticSource::Heuristic && s.confidence.value() == 0.5
        )
    );
    query(report.delta);
    let source = "class MissingNative { fun save() {} }\n";
    let doc = document("kotlin", source);
    let report = SemanticAdapterSession::new(&GrammarRegistry::empty(), "kt")
        .extract(
            &definitions,
            &keywords,
            "missing.kt",
            &doc.snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(report.capability, AdapterCapability::Heuristic);
    assert!(!report.notes.is_empty());
    assert!(
        report
            .delta
            .symbols
            .iter()
            .any(|s| s.name == "MissingNative")
    );
    query(report.delta);
}

#[test]
fn configured_patterns_override_defaults_use_numbered_captures_and_preserve_masked_unicode_bytes() {
    let definitions = DefinitionRegistry::from_json(r##"{"languages":[{"language":"dsl","aliases":["custom"],"lineComments":["#"],"patterns":[{"kind":"CLASS","regex":"(record)\\s+([\\p{L}_][\\p{L}\\p{N}_]*)","groupIndex":2},{"kind":"METHOD","regex":"(?<=fn )([A-Za-z_]+)"}]}]}"##).unwrap();
    let keywords = KeywordProvider::compiled().unwrap();
    let source = "# record Commenté\r\nrecord Café\r\nclass Ignored\r\n\"record Stringé\"\r\n/* record Blocké */ fn save\r\nrecord Next\r\n";
    let doc = document("custom", source);
    let delta = legacy::extract(
        &definitions,
        &keywords,
        "Fixture.dsl",
        "custom",
        &doc.snapshot(),
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        delta
            .symbols
            .iter()
            .map(|s| s.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["Café", "Next", "save"])
    );
    assert_eq!(
        delta
            .symbols
            .iter()
            .find(|s| s.name == "save")
            .unwrap()
            .kind,
        SymbolKind::Method
    );
    delta.validate_source(source).unwrap();
    assert!(
        !delta
            .occurrences
            .iter()
            .any(|o| matches!(o.text.as_str(), "Commenté" | "Stringé" | "Blocké"))
    );
    assert_eq!(delta.lexical_tokens.len(), 3);
    query(delta);
    let invalid = DefinitionRegistry::from_json(r#"{"languages":[{"language":"dsl","patterns":[{"kind":"CLASS","regex":"("},{"kind":"CLASS","regex":"(a)","groupIndex":2}]}]}"#).unwrap();
    assert_eq!(invalid.diagnostics.len(), 2);
    assert!(DefinitionRegistry::from_json("{}").is_err());
}

#[test]
fn external_registry_precedence_and_invalid_catalog_fallback_use_synthetic_resources() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let write = |name: &str, language: &str| {
        let path = root.join(name).join("codeintel/definitions.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(r#"{{"languages":[{{"language":"{language}"}}]}}"#),
        )
        .unwrap();
        path
    };
    let explicit = write("explicit", "explicit");
    write("rust", "rustoverride");
    write("legacy", "legacyoverride");
    write("executable", "exeoverride");
    let mut paths = ResourcePaths {
        explicit: Some(root.join("explicit")),
        rust_home: Some(root.join("rust")),
        legacy_home: Some(root.join("legacy")),
        executable_dir: root.join("executable"),
        config_dir: root.join("config"),
    };
    let loaded = DefinitionRegistry::load(&paths, None).unwrap();
    assert_eq!(loaded.origin.as_ref(), Some(&explicit));
    assert_eq!(loaded.languages().collect::<Vec<_>>(), vec!["explicit"]);
    paths.explicit = None;
    assert_eq!(
        DefinitionRegistry::load(&paths, None)
            .unwrap()
            .languages()
            .collect::<Vec<_>>(),
        vec!["rustoverride"]
    );
    let override_file = write("override", "fileoverride");
    assert_eq!(
        DefinitionRegistry::load(&paths, Some(&override_file))
            .unwrap()
            .languages()
            .collect::<Vec<_>>(),
        vec!["fileoverride"]
    );
    fs::write(&override_file, b"not a catalog").unwrap();
    let fallback = DefinitionRegistry::load(&paths, Some(&override_file)).unwrap();
    assert!(fallback.origin.is_none());
    assert!(!fallback.diagnostics.is_empty());
    assert!(fallback.languages().any(|l| l == "kotlin"));
    paths.rust_home = None;
    assert_eq!(
        DefinitionRegistry::load(&paths, None)
            .unwrap()
            .languages()
            .collect::<Vec<_>>(),
        vec!["legacyoverride"]
    );
}

#[test]
fn generic_fresh_and_reused_parses_match_and_cancelled_or_invalid_depth_work_is_rejected() {
    let registry = GrammarRegistry::compiled();
    let mut adapter = GenericSemanticAdapter::new(&registry, "typescript").unwrap();
    let doc = document("typescript", "export class Café { save() { return 1; } }\n");
    let snapshot = doc.snapshot();
    let first = adapter
        .extract(
            "Fixture.ts",
            &snapshot,
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap();
    let reused = adapter
        .extract(
            "Fixture.ts",
            &snapshot,
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(first, reused);
    let fresh = GenericSemanticAdapter::new(&registry, "typescript")
        .unwrap()
        .extract(
            "Fixture.ts",
            &snapshot,
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(fresh, reused);
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        adapter.extract("Fixture.ts", &snapshot, WorkspaceGeneration(1), &token),
        Err(kode_core::DomainError::Cancelled)
    ));
    let definitions = DefinitionRegistry::compiled().unwrap();
    let keywords = KeywordProvider::compiled().unwrap();
    assert!(matches!(
        legacy::extract(
            &definitions,
            &keywords,
            "Fixture.ts",
            "typescript",
            &snapshot,
            &token
        ),
        Err(kode_core::DomainError::Cancelled)
    ));
    let text = format!("let value = {}0{};", "(".repeat(400), ")".repeat(400));
    let deep = document("typescript", &text);
    assert!(
        adapter
            .extract(
                "Fixture.ts",
                &deep.snapshot(),
                WorkspaceGeneration(1),
                &CancellationToken::new()
            )
            .is_err()
    );
    let empty = extract("unknown", "");
    assert_eq!(empty.delta.scopes[0].range.start, ByteOffset(0));
    assert_eq!(empty.delta.scopes[0].range.end, ByteOffset(0));
    assert_eq!(empty.delta.file.id, SemanticIds::file("Fixture.unknown"));
    query(empty.delta);
}
