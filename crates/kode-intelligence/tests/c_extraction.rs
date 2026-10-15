use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, TextRange, WorkspaceGeneration, commands::EditKind,
    document::Document,
};
use kode_intelligence::{
    frontend::c::{CSemanticAdapter, completion_context},
    ids::SemanticIds,
    model::*,
    snapshot::SemanticSnapshot,
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::registry::GrammarRegistry;
fn doc(text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: "c".into(),
            label: "synthetic C".into(),
        },
        Some("c".into()),
        text,
    )
}
fn extract(path: &str, text: &str) -> FileSemanticDelta {
    CSemanticAdapter::new(&GrammarRegistry::compiled())
        .unwrap()
        .extract(
            path,
            &doc(text).snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap()
}
fn named<'a>(delta: &'a FileSemanticDelta, name: &str, kind: SymbolKind) -> &'a SymbolRecord {
    delta
        .symbols
        .iter()
        .find(|s| s.name == name && s.kind == kind)
        .unwrap_or_else(|| {
            panic!(
                "missing {name} {kind:?}: {:?}",
                delta
                    .symbols
                    .iter()
                    .map(|s| (&s.name, s.kind))
                    .collect::<Vec<_>>()
            )
        })
}
const HEADER: &str = include_str!("../../../fixtures/languages/c/extraction.h");
const SOURCE: &str = include_str!("../../../fixtures/languages/c/extraction.c");

#[test]
fn tags_aliases_anonymous_types_fields_enums_and_pointer_prototypes_keep_distinct_identity() {
    let delta = extract("extraction.h", HEADER);
    let tag = named(&delta, "Foo", SymbolKind::Struct);
    let alias = named(&delta, "Foo", SymbolKind::TypeAlias);
    assert_ne!(tag.id, alias.id);
    assert_eq!(delta.symbols.iter().filter(|s| s.name == "Foo").count(), 2);
    let anonymous = delta
        .symbols
        .iter()
        .find(|s| s.name.starts_with("__anonymous_at_"))
        .unwrap();
    assert_eq!(anonymous.kind, SymbolKind::Struct);
    assert!(anonymous.name_range.is_empty());
    let bar = named(&delta, "Bar", SymbolKind::TypeAlias);
    assert!(
        delta
            .unresolved_types
            .iter()
            .any(|t| t.owner_symbol_id == bar.id && t.name == format!("struct {}", anonymous.name))
    );
    assert!(
        delta
            .unresolved_types
            .iter()
            .any(|t| t.owner_symbol_id == alias.id && t.name == "struct Foo")
    );
    for (symbol, ty, role) in [
        (
            named(&delta, "name", SymbolKind::Field),
            "char *",
            TypeRole::Declared,
        ),
        (
            named(&delta, "next", SymbolKind::Field),
            "struct Foo *",
            TypeRole::Declared,
        ),
        (
            named(&delta, "create", SymbolKind::Function),
            "Foo *",
            TypeRole::Return,
        ),
        (
            named(&delta, "count", SymbolKind::Parameter),
            "int",
            TypeRole::Parameter,
        ),
    ] {
        assert!(
            delta
                .unresolved_types
                .iter()
                .any(|t| t.owner_symbol_id == symbol.id && t.name == ty && t.role == role),
            "{} {ty}",
            symbol.name
        );
    }
    let tag_id = tag.id;
    let color = named(&delta, "Color", SymbolKind::Enum).id;
    let snapshot =
        SemanticSnapshot::new(1, ResolvedSemanticProject::from_deltas([delta]).unwrap()).unwrap();
    assert_eq!(
        snapshot
            .members(&TypeRef::named(tag_id))
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["id", "name", "next"]
    );
    assert_eq!(
        snapshot
            .members(&TypeRef::named(color))
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["RED", "BLUE"]
    );
}

#[test]
fn consumer_deltas_join_other_files_without_resolving_names_or_losing_byte_ranges() {
    let delta = extract("extraction.c", SOURCE);
    assert_eq!(delta.imports[0].path, "extraction.h");
    for (name, ty) in [
        ("ptr", "Foo *"),
        ("tagged", "struct Foo"),
        ("aliased", "Foo"),
        ("other", "Foo *"),
        ("bar", "Bar"),
    ] {
        let symbol = named(
            &delta,
            name,
            if name == "ptr" {
                SymbolKind::Parameter
            } else {
                SymbolKind::Variable
            },
        );
        assert!(
            delta
                .unresolved_types
                .iter()
                .any(|t| t.owner_symbol_id == symbol.id && t.name == ty)
        );
    }
    assert!(
        delta
            .occurrences
            .iter()
            .filter(|o| o.kind == OccurrenceKind::MemberReference)
            .all(|o| o.resolved_symbol_id.is_none())
    );
    for o in &delta.occurrences {
        assert_eq!(&SOURCE[o.range.start.0..o.range.end.0], o.text);
        assert!(!o.text.contains(['.', '>', ';']));
    }
    assert!(
        delta
            .symbols
            .iter()
            .any(|s| s.name_range.start.0 > SOURCE[..s.name_range.start.0].encode_utf16().count())
    );
    let header = extract("extraction.h", HEADER);
    let project = ResolvedSemanticProject::from_deltas([delta.clone(), header.clone()]).unwrap();
    let reversed = ResolvedSemanticProject::from_deltas([header, delta]).unwrap();
    let first = SemanticSnapshot::new(1, project).unwrap();
    let second = SemanticSnapshot::new(1, reversed).unwrap();
    assert_eq!(first.project().deltas, second.project().deltas);
}

#[test]
fn member_receiver_links_follow_chains_and_call_results_without_choosing_arguments() {
    let delta = extract("extraction.c", SOURCE);
    for (expression, receiver) in [
        ("tagged.id", "tagged"),
        ("aliased.id", "aliased"),
        ("ptr->id", "ptr"),
        ("ptr->next->id", "next"),
        ("create(index)->id", "create"),
        ("unknown.id", "unknown"),
    ] {
        let start = SOURCE.find(expression).unwrap() + expression.rfind("id").unwrap();
        let occurrence = delta
            .occurrences
            .iter()
            .find(|o| o.range.start.0 == start)
            .unwrap();
        assert_eq!(occurrence.kind, OccurrenceKind::MemberReference);
        let link = delta
            .occurrences
            .iter()
            .find(|o| Some(o.id) == occurrence.receiver_occurrence_id)
            .unwrap();
        assert_eq!(link.text, receiver, "{expression}");
    }
}

#[test]
fn incomplete_unicode_and_complex_declarators_have_valid_ranges_and_scopes() {
    for text in [
        "",
        "/* 中 */\r\ntypedef struct Box { int value; } Box;\r\nvoid use(Box *ptr) { ptr->",
        "int a = 1, *b, values[4];\nint (*callback)(int input);\nint *factory(void);\n",
        "struct Item { int (*callback)(int input); int (*rows)[2]; };\n",
    ] {
        let delta = extract("partial.c", text);
        delta.validate_source(text).unwrap();
        SemanticSnapshot::new(1, ResolvedSemanticProject::from_deltas([delta]).unwrap()).unwrap();
    }
    let delta = extract(
        "declarations.c",
        "int a = 1, *b, values[4];\nint (*callback)(int input);\nint *factory(void);\n",
    );
    for name in ["a", "b", "values", "callback"] {
        named(&delta, name, SymbolKind::Variable);
    }
    named(&delta, "factory", SymbolKind::Function);
}

#[test]
fn incremental_field_edits_match_fresh_extraction_keep_old_facts_and_change_surface() {
    let mut document = doc(HEADER);
    let registry = GrammarRegistry::compiled();
    let mut adapter = CSemanticAdapter::new(&registry).unwrap();
    let token = CancellationToken::new();
    let old = adapter
        .extract(
            "types.h",
            &document.snapshot(),
            WorkspaceGeneration(1),
            &token,
        )
        .unwrap();
    for (from, to) in [
        ("int id;", "int id; int extra;"),
        ("char *name", "char **title"),
        ("typedef struct Foo", "/* 😀 */ typedef struct Foo"),
    ] {
        let source = document.snapshot().text();
        let start = source.find(from).unwrap();
        document
            .apply_edit(
                document.revision(),
                TextRange::new(ByteOffset(start), ByteOffset(start + from.len())).unwrap(),
                to,
                EditKind::Typing,
            )
            .unwrap();
        let reused = adapter
            .extract(
                "types.h",
                &document.snapshot(),
                WorkspaceGeneration(1),
                &token,
            )
            .unwrap();
        let fresh = CSemanticAdapter::new(&registry)
            .unwrap()
            .extract(
                "types.h",
                &document.snapshot(),
                WorkspaceGeneration(1),
                &token,
            )
            .unwrap();
        assert_eq!(reused, fresh);
        assert_ne!(reused.exported_surface_hash, old.exported_surface_hash);
    }
    assert!(!old.symbols.iter().any(|s| s.name == "extra"));
    token.cancel();
    assert!(matches!(
        adapter.extract(
            "types.h",
            &document.snapshot(),
            WorkspaceGeneration(1),
            &token
        ),
        Err(kode_core::DomainError::Cancelled)
    ));
}

#[test]
fn c_completion_receivers_preserve_source_dot_arrow_index_and_call_context() {
    for (text, receiver, prefix) in [
        ("ptr->co", "ptr", "co"),
        ("item.next.va", "item.next", "va"),
        ("x = create(arg)->na", "create(arg)", "na"),
        ("items[0].", "items[0]", ""),
        ("中->va", "中", "va"),
    ] {
        let context = completion_context(text, ByteOffset(text.len())).unwrap();
        assert!(context.member_access);
        let range = context.receiver_range.unwrap();
        assert_eq!(&text[range.start.0..range.end.0], receiver);
        assert_eq!(context.prefix, prefix);
    }
    assert!(
        !completion_context("plain", ByteOffset(5))
            .unwrap()
            .member_access
    );
    assert!(completion_context("😀", ByteOffset(1)).is_err());
    assert_ne!(SemanticIds::file("types.h"), SemanticIds::file("main.c"));
}
