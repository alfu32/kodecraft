use kode_core::{ByteOffset, TextRange};
use kode_intelligence::{
    ids::*,
    model::*,
    snapshot::{SemanticSnapshot, SnapshotPublisher},
};
use std::sync::Arc;
fn range(start: usize, end: usize) -> SourceRange {
    TextRange::new(ByteOffset(start), ByteOffset(end)).unwrap()
}
fn scope(
    file: FileId,
    start: usize,
    end: usize,
    kind: ScopeKind,
    parent: Option<ScopeId>,
    owner: Option<SymbolId>,
) -> ScopeRecord {
    ScopeRecord {
        id: SemanticIds::scope(file, ByteOffset(start), kind),
        file_id: file,
        parent_id: parent,
        owner_symbol_id: owner,
        kind,
        range: range(start, end),
    }
}
fn symbol(
    file: FileId,
    name: &str,
    start: usize,
    end: usize,
    kind: SymbolKind,
    scope: ScopeId,
    owner: Option<SymbolId>,
) -> SymbolRecord {
    SymbolRecord {
        id: SemanticIds::symbol(file, ByteOffset(start), kind, name),
        file_id: file,
        name: name.into(),
        qualified_name: Some(name.into()),
        kind,
        declaration_range: range(start, end),
        name_range: range(start, start + name.len()),
        scope_id: scope,
        owner_symbol_id: owner,
        declared_type_id: None,
        inferred_type_id: None,
        flags: 0,
        confidence: Confidence::default(),
    }
}
fn delta(name: &str) -> FileSemanticDelta {
    let id = SemanticIds::file("Program.kt");
    let file_scope = scope(id, 0, 100, ScopeKind::File, None, None);
    let class = symbol(id, name, 0, 50, SymbolKind::Class, file_scope.id, None);
    FileSemanticDelta {
        file: FileRecord {
            id,
            path: "Program.kt".into(),
            language_id: "kotlin".into(),
            content_hash: "fixture".into(),
            parse_version: 1,
            semantic_version: 1,
            dependency_generation: 0,
        },
        scopes: vec![file_scope],
        symbols: vec![class],
        occurrences: vec![],
        relations: vec![],
        unresolved_types: vec![],
        imports: vec![],
        type_hints: vec![],
        expression_types: vec![],
        lexical_tokens: vec![],
        exported_surface_hash: "fixture".into(),
    }
}
fn snapshot(delta: FileSemanticDelta) -> SemanticSnapshot {
    SemanticSnapshot::new(1, ResolvedSemanticProject::from_deltas([delta]).unwrap()).unwrap()
}
#[test]
fn frozen_signed_ids_match_kotlin_with_explicit_unicode_byte_coordinates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/languages/semantic/ids-kotlin.json");
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let file = SemanticIds::file(case["path"].as_str().unwrap());
        let start = ByteOffset(case["start"].as_u64().unwrap() as usize);
        let end = ByteOffset(case["end"].as_u64().unwrap() as usize);
        let kind: SymbolKind = serde_json::from_value(case["kind"].clone()).unwrap();
        let scope: ScopeKind = serde_json::from_value(case["scope"].clone()).unwrap();
        assert_eq!(file.0, case["fileId"].as_i64().unwrap());
        assert_eq!(
            SemanticIds::symbol(file, start, kind, case["name"].as_str().unwrap()).0,
            case["symbolId"].as_i64().unwrap()
        );
        assert_eq!(
            SemanticIds::scope(file, start, scope).0,
            case["scopeId"].as_i64().unwrap()
        );
        assert_eq!(
            SemanticIds::occurrence(file, TextRange::new(start, end).unwrap()).0,
            case["occurrenceId"].as_i64().unwrap()
        );
        assert_eq!(
            SemanticIds::type_description("Int").0,
            case["typeId"].as_i64().unwrap()
        );
        if start.0 != case["legacyUtf16Start"].as_u64().unwrap() as usize {
            assert_ne!(
                SemanticIds::symbol(
                    file,
                    ByteOffset(case["legacyUtf16Start"].as_u64().unwrap() as usize),
                    kind,
                    case["name"].as_str().unwrap()
                ),
                SemanticIds::symbol(file, start, kind, case["name"].as_str().unwrap())
            );
        }
    }
}
#[test]
fn model_serialization_type_variants_confidence_and_source_ranges_are_validated() {
    let values = [
        TypeRef::named(SymbolId(-42)),
        TypeRef::primitive("Int"),
        TypeRef::Generic {
            base: Box::new(TypeRef::primitive("List")),
            arguments: vec![TypeRef::Nullable {
                inner: Box::new(TypeRef::named(SymbolId(3))),
            }],
        },
        TypeRef::Function {
            parameters: vec![TypeRef::primitive("String")],
            returns: Some(Box::new(TypeRef::primitive("Int"))),
        },
        TypeRef::Union {
            alternatives: vec![TypeRef::primitive("Int"), TypeRef::Unknown],
        },
        TypeRef::Unknown,
    ];
    for value in values {
        let json = serde_json::to_string(&value).unwrap();
        let restored: TypeRef = serde_json::from_str(&json).unwrap();
        assert_eq!(value, restored);
        assert_eq!(
            SemanticIds::type_ref(&value),
            SemanticIds::type_ref(&restored)
        );
    }
    for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert!(Confidence::new(SemanticSource::Heuristic, value).is_err());
    }
    assert!(serde_json::from_str::<Confidence>(r#"{"source":"LSP","value":1.1}"#).is_err());
    assert!(serde_json::from_str::<SourceRange>(r#"{"start":8,"end":2}"#).is_err());
    assert!(range(1, 2).validate("😀").is_err());
    assert!(range(0, 4).validate("😀").is_ok());
    let delta = delta("Before");
    assert!(delta.validate_source(&"x".repeat(100)).is_ok());
    assert!(delta.validate_source("short").is_err());
    let json = serde_json::to_string(&delta).unwrap();
    assert_eq!(
        serde_json::from_str::<FileSemanticDelta>(&json).unwrap(),
        delta
    );
}
#[test]
fn duplicate_identity_collapse_conflicts_scope_cycles_and_foreign_facts_are_explicit() {
    let mut facts = delta("Class");
    facts.symbols.push(facts.symbols[0].clone());
    assert_eq!(snapshot(facts.clone()).workspace_symbols().count(), 1);
    facts.symbols[1].name = "conflicting identity".into();
    assert!(ResolvedSemanticProject::from_deltas([facts]).is_err());
    let mut facts = delta("Class");
    facts.symbols[0].declared_type_id = Some(TypeId(999));
    assert!(ResolvedSemanticProject::from_deltas([facts]).is_err());
    let mut project = ResolvedSemanticProject::from_deltas([delta("Class")]).unwrap();
    let ty = TypeRef::named(SymbolId(999));
    let id = SemanticIds::type_ref(&ty);
    project.types.insert(
        id,
        TypeRecord {
            id,
            r#ref: ty,
            confidence: Confidence::default(),
            knowledge_level: TypeKnowledgeLevel::CrossFileResolved,
        },
    );
    assert!(SemanticSnapshot::new(1, project).is_err());
    let mut facts = delta("Class");
    facts.scopes[0].parent_id = Some(facts.scopes[0].id);
    assert!(ResolvedSemanticProject::from_deltas([facts]).is_err());
    let mut facts = delta("Class");
    facts.lexical_tokens.push(LexicalTokenRecord {
        file_id: FileId(999),
        range: range(0, 1),
        kind: LexicalTokenKind::Keyword,
        confidence: Confidence::default(),
    });
    assert!(ResolvedSemanticProject::from_deltas([facts]).is_err());
}
#[test]
fn published_snapshot_is_stable_monotonic_and_readable_while_new_tables_replace_it() {
    let publisher = Arc::new(SnapshotPublisher::default());
    publisher.publish(snapshot(delta("Before"))).unwrap();
    let old = publisher.load().unwrap();
    let mut next = delta("After");
    next.file.semantic_version = 2;
    let next =
        SemanticSnapshot::new(2, ResolvedSemanticProject::from_deltas([next]).unwrap()).unwrap();
    let reader = publisher.clone();
    let thread = std::thread::spawn(move || {
        for _ in 0..1000 {
            let view = reader.load().unwrap();
            assert_eq!(view.workspace_symbols().count(), 1);
            assert!(matches!(
                view.workspace_symbols().next().unwrap().name.as_str(),
                "Before" | "After"
            ));
        }
    });
    publisher.publish(next).unwrap();
    thread.join().unwrap();
    assert_eq!(old.workspace_symbols().next().unwrap().name, "Before");
    assert_eq!(
        publisher
            .load()
            .unwrap()
            .workspace_symbols()
            .next()
            .unwrap()
            .name,
        "After"
    );
    assert!(publisher.publish(old).is_err());
}

#[test]
fn immutable_queries_resolve_members_visibility_shadowing_types_and_dependency_tables() {
    let mut facts = delta("Base");
    let file = facts.file.id;
    facts.scopes[0].range = range(0, 300);
    facts.symbols[0].declaration_range = range(0, 70);
    let root = facts.scopes[0].id;
    let base = facts.symbols[0].id;
    let derived = symbol(file, "Derived", 80, 220, SymbolKind::Class, root, None);
    let derived_id = derived.id;
    let base_scope = scope(file, 0, 70, ScopeKind::Type, Some(root), Some(base));
    let derived_scope = scope(file, 80, 220, ScopeKind::Type, Some(root), Some(derived_id));
    let method = symbol(
        file,
        "test",
        120,
        190,
        SymbolKind::Method,
        derived_scope.id,
        Some(derived_id),
    );
    let method_id = method.id;
    let function = scope(
        file,
        130,
        190,
        ScopeKind::Function,
        Some(derived_scope.id),
        Some(method_id),
    );
    let block = scope(
        file,
        140,
        170,
        ScopeKind::Block,
        Some(function.id),
        Some(method_id),
    );
    let mut secret = symbol(
        file,
        "secret",
        20,
        26,
        SymbolKind::Property,
        base_scope.id,
        Some(base),
    );
    secret.flags = SymbolFlags::PRIVATE;
    let mut protected = symbol(
        file,
        "protected",
        30,
        39,
        SymbolKind::Property,
        base_scope.id,
        Some(base),
    );
    protected.flags = SymbolFlags::PROTECTED;
    let inherited = symbol(
        file,
        "member",
        10,
        16,
        SymbolKind::Property,
        base_scope.id,
        Some(base),
    );
    let own = symbol(
        file,
        "member",
        100,
        106,
        SymbolKind::Property,
        derived_scope.id,
        Some(derived_id),
    );
    let own_id = own.id;
    let outer = symbol(
        file,
        "before",
        130,
        136,
        SymbolKind::Variable,
        function.id,
        Some(method_id),
    );
    let inner = symbol(
        file,
        "inner",
        145,
        150,
        SymbolKind::Variable,
        block.id,
        Some(method_id),
    );
    let shadow = symbol(
        file,
        "before",
        150,
        156,
        SymbolKind::Variable,
        block.id,
        Some(method_id),
    );
    let shadow_id = shadow.id;
    let after = symbol(
        file,
        "after",
        175,
        180,
        SymbolKind::Variable,
        function.id,
        Some(method_id),
    );
    for (from, to, kind) in [
        (base, inherited.id, RelationKind::Contains),
        (base, secret.id, RelationKind::Contains),
        (base, protected.id, RelationKind::Contains),
        (derived_id, own_id, RelationKind::Contains),
        (derived_id, method_id, RelationKind::Contains),
        (derived_id, base, RelationKind::Extends),
        (method_id, derived_id, RelationKind::MemberOf),
    ] {
        facts.relations.push(RelationRecord {
            from,
            to,
            kind,
            confidence: Confidence::default(),
        });
    }
    facts.symbols.extend([
        derived,
        inherited,
        secret.clone(),
        protected.clone(),
        own,
        method,
        outer,
        inner,
        shadow,
        after,
    ]);
    facts
        .scopes
        .extend([base_scope, derived_scope, function, block]);
    let occurrence_range = range(160, 166);
    facts.occurrences.push(OccurrenceRecord {
        id: SemanticIds::occurrence(file, occurrence_range),
        file_id: file,
        range: occurrence_range,
        text: "member".into(),
        kind: OccurrenceKind::MemberReference,
        scope_id: facts.scopes.last().unwrap().id,
        resolved_symbol_id: Some(own_id),
        receiver_occurrence_id: None,
        confidence: Confidence::default(),
    });
    facts.expression_types.push(ExpressionTypeRecord {
        file_id: file,
        range: range(160, 166),
        r#type: TypeRef::named(derived_id),
        confidence: Confidence::default(),
    });
    let mut project = ResolvedSemanticProject::from_deltas([facts]).unwrap();
    let ty = TypeRef::named(derived_id);
    let type_id = SemanticIds::type_ref(&ty);
    project.types.insert(
        type_id,
        TypeRecord {
            id: type_id,
            r#ref: ty.clone(),
            confidence: Confidence::default(),
            knowledge_level: TypeKnowledgeLevel::CrossFileResolved,
        },
    );
    project.dependencies.push(FileDependencyRecord {
        from_file_id: file,
        to_file_id: file,
        kind: FileDependencyKind::Reference,
        generation: 7,
    });
    project.resolution_generations.insert(file, 7);
    let snapshot = SemanticSnapshot::new(7, project).unwrap();
    assert_eq!(
        snapshot.symbol_at(file, ByteOffset(161)).unwrap().id,
        own_id
    );
    assert_eq!(
        snapshot.symbol_at(file, ByteOffset(166)).unwrap().id,
        own_id
    );
    assert_eq!(snapshot.references(own_id).count(), 1);
    assert_eq!(snapshot.expression_type(file, range(159, 170)), Some(&ty));
    assert_eq!(&snapshot.type_record(type_id).unwrap().r#ref, &ty);
    let members = snapshot.members(&TypeRef::Nullable {
        inner: Box::new(TypeRef::Generic {
            base: Box::new(ty.clone()),
            arguments: vec![],
        }),
    });
    assert_eq!(
        members
            .iter()
            .filter(|member| member.name == "member")
            .count(),
        1
    );
    assert_eq!(
        members
            .iter()
            .find(|member| member.name == "member")
            .unwrap()
            .id,
        own_id
    );
    assert!(!snapshot.is_accessible(&secret, file, ByteOffset(160)));
    assert!(snapshot.is_accessible(&protected, file, ByteOffset(160)));
    let visible = snapshot.visible_symbols(file, ByteOffset(160));
    assert!(visible.iter().any(|symbol| symbol.name == "inner"));
    assert!(!visible.iter().any(|symbol| symbol.name == "after"));
    assert_eq!(
        visible
            .iter()
            .find(|symbol| symbol.name == "before")
            .unwrap()
            .id,
        shadow_id
    );
    assert!(!visible.iter().any(|symbol| symbol.name == "secret"));
    assert!(visible.iter().any(|symbol| symbol.name == "protected"));
    assert_eq!(snapshot.dependencies(file).count(), 1);
    assert_eq!(snapshot.dependents(file).count(), 1);
    assert_eq!(snapshot.resolution_generation(file), 7);
    let union = TypeRef::Union {
        alternatives: vec![ty, TypeRef::named(base)],
    };
    assert!(
        snapshot
            .members(&union)
            .iter()
            .all(|symbol| symbol.name != "test")
    );
    let mut cyclic = snapshot.project().clone();
    cyclic.relations.push(RelationRecord {
        from: base,
        to: derived_id,
        kind: RelationKind::Extends,
        confidence: Confidence::default(),
    });
    let cyclic = SemanticSnapshot::new(8, cyclic).unwrap();
    assert_eq!(
        cyclic.members(&TypeRef::named(derived_id)).len(),
        members.len()
    );
}
#[test]
fn interval_queries_keep_nested_long_spans_and_best_expression_confidence() {
    let mut facts = delta("Class");
    let file = facts.file.id;
    for (start, end) in [(0, 100), (20, 21), (30, 31)] {
        facts.lexical_tokens.push(LexicalTokenRecord {
            file_id: file,
            range: range(start, end),
            kind: LexicalTokenKind::String,
            confidence: Confidence::default(),
        });
    }
    facts.expression_types = vec![
        ExpressionTypeRecord {
            file_id: file,
            range: range(40, 45),
            r#type: TypeRef::primitive("Int"),
            confidence: Confidence::default(),
        },
        ExpressionTypeRecord {
            file_id: file,
            range: range(40, 45),
            r#type: TypeRef::primitive("Long"),
            confidence: Confidence::new(SemanticSource::Heuristic, 0.5).unwrap(),
        },
        ExpressionTypeRecord {
            file_id: file,
            range: range(40, 40),
            r#type: TypeRef::Unknown,
            confidence: Confidence::default(),
        },
    ];
    let snapshot = snapshot(facts);
    assert_eq!(snapshot.lexical_tokens_in(file, range(50, 60)).count(), 1);
    assert_eq!(snapshot.lexical_tokens_in(file, range(20, 21)).count(), 2);
    assert_eq!(snapshot.lexical_tokens_in(file, range(20, 20)).count(), 0);
    assert_eq!(
        snapshot.expression_type(file, range(40, 45)),
        Some(&TypeRef::primitive("Int"))
    );
}
#[test]
fn public_surface_hash_ignores_local_bodies_but_tracks_signature_identity_and_imports() {
    let mut facts = delta("Public");
    let first = public_surface_hash(&facts);
    let file = facts.file.id;
    let owner = facts.symbols[0].id;
    let method = symbol(
        file,
        "method",
        10,
        40,
        SymbolKind::Method,
        facts.scopes[0].id,
        Some(owner),
    );
    let method_id = method.id;
    facts.symbols.push(method);
    let with_method = public_surface_hash(&facts);
    assert_ne!(first, with_method);
    let local = symbol(
        file,
        "local",
        20,
        25,
        SymbolKind::Variable,
        facts.scopes[0].id,
        Some(method_id),
    );
    facts.symbols.push(local);
    facts.file.content_hash = "new local body".into();
    facts.symbols[1].declaration_range.end = ByteOffset(45);
    assert_eq!(with_method, public_surface_hash(&facts));
    facts.unresolved_types.push(UnresolvedTypeRef {
        owner_symbol_id: method_id,
        scope_id: facts.scopes[0].id,
        name: "String".into(),
        role: TypeRole::Return,
        range: range(10, 16),
        confidence: Confidence::default(),
    });
    assert_ne!(with_method, public_surface_hash(&facts));
    let without_import = public_surface_hash(&facts);
    facts.imports.push(ImportRecord {
        file_id: file,
        scope_id: facts.scopes[0].id,
        path: "demo.Library".into(),
        alias: Some("Lib".into()),
        wildcard: false,
        range: range(60, 72),
    });
    assert_ne!(without_import, public_surface_hash(&facts));
    let old = public_surface_hash(&facts);
    facts.symbols[1].id = SemanticIds::symbol(file, ByteOffset(11), SymbolKind::Method, "method");
    assert_ne!(old, public_surface_hash(&facts));
}
