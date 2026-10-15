use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, TextRange, WorkspaceGeneration, commands::EditKind,
    document::Document,
};
use kode_intelligence::{
    frontend::{kotlin::KotlinSemanticAdapter, kotlin_completion_context},
    ids::SemanticIds,
    model::*,
    snapshot::SemanticSnapshot,
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{parser::ParserSession, registry::GrammarRegistry};
fn document(text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: "kotlin".into(),
            label: "synthetic Kotlin".into(),
        },
        Some("kotlin".into()),
        text,
    )
}
fn extract(text: &str) -> FileSemanticDelta {
    KotlinSemanticAdapter::new(&GrammarRegistry::compiled())
        .unwrap()
        .extract(
            "Fixture.kt",
            &document(text).snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap()
}
fn named<'a>(delta: &'a FileSemanticDelta, name: &str) -> &'a SymbolRecord {
    delta
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| {
            panic!(
                "missing {name}: {:?}",
                delta.symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        })
}
const FIXTURE: &str = include_str!("../../../fixtures/languages/kotlin/extraction.kt");

#[test]
fn extracts_real_ast_declarations_scopes_types_imports_visibility_and_unicode_bytes() {
    let delta = extract(FIXTURE);
    for (name, kind, qualified) in [
        ("core", SymbolKind::Package, "demo.core"),
        ("Café", SymbolKind::Class, "demo.core.Café"),
        ("Face", SymbolKind::Interface, "demo.core.Face"),
        ("Color", SymbolKind::Enum, "demo.core.Color"),
        ("RED", SymbolKind::EnumMember, "demo.core.Color.RED"),
        ("Maybe", SymbolKind::TypeAlias, "demo.core.Maybe"),
        ("name", SymbolKind::Property, "demo.core.Café.name"),
        ("count", SymbolKind::Parameter, "demo.core.Café.count"),
        ("work", SymbolKind::Method, "demo.core.Café.work"),
        ("arg", SymbolKind::Parameter, "demo.core.Café.work.arg"),
        ("top", SymbolKind::Function, "demo.core.top"),
        ("box", SymbolKind::Parameter, "demo.core.top.box"),
        ("LIMIT", SymbolKind::Constant, "demo.core.LIMIT"),
    ] {
        let symbol = named(&delta, name);
        assert_eq!(symbol.kind, kind, "{name}");
        assert_eq!(symbol.qualified_name.as_deref(), Some(qualified));
        assert_eq!(
            FIXTURE[symbol.name_range.start.0..symbol.name_range.end.0].trim_matches('`'),
            name
        );
        assert!(
            delta
                .occurrences
                .iter()
                .any(|o| o.range == symbol.name_range
                    && o.kind == OccurrenceKind::Declaration
                    && o.resolved_symbol_id == Some(symbol.id))
        );
    }
    assert_eq!(
        named(&delta, "Café").name_range.start.0,
        FIXTURE.find("Café").unwrap()
    );
    assert_eq!(named(&delta, "name").flags, SymbolFlags::PRIVATE);
    assert_eq!(named(&delta, "number").flags, SymbolFlags::PROTECTED);
    assert_eq!(named(&delta, "work").flags, SymbolFlags::INTERNAL);
    assert_eq!(delta.imports.len(), 2);
    assert_eq!(
        (&*delta.imports[0].path, delta.imports[0].alias.as_deref()),
        ("example.Customer", Some("Person"))
    );
    assert!(delta.imports[1].wildcard);
    for (owner, name, role) in [
        ("address", "Box<String>?", TypeRole::Declared),
        ("arg", "Person", TypeRole::Parameter),
        ("work", "String", TypeRole::Return),
        ("Café", "Base", TypeRole::SuperType),
        ("Café", "Face", TypeRole::SuperType),
        ("Maybe", "Box<String>?", TypeRole::Declared),
        ("box", "Café<String>?", TypeRole::Parameter),
    ] {
        assert!(
            delta
                .unresolved_types
                .iter()
                .any(|t| t.owner_symbol_id == named(&delta, owner).id
                    && t.name == name
                    && t.role == role),
            "{owner} {name} {role:?}"
        );
    }
    assert!(delta.scopes.iter().any(|s| s.kind == ScopeKind::Lambda));
    assert!(
        delta
            .relations
            .iter()
            .any(|r| r.from == named(&delta, "Café").id
                && r.to == named(&delta, "work").id
                && r.kind == RelationKind::Contains)
    );
    let owner = named(&delta, "Café").id;
    let snapshot =
        SemanticSnapshot::new(1, ResolvedSemanticProject::from_deltas([delta]).unwrap()).unwrap();
    let members = snapshot.members(&TypeRef::named(owner));
    assert!(members.iter().any(|s| s.name == "name"));
    assert!(
        !members
            .iter()
            .any(|s| matches!(s.name.as_str(), "count" | "T"))
    );
}

#[test]
fn literals_hints_member_chains_and_string_interpolations_keep_real_identity_ranges() {
    let delta = extract(FIXTURE);
    for (target, name, kind) in [
        ("copy", "arg", TypeHintKind::Assignment),
        ("value", "Person", TypeHintKind::ConstructorCall),
        ("cast", "Person", TypeHintKind::Cast),
        ("self", "this", TypeHintKind::Assignment),
        ("parent", "super", TypeHintKind::Assignment),
        ("number", "Int", TypeHintKind::Literal),
    ] {
        assert!(
            delta
                .type_hints
                .iter()
                .any(|h| h.target_symbol_id == named(&delta, target).id
                    && h.referenced_name == name
                    && h.kind == kind),
            "{target}: {name}"
        );
    }
    assert!(
        !delta
            .type_hints
            .iter()
            .any(|h| h.target_symbol_id == named(&delta, "nested").id)
    );
    assert!(
        !delta
            .occurrences
            .iter()
            .any(|o| matches!(&*o.text, "true" | "false" | "null"))
    );
    let member = delta.occurrences.iter().find(|o| o.text == "city").unwrap();
    assert_eq!(member.kind, OccurrenceKind::MemberReference);
    assert_eq!(member.resolved_symbol_id, None);
    let receiver = delta
        .occurrences
        .iter()
        .find(|o| Some(o.id) == member.receiver_occurrence_id)
        .unwrap();
    assert_eq!(receiver.text, "address");
    let first = delta
        .occurrences
        .iter()
        .find(|o| Some(o.id) == receiver.receiver_occurrence_id)
        .unwrap();
    assert_eq!(first.text, "arg");
    assert!(delta.occurrences.iter().any(|o| o.text == "name"
        && o.kind == OccurrenceKind::Read
        && FIXTURE[..o.range.start.0].ends_with('$')));
    for (text, primitive) in [("1", "Int"), ("true", "Boolean"), ("\"$name\"", "String")] {
        assert!(
            delta
                .expression_types
                .iter()
                .any(|e| &FIXTURE[e.range.start.0..e.range.end.0] == text
                    && e.r#type == TypeRef::primitive(primitive))
        );
    }
    assert!(
        delta
            .lexical_tokens
            .iter()
            .any(|t| t.kind == LexicalTokenKind::Keyword
                && &FIXTURE[t.range.start.0..t.range.end.0] == "null")
    );
    for occurrence in &delta.occurrences {
        assert_eq!(
            FIXTURE[occurrence.range.start.0..occurrence.range.end.0].trim_matches('`'),
            occurrence.text
        );
    }
    let calls = extract("fun use(arg: Any) { factory(arg).field }\n");
    let field = calls
        .occurrences
        .iter()
        .find(|o| o.text == "field")
        .unwrap();
    let receiver = calls
        .occurrences
        .iter()
        .find(|o| Some(o.id) == field.receiver_occurrence_id)
        .unwrap();
    assert_eq!(receiver.text, "factory");
}

#[test]
fn declaration_order_shadowing_and_assignment_hints_stay_in_their_lexical_scope() {
    let text = "fun test() {\n val before = 1\n if (true) {\n var before = \"two\"\n before = \"three\"\n /*caret*/\n }\n val after = 3\n}\n";
    let delta = extract(text);
    let variables: Vec<_> = delta
        .symbols
        .iter()
        .filter(|s| s.name == "before")
        .collect();
    assert_eq!(variables.len(), 2);
    assert_ne!(variables[0].id, variables[1].id);
    assert_ne!(variables[0].scope_id, variables[1].scope_id);
    let hint = delta
        .type_hints
        .iter()
        .find(|h| &text[h.expression_range.start.0..h.expression_range.end.0] == "\"three\"")
        .unwrap();
    assert_eq!(hint.target_symbol_id, variables[1].id);
    let inner = variables[1].id;
    let snapshot =
        SemanticSnapshot::new(1, ResolvedSemanticProject::from_deltas([delta]).unwrap()).unwrap();
    let visible = snapshot.visible_symbols(
        SemanticIds::file("Fixture.kt"),
        ByteOffset(text.find("/*caret*/").unwrap()),
    );
    assert_eq!(visible.iter().filter(|s| s.name == "before").count(), 1);
    assert_eq!(
        visible.iter().find(|s| s.name == "before").unwrap().id,
        inner
    );
    assert!(!visible.iter().any(|s| s.name == "after"));
}

#[test]
fn incomplete_empty_backtick_and_unicode_inputs_remain_valid_partial_facts() {
    for text in [
        "",
        "// 😀\r\nclass Café {\r\n val `hello world`: String = \"\"\r\n fun work() {\r\n  val 中 = 1\r\n  中.",
        "fun before() {}\nfun broken(value:",
        "class Box<T> { val value: List<T?>? = null\n",
    ] {
        let delta = extract(text);
        delta.validate_source(text).unwrap();
        SemanticSnapshot::new(1, ResolvedSemanticProject::from_deltas([delta]).unwrap()).unwrap();
    }
    let text =
        "// 😀\r\nval `hello world`: String = \"a\"\r\nfun use() { println(`hello world`) }\r\n";
    let delta = extract(text);
    assert_eq!(
        named(&delta, "hello world").name_range.start.0,
        text.find('`').unwrap()
    );
    assert_eq!(
        delta
            .occurrences
            .iter()
            .filter(|o| o.text == "hello world")
            .count(),
        2
    );
}

#[test]
fn real_reused_and_fresh_parses_extract_equivalent_facts_and_preserve_old_delta() {
    let registry = GrammarRegistry::compiled();
    let mut adapter = KotlinSemanticAdapter::new(&registry).unwrap();
    let mut doc = document(FIXTURE);
    let cancel = CancellationToken::new();
    let old = adapter
        .extract(
            "Fixture.kt",
            &doc.snapshot(),
            WorkspaceGeneration(1),
            &cancel,
        )
        .unwrap();
    for (from, to) in [
        ("Café", "Cafè"),
        ("1", "222"),
        ("val copy = arg", "val copy = Person()"),
        ("String", "List<String>?"),
    ] {
        let text = doc.snapshot().text();
        let start = text.find(from).unwrap();
        doc.apply_edit(
            doc.revision(),
            TextRange::new(ByteOffset(start), ByteOffset(start + from.len())).unwrap(),
            to,
            EditKind::Typing,
        )
        .unwrap();
        let reused = adapter
            .extract(
                "Fixture.kt",
                &doc.snapshot(),
                WorkspaceGeneration(1),
                &cancel,
            )
            .unwrap();
        let fresh = KotlinSemanticAdapter::new(&registry)
            .unwrap()
            .extract(
                "Fixture.kt",
                &doc.snapshot(),
                WorkspaceGeneration(1),
                &cancel,
            )
            .unwrap();
        assert_eq!(reused, fresh);
    }
    assert!(old.symbols.iter().any(|s| s.name == "Café"));
    cancel.cancel();
    assert!(matches!(
        adapter.extract(
            "Fixture.kt",
            &doc.snapshot(),
            WorkspaceGeneration(1),
            &cancel
        ),
        Err(kode_core::DomainError::Cancelled)
    ));
    let parsed_c = ParserSession::new(registry.get("c").unwrap())
        .unwrap()
        .parse(
            &document("int x;").snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(
        KotlinSemanticAdapter::extract_parsed("Wrong.kt", &parsed_c, &CancellationToken::new())
            .is_err()
    );
}

#[test]
fn public_surface_hash_preserves_source_body_signature_and_movement_distinctions() {
    let source = |value, ty| {
        format!("class Customer\nfun load(): {ty} {{ val local = {value}; return Customer() }}")
    };
    let first = extract(&source(1, "Customer"));
    let body = extract(&source(2000, "Customer"));
    let signature = extract(&source(2000, "String"));
    let moved = extract(&format!("\n{}", source(1, "Customer")));
    assert_ne!(first.file.content_hash, body.file.content_hash);
    assert_eq!(first.exported_surface_hash, body.exported_surface_hash);
    assert_ne!(first.exported_surface_hash, signature.exported_surface_hash);
    assert_ne!(first.exported_surface_hash, moved.exported_surface_hash);
}

#[test]
fn completion_context_retains_parenthesized_calls_casts_chains_and_utf8_ranges() {
    for (text, receiver, prefix) in [
        ("中.address.ci", "中.address", "ci"),
        ("val x = factory().na", "factory()", "na"),
        ("\n(value as Café)?.", "(value as Café)", ""),
        ("box?.si", "box", "si"),
        ("foo[0].x", "foo[0]", "x"),
    ] {
        let context = kotlin_completion_context(text, ByteOffset(text.len())).unwrap();
        assert!(context.member_access);
        assert_eq!(context.prefix, prefix);
        let range = context.receiver_range.unwrap();
        assert_eq!(&text[range.start.0..range.end.0], receiver);
    }
    assert!(
        !kotlin_completion_context("fun name", ByteOffset(8))
            .unwrap()
            .member_access
    );
    assert!(kotlin_completion_context("😀", ByteOffset(1)).is_err());
}

#[test]
fn literal_shapes_escapes_and_explicit_interpolation_do_not_create_false_references() {
    let text = r#"fun example(name: String) {
 val values = listOf(1L, 2.5, 3f, 0xFF, 'x', true, false, null)
 val escaped = "\$name"
 val interpolated = "${name}"
 val raw = """$name"""
}"#;
    let delta = extract(text);
    for (literal, ty) in [
        ("1L", "Long"),
        ("2.5", "Double"),
        ("3f", "Float"),
        ("0xFF", "Int"),
        ("'x'", "Char"),
        ("true", "Boolean"),
    ] {
        assert!(
            delta
                .expression_types
                .iter()
                .any(|e| &text[e.range.start.0..e.range.end.0] == literal
                    && e.r#type == TypeRef::primitive(ty)),
            "{literal}"
        );
    }
    let references: Vec<_> = delta
        .occurrences
        .iter()
        .filter(|o| o.text == "name" && o.kind == OccurrenceKind::Read)
        .collect();
    assert_eq!(references.len(), 2);
    let escaped = text.find("\\$name").unwrap();
    assert!(!references.iter().any(|o| o.range.start.0 == escaped + 2));
}

#[test]
fn extraction_cancels_and_rejects_excessive_depth_before_building_a_snapshot() {
    let registry = GrammarRegistry::compiled();
    let text = format!(
        "fun deep() {{ {} val value = 1 {} }}",
        "if (true) {".repeat(140),
        "}".repeat(140)
    );
    let parsed = ParserSession::new(registry.get("kotlin").unwrap())
        .unwrap()
        .parse(
            &document(&text).snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        KotlinSemanticAdapter::extract_parsed("Deep.kt", &parsed, &cancel),
        Err(kode_core::DomainError::Cancelled)
    ));
    assert!(
        matches!(KotlinSemanticAdapter::extract_parsed("Deep.kt", &parsed, &CancellationToken::new()), Err(kode_core::DomainError::Unsupported(message)) if message.contains("work limit"))
    );
}
