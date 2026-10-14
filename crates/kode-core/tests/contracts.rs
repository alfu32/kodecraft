use kode_core::intelligence::{
    CompletionItem, DocumentSnapshot, HighlightSpan, IntelligenceProvider, IntelligenceRequest,
    NavigationTarget, identifier_prefix,
};
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, JobTag, RequestId, Revision, Selection, TextRange,
    WorkspaceGeneration,
};
use std::sync::Arc;

#[test]
fn wire_types_preserve_byte_units_and_reverse_selection() {
    let selection: Selection = serde_json::from_str(r#"{"anchor":5,"head":1}"#).unwrap();
    assert!(selection.is_reverse());
    assert_eq!(
        selection.range(),
        TextRange::new(ByteOffset(1), ByteOffset(5)).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&selection).unwrap(),
        r#"{"anchor":5,"head":1}"#
    );
    assert!(selection.range().validate("a😀b").is_ok());
    assert!(
        TextRange::new(ByteOffset(2), ByteOffset(5))
            .unwrap()
            .validate("a😀b")
            .is_err()
    );
    assert!(serde_json::from_str::<TextRange>(r#"{"start":5,"end":1}"#).is_err());
}

#[test]
fn job_identity_rejects_every_kind_of_stale_result() {
    let current = JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId(2),
        document: Some((DocumentId(3), Revision(4))),
    };
    assert!(current.is_current(current));
    for stale in [
        JobTag {
            workspace: WorkspaceGeneration(0),
            ..current
        },
        JobTag {
            request: RequestId(1),
            ..current
        },
        JobTag {
            document: Some((DocumentId(0), Revision(4))),
            ..current
        },
        JobTag {
            document: Some((DocumentId(3), Revision(0))),
            ..current
        },
    ] {
        assert!(!stale.is_current(current));
    }
}

#[test]
fn completion_fallback_never_eats_receiver_or_suffix() {
    for (text, cursor, expected) in [
        ("customer.", 9, ""),
        ("customer.na + suffix", 11, "na"),
        ("customer?.name", 14, "name"),
        ("$value_2", 8, "$value_2"),
    ] {
        let range = identifier_prefix(text, ByteOffset(cursor)).unwrap();
        assert_eq!(&text[range.start.0..range.end.0], expected);
    }
    assert!(identifier_prefix("😀", ByteOffset(1)).is_err());
}

struct FakeIntelligence;
impl IntelligenceProvider for FakeIntelligence {
    fn definitions(&self, _: &IntelligenceRequest) -> kode_core::Result<Vec<NavigationTarget>> {
        Ok(vec![])
    }
    fn references(&self, _: &IntelligenceRequest) -> kode_core::Result<Vec<NavigationTarget>> {
        Ok(vec![])
    }
    fn completions(&self, _: &IntelligenceRequest) -> kode_core::Result<Vec<CompletionItem>> {
        Ok(vec![])
    }
    fn highlights(&self, _: &DocumentSnapshot) -> kode_core::Result<Vec<HighlightSpan>> {
        Ok(vec![])
    }
}

#[test]
fn virtual_document_snapshot_roundtrips_without_a_file_path() {
    let snapshot = DocumentSnapshot {
        id: DocumentId(1),
        revision: Revision(2),
        origin: DocumentOrigin::Virtual {
            owner: "rest".into(),
            key: "response:1".into(),
            label: "Response".into(),
        },
        language: Some("json".into()),
        format: Default::default(),
        text: Arc::from("{\"future\":true}"),
    };
    let encoded = serde_json::to_vec(&snapshot).unwrap();
    let decoded: DocumentSnapshot = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, snapshot);
    assert!(FakeIntelligence.highlights(&decoded).unwrap().is_empty());
}
