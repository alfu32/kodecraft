use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, JobTag, RequestId, Result, Revision, TextRange,
    WorkspaceGeneration,
    commands::EditKind,
    document::Document,
    intelligence::{
        CompletionItem, DocumentSnapshot, HighlightSpan, IntelligenceProvider, IntelligenceRequest,
        NavigationTarget,
    },
};
use kode_lsp::{
    catalog::ServerEntry,
    client::ClientOptions,
    discovery::ResolvedCommand,
    documents::{PositionEncoding, PositionMapper, file_uri},
    provider::EditorLsp,
    service::LspService,
};
use kode_platform::{jobs::CancellationToken, process::SystemProcessFactory};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
struct Fallback(AtomicUsize);
impl IntelligenceProvider for Fallback {
    fn definitions(&self, _: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![NavigationTarget {
            path: "native.kt".into(),
            document: None,
            identifier_range: TextRange::caret(ByteOffset(0)),
            location: None,
            declaration_range: None,
            language: None,
            kind: None,
            name: Some("native".into()),
            missing_source: false,
        }])
    }
    fn references(&self, r: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        self.definitions(r)
    }
    fn completions(&self, _: &IntelligenceRequest) -> Result<Vec<CompletionItem>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![CompletionItem {
            label: "native".into(),
            insert_text: "native".into(),
            detail: None,
            kind: None,
            replacement_range: TextRange::caret(ByteOffset(0)),
            source: "native".into(),
        }])
    }
    fn highlights(&self, _: &DocumentSnapshot) -> Result<Vec<HighlightSpan>> {
        Ok(vec![HighlightSpan {
            range: TextRange::caret(ByteOffset(0)),
            class: "native".into(),
        }])
    }
}
struct Fixture {
    directory: tempfile::TempDir,
    service: Arc<LspService>,
    editor: Arc<EditorLsp>,
    client: kode_lsp::client::LspClient,
    document: Document,
    fallback: Arc<Fallback>,
}
impl Fixture {
    async fn new(mode: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/lsp/server.py");
        let mut options = ClientOptions::new(directory.path().into());
        options.configuration = json!({"fixture":{"option":42}});
        options.request_timeout = Duration::from_millis(500);
        let service = Arc::new(LspService::with_factory(
            options,
            Arc::new(SystemProcessFactory),
            None,
        ));
        let entry: ServerEntry = serde_json::from_value(
            json!({"id":"fixture","name":"Fixture","languages":["kotlin"],"versions":[]}),
        )
        .unwrap();
        let command = ResolvedCommand {
            args: vec![
                "python3".into(),
                script.display().to_string(),
                mode.into(),
                directory.path().join("trace.jsonl").display().to_string(),
                directory.path().join("pid").display().to_string(),
            ],
            cwd: None,
        };
        let client = service
            .start(entry, command, &CancellationToken::new())
            .await
            .unwrap();
        let editor = Arc::new(EditorLsp::new(
            service.clone(),
            WorkspaceGeneration(1),
            tokio::runtime::Handle::current(),
        ));
        let document = Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::File(directory.path().join("source 😀.kt")),
            Some("kt".into()),
            "// 😀 café\r\nval café = 1\r\n",
        );
        editor.replace_sources([document.snapshot()]);
        Self {
            directory,
            service,
            editor,
            client,
            document,
            fallback: Arc::new(Fallback(AtomicUsize::new(0))),
        }
    }
    fn request(&self) -> IntelligenceRequest {
        let snapshot = self.document.snapshot();
        IntelligenceRequest {
            tag: JobTag {
                workspace: WorkspaceGeneration(1),
                request: RequestId::fresh().unwrap(),
                document: Some((snapshot.id, snapshot.revision)),
            },
            snapshot: snapshot.to_service_snapshot(),
            position: ByteOffset("// 😀 café".len()),
        }
    }
    fn provider(&self) -> Arc<dyn IntelligenceProvider> {
        self.editor
            .provider(Some(self.fallback.clone()), CancellationToken::new())
    }
    async fn configure(&self, value: Value) {
        self.client
            .request("fixture/configure", value, &CancellationToken::new())
            .await
            .unwrap();
    }
    fn trace(&self) -> Vec<Value> {
        fs::read_to_string(self.directory.path().join("trace.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
#[tokio::test]
async fn nonempty_primary_wins_native_handles_empty_errors_missing_crashed_and_virtual_documents() {
    let fixture = Fixture::new("fragment").await;
    let provider = fixture.provider();
    let request = fixture.request();
    let copy = provider.clone();
    let captured = request.clone();
    let (definitions, references, completions, highlights) =
        tokio::task::spawn_blocking(move || {
            (
                copy.definitions(&captured).unwrap(),
                copy.references(&captured).unwrap(),
                copy.completions(&captured).unwrap(),
                copy.highlights(&captured.snapshot).unwrap(),
            )
        })
        .await
        .unwrap();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].document, Some(fixture.document.id()));
    let mapper = PositionMapper::new(fixture.document.snapshot(), PositionEncoding::Utf16).unwrap();
    assert_eq!(
        definitions[0].identifier_range,
        mapper
            .byte_range(lsp_types::Range::new(
                lsp_types::Position::new(1, 2),
                lsp_types::Position::new(1, 6)
            ))
            .unwrap()
    );
    assert!(definitions[0].declaration_range.is_some());
    assert_eq!(
        references[0].identifier_range,
        TextRange::caret(request.position)
    );
    assert_eq!(completions[0].label, "naïve");
    assert_eq!(completions[0].source, "lsp");
    assert_eq!(highlights[0].class, "native");
    assert_eq!(fixture.fallback.0.load(Ordering::SeqCst), 0);
    for behavior in [json!({"empty":true}), json!({"error":true})] {
        fixture.configure(behavior).await;
        let copy = provider.clone();
        let captured = request.clone();
        tokio::task::spawn_blocking(move || {
            assert_eq!(
                copy.definitions(&captured).unwrap()[0].name.as_deref(),
                Some("native")
            );
            assert_eq!(
                copy.references(&captured).unwrap()[0].name.as_deref(),
                Some("native")
            );
            assert_eq!(copy.completions(&captured).unwrap()[0].source, "native");
        })
        .await
        .unwrap();
    }
    let _ = fixture
        .client
        .request("fixture/crash", json!({}), &CancellationToken::new())
        .await;
    let copy = provider.clone();
    let captured = request.clone();
    tokio::task::spawn_blocking(move || {
        assert_eq!(copy.completions(&captured).unwrap()[0].source, "native")
    })
    .await
    .unwrap();
    let virtual_document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: "query".into(),
            label: "Query".into(),
        },
        Some("kotlin".into()),
        "val native = 1",
    );
    let mut virtual_request = request;
    virtual_request.snapshot = virtual_document.snapshot().to_service_snapshot();
    virtual_request.position = ByteOffset(0);
    tokio::task::spawn_blocking(move || {
        assert_eq!(
            provider.completions(&virtual_request).unwrap()[0].source,
            "native"
        )
    })
    .await
    .unwrap();
    fixture.service.shutdown().await.unwrap();
}
#[tokio::test]
async fn all_negotiated_encodings_convert_queries_and_returned_locations_to_exact_editor_bytes() {
    for (mode, encoding, column) in [
        ("utf8", PositionEncoding::Utf8, 13),
        ("fragment", PositionEncoding::Utf16, 10),
        ("utf32", PositionEncoding::Utf32, 9),
    ] {
        let fixture = Fixture::new(mode).await;
        assert_eq!(fixture.client.info().unwrap().encoding, encoding);
        let provider = fixture.provider();
        let request = fixture.request();
        let expected = request.position;
        let targets = tokio::task::spawn_blocking(move || provider.references(&request).unwrap())
            .await
            .unwrap();
        assert_eq!(targets[0].identifier_range, TextRange::caret(expected));
        assert_eq!(targets[0].location.unwrap().column.0, 10);
        let trace = fixture.trace();
        let query = trace
            .iter()
            .find(|value| value["method"] == "textDocument/references")
            .unwrap();
        assert_eq!(
            query["params"]["position"],
            json!({"line":0,"character":column})
        );
        fixture.service.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn late_response_after_edit_or_request_cancellation_cannot_win_over_native_fallback() {
    let mut fixture = Fixture::new("fragment").await;
    fixture.configure(json!({"delay":0.2})).await;
    let request = fixture.request();
    let provider = fixture.provider();
    let pending = tokio::task::spawn_blocking(move || provider.completions(&request));
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !fixture
        .trace()
        .iter()
        .any(|message| message["method"] == "textDocument/completion")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "fixture did not receive delayed query"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    fixture
        .document
        .apply_edit(
            Revision(0),
            TextRange::caret(ByteOffset(0)),
            "// changed\n",
            EditKind::Typing,
        )
        .unwrap();
    fixture
        .editor
        .replace_sources([fixture.document.snapshot()]);
    assert_eq!(pending.await.unwrap().unwrap()[0].source, "native");
    let cancel = CancellationToken::new();
    let provider = fixture
        .editor
        .provider(Some(fixture.fallback.clone()), cancel.clone());
    let request = fixture.request();
    cancel.cancel();
    let result = tokio::task::spawn_blocking(move || provider.completions(&request))
        .await
        .unwrap();
    assert!(matches!(result, Err(kode_core::DomainError::Cancelled)));
    let provider = fixture.provider();
    let mut request = fixture.request();
    request.tag.workspace = WorkspaceGeneration(2);
    assert!(matches!(
        tokio::task::spawn_blocking(move || provider.completions(&request))
            .await
            .unwrap(),
        Err(kode_core::DomainError::Cancelled)
    ));
    fixture.editor.stop();
    let provider = fixture.provider();
    let request = fixture.request();
    assert!(matches!(
        tokio::task::spawn_blocking(move || provider.definitions(&request))
            .await
            .unwrap(),
        Err(kode_core::DomainError::Cancelled)
    ));
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn completion_edits_preserve_prefix_contract_and_unsupported_items_fall_back_instead_of_inserting_snippet_commands()
 {
    use lsp_types::{
        CompletionItem, CompletionItemKind, CompletionTextEdit, InsertTextFormat, TextEdit,
    };
    let fixture = Fixture::new("fragment").await;
    let mut request = fixture.request();
    request.position = ByteOffset("// 😀 caf".len());
    let prefix =
        kode_core::intelligence::identifier_prefix(&request.snapshot.text, request.position)
            .unwrap();
    let mapper = PositionMapper::new(fixture.document.snapshot(), PositionEncoding::Utf16).unwrap();
    let range = lsp_types::Range::new(
        mapper.position(prefix.start).unwrap(),
        mapper.position(prefix.end).unwrap(),
    );
    let valid = CompletionItem {
        label: "replacement".into(),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: "café".into(),
        })),
        kind: Some(CompletionItemKind::VARIABLE),
        ..Default::default()
    };
    let snippet = CompletionItem {
        label: "snippet".into(),
        insert_text: Some("${1:unsafe}".into()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    };
    let additional = CompletionItem {
        label: "additional".into(),
        additional_text_edits: Some(vec![TextEdit {
            range,
            new_text: "unadvertised edit".into(),
        }]),
        ..Default::default()
    };
    let outside = CompletionItem {
        label: "outside".into(),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: lsp_types::Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 1),
            ),
            new_text: "unsafe".into(),
        })),
        ..Default::default()
    };
    fixture
        .configure(json!({"textDocument/completion":[valid,valid,snippet,additional,outside]}))
        .await;
    let (provider, usage) = fixture
        .editor
        .tracked_provider(Some(fixture.fallback.clone()), CancellationToken::new());
    let copy = request.clone();
    let items = tokio::task::spawn_blocking(move || provider.completions(&copy).unwrap())
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].insert_text, "café");
    assert_eq!(items[0].replacement_range, prefix);
    assert_eq!(fixture.fallback.0.load(Ordering::SeqCst), 0);
    let proof = usage.proof().unwrap();
    assert!(proof.is_current());
    fixture
        .configure(json!({"textDocument/completion":[snippet,additional,outside]}))
        .await;
    let (provider, usage) = fixture
        .editor
        .tracked_provider(Some(fixture.fallback.clone()), CancellationToken::new());
    let items = tokio::task::spawn_blocking(move || provider.completions(&request).unwrap())
        .await
        .unwrap();
    assert_eq!(items[0].source, "native");
    assert!(usage.proof().is_none());
    fixture.service.stop("fixture").await.unwrap();
    assert!(
        !proof.is_current(),
        "a queued result must be invalidated by process disposal"
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn locations_use_other_unsaved_captures_reject_nonfile_uris_and_preserve_missing_utf16_source_locations()
 {
    let fixture = Fixture::new("fragment").await;
    let target_path = fixture.directory.path().join("Unsaved 😀.kt");
    let target = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(target_path.clone()),
        Some("kotlin".into()),
        "// unsaved 😀\r\nval target = 3\r\n",
    );
    fixture
        .editor
        .replace_sources([fixture.document.snapshot(), target.snapshot()]);
    let uri = file_uri(&target_path).unwrap();
    let range = json!({"start":{"line":1,"character":4},"end":{"line":1,"character":10}});
    fixture
        .configure(json!({"textDocument/definition":{"uri":uri.as_str(),"range":range}}))
        .await;
    let provider = fixture.provider();
    let request = fixture.request();
    let targets = tokio::task::spawn_blocking(move || provider.definitions(&request).unwrap())
        .await
        .unwrap();
    assert!(!target_path.exists());
    assert_eq!(targets[0].document, Some(target.id()));
    assert_eq!(
        targets[0].identifier_range.start.0,
        target.snapshot().text().find("target").unwrap()
    );
    assert_eq!(
        fixture.client.document_stamp(&uri).unwrap().document,
        target.id()
    );
    let missing = fixture.directory.path().join("missing.kt");
    fixture.configure(json!({"textDocument/definition":[{"uri":"untitled:unsupported","range":range},{"uri":file_uri(&missing).unwrap().as_str(),"range":range}]})).await;
    let provider = fixture.provider();
    let request = fixture.request();
    let targets = tokio::task::spawn_blocking(move || provider.definitions(&request).unwrap())
        .await
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert!(targets[0].missing_source);
    assert_eq!(targets[0].path, missing);
    assert_eq!(targets[0].location.unwrap().line.0, 1);
    assert_eq!(targets[0].location.unwrap().column.0, 4);
    fixture.service.shutdown().await.unwrap();
    let trace = fixture.trace();
    let opened = trace
        .iter()
        .position(|message| {
            message["method"] == "textDocument/didOpen"
                && message["params"]["textDocument"]["uri"] == uri.as_str()
        })
        .unwrap();
    let queried = trace
        .iter()
        .position(|message| message["method"] == "textDocument/definition")
        .unwrap();
    assert!(
        opened < queried,
        "other unsaved document must precede the query on the wire"
    );
}
#[tokio::test]
async fn lifecycle_tracks_rename_close_and_saved_bytes_without_reverting_a_newer_replica() {
    let mut fixture = Fixture::new("fragment").await;
    fixture
        .editor
        .synchronize(&CancellationToken::new())
        .await
        .unwrap();
    let saved = fixture.document.snapshot();
    fixture
        .document
        .apply_edit(
            Revision(0),
            TextRange::caret(ByteOffset(0)),
            "// unsaved\n",
            EditKind::Typing,
        )
        .unwrap();
    fixture
        .editor
        .replace_sources([fixture.document.snapshot()]);
    fixture
        .editor
        .synchronize(&CancellationToken::new())
        .await
        .unwrap();
    let uri = file_uri(&fixture.directory.path().join("source 😀.kt")).unwrap();
    let stamp = fixture.client.document_stamp(&uri).unwrap();
    let editor = fixture.editor.clone();
    tokio::task::spawn_blocking(move || editor.saved(saved))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.client.document_stamp(&uri), Some(stamp));
    fixture
        .document
        .retarget_file(fixture.directory.path().join("renamed.kt"))
        .unwrap();
    fixture
        .editor
        .replace_sources([fixture.document.snapshot()]);
    fixture
        .editor
        .synchronize(&CancellationToken::new())
        .await
        .unwrap();
    assert!(fixture.client.document_stamp(&uri).is_none());
    fixture.editor.replace_sources([]);
    fixture
        .editor
        .synchronize(&CancellationToken::new())
        .await
        .unwrap();
    assert!(fixture.client.document_origins().is_empty());
    fixture.service.shutdown().await.unwrap();
    let trace = fixture.trace();
    let save = trace
        .iter()
        .find(|message| message["method"] == "textDocument/didSave")
        .unwrap();
    assert_eq!(save["params"]["text"], "// 😀 café\r\nval café = 1\r\n");
    let methods: Vec<_> = trace
        .iter()
        .filter_map(|message| message["method"].as_str())
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "initialized",
            "textDocument/didOpen",
            "textDocument/didChange",
            "textDocument/didSave",
            "textDocument/didClose",
            "textDocument/didOpen",
            "textDocument/didClose",
            "shutdown",
            "exit"
        ]
    );
}
