//! Native qualification only: these fixtures construct no LSP client, helper,
//! external process, developer corpus or legacy H2 database.
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, JobTag, RequestId, TextRange, WorkspaceGeneration,
    commands::EditKind, document::Document,
};
use kode_intelligence::{
    completion::{CompletionContext, CompletionEngine},
    facade::IntelligenceResources,
    frontend::registry::{AdapterCapability, SemanticAdapterSession},
    ids::SemanticIds,
    index::IncrementalSemanticIndex,
    model::{FileSemanticDelta, ResolvedSemanticProject},
    resolver::SemanticResolver,
    snapshot::SemanticSnapshot,
    store::{StoreLocation, StoreWorker},
    tokens::SemanticTokenService,
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{keywords::KeywordProvider, registry::canonical_language};
use kode_workspace::mime::MimeDetector;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
fn fixtures() -> Value {
    serde_json::from_str(include_str!(
        "../../../../fixtures/languages/parsers/node-fields.json"
    ))
    .unwrap()
}
fn matrix() -> Value {
    serde_json::from_str(include_str!("../../../../docs/parity/language-matrix.json")).unwrap()
}
fn tag() -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: None,
    }
}
fn assert_answers(actual: Value, expected: &Value, context: &str) {
    for (key, value) in actual.as_object().unwrap() {
        assert_eq!(value, &expected[key], "{context}: {key}");
    }
}
fn extract(
    session: &mut SemanticAdapterSession,
    resources: &IntelligenceResources,
    path: &str,
    document: &Document,
) -> FileSemanticDelta {
    let report = session
        .extract(
            &resources.definitions,
            &resources.keywords,
            path,
            &document.snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_ne!(
        report.capability,
        AdapterCapability::Heuristic,
        "{}: {:?}",
        session.language(),
        report.notes
    );
    assert!(
        report.notes.is_empty(),
        "{}: {:?}",
        session.language(),
        report.notes
    );
    report
        .delta
        .validate_source(&document.snapshot().text())
        .unwrap();
    report.delta
}
fn document(language: &str, text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "parity".into(),
            key: language.into(),
            label: language.into(),
        },
        Some(language.into()),
        text,
    )
}
/// Compare meaningful facts AND their immutable query indexes. Revisions and
/// generation counters intentionally differ between incremental and fresh work.
fn answers(
    snapshot: &SemanticSnapshot,
    path: &str,
    text: &str,
    probe: usize,
    keywords: Arc<KeywordProvider>,
) -> Value {
    let file = snapshot.file_at_path(path).unwrap();
    let project = snapshot.project();
    let tokens = SemanticTokenService::tokens(file.id, snapshot, None);
    let context =
        CompletionContext::at(file.id, &file.language_id, text, ByteOffset(probe)).unwrap();
    let completions = CompletionEngine::native(keywords)
        .complete(&context, snapshot, 1000, &CancellationToken::new())
        .unwrap();
    let locations: Vec<_> = snapshot
        .occurrences(file.id)
        .map(|o| {
            o.range.validate(text).unwrap();
            (
                o.range,
                snapshot.occurrence_at(file.id, o.range.start).map(|o| o.id),
                o.resolved_symbol_id,
                snapshot.symbol_at(file.id, o.range.start).map(|s| s.id),
            )
        })
        .collect();
    let references: Vec<_> = snapshot
        .symbols(file.id)
        .map(|symbol| {
            let mut references: Vec<_> = snapshot
                .references(symbol.id)
                .map(|o| (o.file_id, o.range.start, o.range.end, o.kind))
                .collect();
            references.sort();
            (symbol.id, references)
        })
        .collect();
    let members: Vec<_> = project
        .types
        .values()
        .map(|ty| {
            (
                ty.id,
                snapshot
                    .members(&ty.r#ref)
                    .iter()
                    .map(|s| s.id)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    // Raw fact/reference iteration order is unspecified; SQLite bundles sort by
    // identity. Match sets without weakening ordered completion/visible results.
    let mut occurrences = project.occurrences.clone();
    occurrences.sort_by_key(|o| o.id);
    let mut relations = project.relations.clone();
    relations.sort_by_key(|r| (r.from, r.to, r.kind));
    json!({"symbols":project.symbols,"scopes":project.scopes,"occurrences":occurrences,"relations":relations,"types":project.types,"expressions":project.expression_types,"dependencies":project.dependencies,
        "locations":locations,"references":references,"members":members,
        "visible":snapshot.visible_symbols(file.id,ByteOffset(probe)).iter().map(|s|s.id).collect::<Vec<_>>(),
        "tokens":tokens.iter().map(|t|(&t.range,&t.kind,&t.occurrence)).collect::<Vec<_>>(),
        "completion":completions.iter().map(|c|c.item(&context)).collect::<Vec<_>>()})
}

#[tokio::test]
async fn all_28_native_variants_live_edits_fresh_resolution_and_sqlite_restart_have_equivalent_answers()
 {
    let resources = IntelligenceResources::compiled().unwrap();
    let fixtures = fixtures();
    let edits: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/languages/parity-edits.json"
    ))
    .unwrap();
    let mut qualified = BTreeSet::new();
    for edit in edits.as_array().unwrap() {
        let language = edit["language"].as_str().unwrap();
        let fixture = fixtures["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["grammar"]["language"] == language)
            .unwrap();
        let before = fixture["source"].as_str().unwrap();
        let old = edit["from"].as_str().unwrap();
        let new = edit["to"].as_str().unwrap();
        let after = before.replace(old, new);
        assert_ne!(before, after, "{language}");
        let path = format!("Fixture.{language}");
        let file = SemanticIds::file(&path);
        let mut document = document(language, before);
        let mut session = SemanticAdapterSession::new(&resources.grammars, language);
        let original = extract(&mut session, &resources, &path, &document);
        let first = SemanticSnapshot::new(
            1,
            IncrementalSemanticIndex::prepare(
                &ResolvedSemanticProject::default(),
                [original.clone()],
                [],
                &CancellationToken::new(),
            )
            .unwrap()
            .report
            .project,
        )
        .unwrap();
        let held = first.clone();
        let previous_answers = answers(
            &held,
            &path,
            before,
            before.find(old).unwrap() + old.len(),
            resources.keywords.clone(),
        );
        let directory = tempfile::tempdir().unwrap();
        let worker = StoreWorker::start(StoreLocation::Project(directory.path().into()))
            .await
            .unwrap();
        let client = worker.client();
        client
            .commit(tag(), first, None, CancellationToken::new())
            .unwrap()
            .resolve()
            .await
            .unwrap();
        // This replacement crosses line boundaries for HTML's two tag names and
        // C's declaration+use, exercising a reused parser rather than a new tree.
        document
            .apply_edit(
                document.revision(),
                TextRange {
                    start: ByteOffset(0),
                    end: ByteOffset(before.len()),
                },
                &after,
                EditKind::Typing,
            )
            .unwrap();
        let current = extract(&mut session, &resources, &path, &document);
        let fresh_delta = extract(
            &mut SemanticAdapterSession::new(&resources.grammars, language),
            &resources,
            &path,
            &document,
        );
        assert_eq!(current, fresh_delta, "reused extraction: {language}");
        assert_ne!(
            original.file.content_hash, current.file.content_hash,
            "{language}"
        );
        let prepared = IncrementalSemanticIndex::prepare(
            held.project(),
            [current.clone()],
            [],
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(
            prepared.invalidation.affected_files.contains(&file),
            "{language}"
        );
        let live = SemanticSnapshot::new(2, prepared.report.project).unwrap();
        let fresh = SemanticSnapshot::new(
            2,
            SemanticResolver::resolve([fresh_delta], &CancellationToken::new())
                .unwrap()
                .project,
        )
        .unwrap();
        let probe = after.find(new).unwrap() + new.len();
        let expected = answers(&fresh, &path, &after, probe, resources.keywords.clone());
        assert_eq!(
            answers(&live, &path, &after, probe, resources.keywords.clone()),
            expected,
            "live vs fresh: {language}"
        );
        if let Some(name) = edit["symbol"].as_str() {
            assert!(
                live.symbols(file).any(|s| s.name == name),
                "missing {name}: {language}"
            );
        }
        let receipt = client
            .commit(tag(), live.clone(), None, CancellationToken::new())
            .unwrap()
            .resolve()
            .await
            .unwrap()
            .value;
        assert_eq!(receipt.changed_files, vec![file], "{language}");
        assert_eq!(
            answers(
                &held,
                &path,
                before,
                before.find(old).unwrap() + old.len(),
                resources.keywords.clone()
            ),
            previous_answers,
            "old immutable reader: {language}"
        );
        let generation = live.resolution_generation(file);
        worker.shutdown().await.unwrap();
        let restarted = StoreWorker::start(StoreLocation::Project(directory.path().into()))
            .await
            .unwrap();
        let restored = restarted
            .client()
            .load(tag(), CancellationToken::new())
            .unwrap()
            .resolve()
            .await
            .unwrap()
            .value
            .snapshot;
        assert_eq!(restored.version(), 2);
        assert_eq!(
            restored.resolution_generation(file),
            generation,
            "{language}"
        );
        assert_eq!(
            restored.project().deltas,
            live.project().deltas,
            "persisted raw facts: {language}"
        );
        assert_eq!(
            restored.project().resolution_generations,
            live.project().resolution_generations,
            "persisted generations: {language}"
        );
        assert_answers(
            answers(&restored, &path, &after, probe, resources.keywords.clone()),
            &expected,
            &format!("restart: {language}"),
        );
        restarted.shutdown().await.unwrap();
        qualified.insert(language.to_owned());
    }
    assert_eq!(
        qualified,
        resources
            .grammars
            .entries()
            .map(|g| g.id.to_owned())
            .collect()
    );
}

#[test]
fn complete_language_matrix_matches_runtime_mime_grammar_aliases_and_lexical_fallbacks() {
    let resources = IntelligenceResources::compiled().unwrap();
    let detector = MimeDetector::compiled();
    let matrix = matrix();
    let fixtures = fixtures();
    let rows = matrix["rows"].as_array().unwrap();
    let names: BTreeSet<_> = rows
        .iter()
        .map(|row| row["language"].as_str().unwrap())
        .collect();
    for language in resources
        .keywords
        .languages()
        .iter()
        .map(|name| canonical_language(name))
    {
        assert!(
            names.contains(language.as_str()),
            "missing keyword language {language}"
        );
    }
    for language in resources.definitions.languages().map(canonical_language) {
        assert!(
            names.contains(language.as_str()),
            "missing heuristic language {language}"
        );
    }
    let mut extensions = BTreeSet::new();
    let mut grammars = BTreeSet::new();
    let mut aliases = 0;
    for row in rows {
        let language = row["language"].as_str().unwrap();
        assert_eq!(
            row["keywordSupport"].as_bool().unwrap(),
            resources.keywords.supports(language),
            "keyword {language}"
        );
        for mapping in row["mime"].as_array().unwrap() {
            let extension = mapping["extension"].as_str().unwrap();
            let result = detector.detect_filename(&format!("fixture{extension}"));
            assert_eq!(result.mime, mapping["runtimeMime"], "{extension}");
            assert_eq!(
                result.language.as_deref(),
                mapping["runtimeLanguage"].as_str(),
                "{extension}"
            );
            assert!(
                extensions.insert(extension),
                "duplicate mapping {extension}"
            );
        }
        if let Some(grammar) = resources.grammars.get(language) {
            assert_eq!(
                serde_json::to_value(grammar.metadata()).unwrap(),
                row["grammar"],
                "grammar {language}"
            );
            assert!(grammars.insert(language));
            let fixture = fixtures["bindings"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["grammar"]["language"] == language)
                .unwrap();
            let source = fixture["source"].as_str().unwrap();
            for alias in row["aliases"].as_array().unwrap() {
                let alias = alias.as_str().unwrap();
                assert_eq!(canonical_language(alias), language);
                let delta = extract(
                    &mut SemanticAdapterSession::new(&resources.grammars, alias),
                    &resources,
                    alias,
                    &document(alias, source),
                );
                assert_eq!(delta.file.language_id, language);
                aliases += 1;
            }
        } else {
            assert!(row["grammar"].is_null());
            assert_eq!(
                SemanticAdapterSession::new(&resources.grammars, language).capability(),
                AdapterCapability::Heuristic
            );
        }
    }
    assert_eq!(grammars.len(), 28);
    assert_eq!(aliases, 33);
    assert_eq!(
        extensions.len(),
        matrix["effectiveExtensions"].as_u64().unwrap() as usize
    );
    let manifest: Value =
        serde_json::from_str(include_str!("../../../../resources/grammars/manifest.json")).unwrap();
    let original: BTreeSet<_> = manifest["grammars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["language"].as_str().unwrap())
        .collect();
    assert_eq!(grammars, original);
}

#[test]
fn original_mixed_semantic_highlighting_keeps_soft_keyword_properties_unknown_calls_and_utf8_spans()
{
    use kode_intelligence::tokens::SemanticTokenKind as K;
    let resources = IntelligenceResources::compiled().unwrap();
    let source = "// naïve — 😀\r\n/* first\r\n second */\r\nclass Customer {\r\n val id: Long = 42\r\n val name: String = \"Ada\"\r\n val field = true\r\n val none = null\r\n fun save(input: Customer) { missing(input) }\r\n}\r\n";
    let delta = extract(
        &mut SemanticAdapterSession::new(&resources.grammars, "kotlin"),
        &resources,
        "Customer.kt",
        &document("kotlin", source),
    );
    let snapshot = SemanticSnapshot::new(
        1,
        SemanticResolver::resolve([delta], &CancellationToken::new())
            .unwrap()
            .project,
    )
    .unwrap();
    let tokens = SemanticTokenService::tokens(SemanticIds::file("Customer.kt"), &snapshot, None);
    let kinds = |text: &str| {
        tokens
            .iter()
            .filter(|token| &source[token.range.start.0..token.range.end.0] == text)
            .map(|token| token.kind)
            .collect::<BTreeSet<_>>()
    };
    for (text, kind) in [
        ("// naïve — 😀\r", K::Comment),
        ("/* first\r\n second */", K::Comment),
        ("class", K::Keyword),
        ("42", K::Number),
        ("\"Ada\"", K::String),
        ("true", K::Keyword),
        ("null", K::Keyword),
        ("Customer", K::Class),
        ("id", K::Property),
        ("save", K::Method),
        ("input", K::Parameter),
        ("Long", K::Type),
    ] {
        assert!(kinds(text).contains(&kind), "{text:?}: {:?}", kinds(text));
    }
    assert_eq!(kinds("field"), BTreeSet::from([K::Property]));
    assert_eq!(kinds("missing"), BTreeSet::from([K::Unknown]));
    for token in tokens {
        token.range.validate(source).unwrap();
    }
}
