use crossterm::event::MouseButton;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, JobTag, RequestId, Result, TextRange,
    WorkspaceGeneration,
    commands::{EditorCommand, Movement},
    document::Document,
    editor::search::scan,
    services::{Clipboard, DocumentIo},
};
use kode_platform::{
    clipboard::{ClipboardService, NativeClipboard},
    files::NativeDocumentIo,
    jobs::{Clock, Notifications, Scheduler},
};
use kode_tui::{
    editor::{
        host::{
            ClipboardJob, ClipboardResult, EditorDocuments, EditorView, EditorWork,
            ExecutionAction, HostDescriptor, HostEvent, HostRole, QueuedEditorHost, SaveTarget,
            TransientHighlight, schedule_clipboard,
        },
        loading::{prepare, schedule_save},
        popups::completion::{HostCompletionProvider, HostSuggestion, prepare as completions},
    },
    focus::FocusState,
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct ClockZero;
impl Clock for ClockZero {
    fn now_millis(&self) -> u64 {
        0
    }
}
struct OfflineClipboard(Arc<AtomicUsize>);
impl NativeClipboard for OfflineClipboard {
    fn get_text(&mut self) -> std::result::Result<String, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("headless test".into())
    }
    fn set_text(&mut self, _: &str) -> std::result::Result<(), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("headless test".into())
    }
}
fn clipboard() -> (Arc<ClipboardService>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    (
        Arc::new(ClipboardService::new(Box::new(OfflineClipboard(
            calls.clone(),
        )))),
        calls,
    )
}
fn descriptor(role: HostRole, key: &str) -> HostDescriptor {
    let mut descriptor = HostDescriptor::new(
        role,
        "test".into(),
        key.into(),
        format!("<{key}>"),
        PathBuf::from("/project"),
    );
    descriptor.language = Some(if role == HostRole::Sql { "sql" } else { "text" }.into());
    descriptor
}
fn virtual_view(
    role: HostRole,
    key: &str,
    text: &str,
    documents: &mut EditorDocuments,
    clipboard: Arc<ClipboardService>,
) -> EditorView<QueuedEditorHost> {
    let descriptor = descriptor(role, key);
    let id = if matches!(role, HostRole::File | HostRole::ProjectSearch) {
        documents
            .insert(
                Document::new(
                    DocumentId::fresh().unwrap(),
                    DocumentOrigin::File(PathBuf::from("/project/multi-test.txt")),
                    None,
                    text,
                ),
                &descriptor,
                WorkspaceGeneration(1),
                None,
            )
            .unwrap()
    } else {
        documents
            .create_virtual(&descriptor, WorkspaceGeneration(1), text)
            .unwrap()
    };
    EditorView::new(
        id,
        documents,
        QueuedEditorHost::new(descriptor),
        WorkspaceGeneration(1),
        clipboard,
        Arc::new(ClockZero),
    )
    .unwrap()
}
fn key(key: Key, modifiers: Modifiers) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers,
        repeat: false,
    })
}
fn plain(value: Key) -> InputEvent {
    key(value, Modifiers::default())
}
fn ctrl(ch: char) -> InputEvent {
    key(
        Key::Character(ch),
        Modifiers {
            ctrl: true,
            ..Default::default()
        },
    )
}
fn execute() -> InputEvent {
    key(
        Key::Enter,
        Modifiers {
            ctrl: true,
            ..Default::default()
        },
    )
}
fn focus(view: &EditorView<QueuedEditorHost>) -> FocusState {
    FocusState {
        focused: Some(view.input.id),
        ..Default::default()
    }
}
fn text(documents: &EditorDocuments, id: DocumentId) -> String {
    documents
        .document(id)
        .unwrap()
        .snapshot()
        .to_service_snapshot()
        .text
        .to_string()
}
const AREA: Rect = Rect::new(15, 7, 80, 12);

#[test]
fn writable_sql_rest_and_environment_hosts_share_edit_history_language_and_callbacks() {
    let (clipboard, _) = clipboard();
    let mut documents = EditorDocuments::default();
    for role in [
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
    ] {
        let mut view = virtual_view(
            role,
            &format!("{role:?}"),
            "",
            &mut documents,
            clipboard.clone(),
        );
        let mut focus = focus(&view);
        view.dispatch(
            &plain(Key::Character('A')),
            &mut documents,
            &mut focus,
            AREA,
        )
        .unwrap();
        view.dispatch(
            &InputEvent::Paste("\t😀\n".into()),
            &mut documents,
            &mut focus,
            AREA,
        )
        .unwrap();
        assert_eq!(text(&documents, view.document_id()), "A\t😀\n");
        assert!(documents.document(view.document_id()).unwrap().is_dirty());
        assert_eq!(view.host.take_events().len(), 1); // latest whole snapshot coalesces successive change callbacks
        view.command(EditorCommand::Undo, &mut documents, AREA)
            .unwrap();
        assert_eq!(text(&documents, view.document_id()), "A");
        assert_eq!(
            documents
                .document(view.document_id())
                .unwrap()
                .snapshot()
                .language
                .as_deref(),
            Some(if role == HostRole::Sql { "sql" } else { "text" })
        );
        let mut buffer = Buffer::empty(AREA);
        view.render(
            &documents,
            &focus,
            AREA,
            &mut buffer,
            &Theme::compiled(),
            &[],
        )
        .unwrap();
    }
}

struct Suggestion;
impl HostCompletionProvider for Suggestion {
    fn suggestions(
        &self,
        _: &kode_core::intelligence::IntelligenceRequest,
        _: &str,
    ) -> Result<Vec<HostSuggestion>> {
        Ok(vec![HostSuggestion {
            label: "change".into(),
            insert_text: Some("MUTATE".into()),
            detail: None,
        }])
    }
}
#[test]
fn response_raw_and_historical_documents_reject_every_edit_path_even_with_false_host_flag() {
    let (clipboard, _) = clipboard();
    let mut documents = EditorDocuments::default();
    for role in [
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let mut view = virtual_view(
            role,
            &format!("{role:?}"),
            "response",
            &mut documents,
            clipboard.clone(),
        );
        view.host.descriptor.read_only = false;
        let mut focus = focus(&view);
        view.command(EditorCommand::SelectAll, &mut documents, AREA)
            .unwrap();
        for event in [
            plain(Key::Character('X')),
            plain(Key::Enter),
            plain(Key::Tab),
            plain(Key::Backspace),
            plain(Key::Delete),
            InputEvent::Paste("changed".into()),
            ctrl('x'),
            ctrl('v'),
            key(
                Key::Character('u'),
                Modifiers {
                    alt: true,
                    ..Default::default()
                },
            ),
        ] {
            assert!(
                matches!(
                    view.dispatch(&event, &mut documents, &mut focus, AREA),
                    Err(DomainError::ReadOnly)
                ),
                "{role:?}"
            );
        }
        assert!(matches!(
            view.programmatic_replace(
                TextRange {
                    start: ByteOffset(0),
                    end: ByteOffset(8)
                },
                "changed",
                &mut documents,
                AREA
            ),
            Err(DomainError::ReadOnly)
        ));
        view.dispatch(&ctrl('f'), &mut documents, &mut focus, AREA)
            .unwrap();
        let scan_work = view
            .take_work()
            .into_iter()
            .find_map(|work| match work {
                EditorWork::Search {
                    tag,
                    snapshot,
                    query,
                } => Some((tag, snapshot, query)),
                _ => None,
            })
            .unwrap();
        view.complete_search(
            scan_work.0,
            scan(&scan_work.1, &scan_work.2, || false),
            &mut documents,
            AREA,
        )
        .unwrap();
        assert!(matches!(
            view.dispatch(&ctrl('r'), &mut documents, &mut focus, AREA),
            Err(DomainError::ReadOnly)
        ));
        view.dispatch(&plain(Key::Escape), &mut documents, &mut focus, AREA)
            .unwrap();
        let job = view
            .completion
            .begin(
                &documents.document(view.document_id()).unwrap().snapshot(),
                &view.input.state,
            )
            .unwrap();
        let entries = completions(&job, Some(&Suggestion), None, || false).unwrap();
        view.complete_completion(job.tag, Ok(entries), &documents, &focus)
            .unwrap();
        assert!(matches!(
            view.dispatch(&plain(Key::Enter), &mut documents, &mut focus, AREA),
            Err(DomainError::ReadOnly)
        ));
        view.completion.dismiss();
        view.dispatch(&ctrl('c'), &mut documents, &mut focus, AREA)
            .unwrap();
        assert_eq!(clipboard.local_text(), "response");
        assert_eq!(text(&documents, view.document_id()), "response");
        assert!(!documents.document(view.document_id()).unwrap().is_dirty());
    }
}

#[test]
fn shared_clipboard_nested_coordinates_modifiers_and_result_focus_stay_with_editor_owner() {
    let (clipboard, calls) = clipboard();
    let mut documents = EditorDocuments::default();
    let mut headers = virtual_view(
        HostRole::RestHeaders,
        "headers",
        "Content-Type: application/json\n",
        &mut documents,
        clipboard.clone(),
    );
    let mut response = virtual_view(
        HostRole::Response,
        "response",
        "immutable result",
        &mut documents,
        clipboard.clone(),
    );
    let mut sql = virtual_view(
        HostRole::Sql,
        "query",
        "select ",
        &mut documents,
        clipboard.clone(),
    );
    let mut focus = focus(&headers);
    headers
        .command(EditorCommand::SelectAll, &mut documents, AREA)
        .unwrap();
    headers
        .dispatch(
            &key(
                Key::Character('c'),
                Modifiers {
                    alt: true,
                    ..Default::default()
                },
            ),
            &mut documents,
            &mut focus,
            AREA,
        )
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    focus.focused = Some(sql.input.id);
    sql.command(
        EditorCommand::Move {
            movement: Movement::DocumentEnd,
            extend: false,
        },
        &mut documents,
        AREA,
    )
    .unwrap();
    sql.dispatch(&ctrl('v'), &mut documents, &mut focus, AREA)
        .unwrap();
    let ticket = sql
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::ClipboardRead(ticket) => Some(ticket),
            _ => None,
        })
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    sql.complete_clipboard(
        &ticket,
        Ok(clipboard.local_text()),
        &mut documents,
        &focus,
        AREA,
    )
    .unwrap();
    assert_eq!(
        text(&documents, sql.document_id()),
        "select Content-Type: application/json\n"
    );
    focus.focused = Some(headers.input.id);
    let before = headers.input.state.caret.cursor();
    let event = key(
        Key::Left,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    assert!(
        !response
            .dispatch(
                &event,
                &mut documents,
                &mut focus,
                Rect::new(15, 20, 80, 12)
            )
            .unwrap()
    );
    headers
        .dispatch(&event, &mut documents, &mut focus, AREA)
        .unwrap();
    assert!(headers.input.state.caret.cursor() < before);
    let geometry = headers.geometry(&documents, AREA).unwrap();
    let modifiers = Modifiers {
        ctrl: true,
        alt: true,
        shift: true,
        meta: true,
    };
    headers
        .dispatch(
            &InputEvent::Pointer(PointerInput {
                x: geometry.text.x + 2,
                y: geometry.text.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers,
            }),
            &mut documents,
            &mut focus,
            AREA,
        )
        .unwrap();
    assert_eq!(headers.input.state.caret.cursor(), ByteOffset(2));
}

#[test]
fn focused_search_ctrl_enter_takes_priority_before_sql_or_rest_host_execution() {
    let (clipboard, _) = clipboard();
    let mut documents = EditorDocuments::default();
    for role in [HostRole::Sql, HostRole::RestBody, HostRole::RestHeaders] {
        let mut view = virtual_view(
            role,
            &format!("{role:?}"),
            "text",
            &mut documents,
            clipboard.clone(),
        );
        let mut focus = focus(&view);
        view.dispatch(&ctrl('f'), &mut documents, &mut focus, AREA)
            .unwrap();
        view.take_work();
        assert!(
            view.dispatch(&execute(), &mut documents, &mut focus, AREA)
                .unwrap()
        );
        assert!(
            !view
                .take_work()
                .iter()
                .any(|work| matches!(work, EditorWork::Execute(_)))
        );
        view.dispatch(&plain(Key::Escape), &mut documents, &mut focus, AREA)
            .unwrap();
        view.dispatch(&execute(), &mut documents, &mut focus, AREA)
            .unwrap();
        let work = view.take_work();
        assert!(work.iter().any(|work|matches!(work,EditorWork::Execute(execution) if execution.action == if role == HostRole::Sql { ExecutionAction::SqlSelectionOrStatement } else { ExecutionAction::SendRequest })));
        assert!(
            view.host
                .take_events()
                .iter()
                .any(|event| matches!(event, HostEvent::Execute(_)))
        );
    }
}

#[test]
fn virtual_save_uses_owner_callback_not_label_and_older_save_keeps_newer_edits_dirty() {
    let (clipboard, _) = clipboard();
    let mut documents = EditorDocuments::default();
    let directory = tempfile::tempdir().unwrap();
    let accidental = directory.path().join("label-is-not-a-file.sql");
    let mut descriptor = descriptor(HostRole::Sql, "query");
    descriptor.label = accidental.display().to_string();
    descriptor.save_target = SaveTarget::Owner("console:1".into());
    let id = documents
        .create_virtual(&descriptor, WorkspaceGeneration(1), "select 1;")
        .unwrap();
    let mut view = EditorView::new(
        id,
        &documents,
        QueuedEditorHost::new(descriptor),
        WorkspaceGeneration(1),
        clipboard,
        Arc::new(ClockZero),
    )
    .unwrap();
    view.command(EditorCommand::InsertText("A".into()), &mut documents, AREA)
        .unwrap();
    view.command(EditorCommand::Save, &mut documents, AREA)
        .unwrap();
    let first = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::SaveOwner(job) => Some(job),
            _ => None,
        })
        .unwrap();
    view.command(EditorCommand::InsertText("B".into()), &mut documents, AREA)
        .unwrap();
    view.command(EditorCommand::Save, &mut documents, AREA)
        .unwrap();
    assert!(
        !view
            .take_work()
            .iter()
            .any(|work| matches!(work, EditorWork::SaveOwner(_)))
    );
    let mut focus = focus(&view);
    view.detach(&mut focus);
    assert!(documents.remove(id).is_err());
    let next = documents
        .complete_owner_save(id, first.tag, Ok(()))
        .unwrap()
        .unwrap();
    assert!(documents.document(id).unwrap().is_dirty());
    let EditorWork::SaveOwner(next) = next else {
        panic!("owner save")
    };
    assert_eq!(next.key, "console:1");
    documents.complete_owner_save(id, next.tag, Ok(())).unwrap();
    assert!(!documents.document(id).unwrap().is_dirty());
    assert!(!accidental.exists());
    let mut wrong = self::descriptor(HostRole::RestBody, "bad");
    wrong.save_target = SaveTarget::File;
    assert!(matches!(
        documents.create_virtual(&wrong, WorkspaceGeneration(1), "body"),
        Err(DomainError::NoSaveTarget)
    ));
}

#[test]
fn independent_views_share_document_changes_and_document_owned_save_lifetime() {
    let (clipboard, _) = clipboard();
    let mut documents = EditorDocuments::default();
    let descriptor = descriptor(HostRole::File, "file");
    let doc = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(PathBuf::from("/project/file.txt")),
        None,
        "long text",
    );
    let id = documents
        .insert(doc, &descriptor, WorkspaceGeneration(1), None)
        .unwrap();
    let mut first = EditorView::new(
        id,
        &documents,
        QueuedEditorHost::new(descriptor.clone()),
        WorkspaceGeneration(1),
        clipboard.clone(),
        Arc::new(ClockZero),
    )
    .unwrap();
    let mut second = EditorView::new(
        id,
        &documents,
        QueuedEditorHost::new(descriptor),
        WorkspaceGeneration(1),
        clipboard,
        Arc::new(ClockZero),
    )
    .unwrap();
    assert_ne!(first.input.id, second.input.id);
    assert_ne!(first.owner(), documents.save_owner(id).unwrap());
    second
        .command(
            EditorCommand::Move {
                movement: Movement::DocumentEnd,
                extend: false,
            },
            &mut documents,
            AREA,
        )
        .unwrap();
    first
        .programmatic_replace(
            TextRange {
                start: ByteOffset(0),
                end: ByteOffset(9),
            },
            "😀",
            &mut documents,
            AREA,
        )
        .unwrap();
    second.synchronize(&documents).unwrap();
    assert_eq!(second.input.state.caret.cursor(), ByteOffset(4));
    assert_eq!(text(&documents, id), "😀");
    let mut buffer = Buffer::empty(AREA);
    second
        .render(
            &documents,
            &focus(&second),
            AREA,
            &mut buffer,
            &Theme::compiled(),
            &[],
        )
        .unwrap();
    let mut focus = focus(&first);
    first.detach(&mut focus);
    assert!(
        !first
            .dispatch(
                &plain(Key::Character('X')),
                &mut documents,
                &mut focus,
                AREA
            )
            .unwrap()
    );
    assert!(documents.document(id).is_ok());
}

#[test]
fn warnings_transient_highlight_safe_header_and_tiny_hosts_render_without_io() {
    let (clipboard, calls) = clipboard();
    let mut documents = EditorDocuments::default();
    let mut view = virtual_view(
        HostRole::RestHeaders,
        "headers",
        "A: 1\nA: 2",
        &mut documents,
        clipboard.clone(),
    );
    view.host.warnings = vec![kode_core::LineIndex(1)];
    view.host.transient = Some(TransientHighlight {
        revision: documents.document(view.document_id()).unwrap().revision(),
        range: TextRange {
            start: ByteOffset(0),
            end: ByteOffset(1),
        },
    });
    for (width, height) in [(0, 0), (1, 1), (20, 6), (80, 24)] {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        view.render(
            &documents,
            &focus(&view),
            area,
            &mut buffer,
            &Theme::compiled(),
            &[],
        )
        .unwrap();
    }
    let mut buffer = Buffer::empty(AREA);
    view.render(
        &documents,
        &FocusState::default(),
        AREA,
        &mut buffer,
        &Theme::compiled(),
        &[],
    )
    .unwrap();
    let geometry = view.geometry(&documents, AREA).unwrap();
    assert_eq!(
        buffer[(geometry.text.x, geometry.text.y)].bg,
        ratatui::style::Color::Rgb(216, 230, 255)
    );
    assert_eq!(
        buffer[(geometry.text.x, geometry.text.y + 1)].bg,
        ratatui::style::Color::Rgb(216, 230, 255)
    );
    let mut hostile = descriptor(HostRole::RestBody, "safe-label");
    hostile.label = "\x1B[2J中".into();
    let id = documents
        .create_virtual(&hostile, WorkspaceGeneration(1), "text")
        .unwrap();
    let mut safe = EditorView::new(
        id,
        &documents,
        QueuedEditorHost::new(hostile),
        WorkspaceGeneration(1),
        clipboard,
        Arc::new(ClockZero),
    )
    .unwrap();
    let mut buffer = Buffer::empty(AREA);
    safe.render(
        &documents,
        &FocusState::default(),
        AREA,
        &mut buffer,
        &Theme::compiled(),
        &[],
    )
    .unwrap();
    let header: String = (AREA.x..AREA.right())
        .map(|x| buffer[(x, AREA.y)].symbol())
        .collect();
    assert!(header.contains("\\x1B[2J\\x4E2D"));
    assert!(!header.contains('\x1B'));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn real_file_host_save_after_detach_and_native_clipboard_jobs_use_workers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("file.txt");
    std::fs::write(&path, b"base").unwrap();
    let io = Arc::new(NativeDocumentIo);
    let tag = JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: None,
    };
    let prepared = prepare(
        io.load(&DocumentOrigin::File(path.clone()), tag).unwrap(),
        DocumentId::fresh().unwrap(),
        None,
    )
    .unwrap();
    let mut documents = EditorDocuments::default();
    let descriptor = descriptor(HostRole::ProjectSearch, "preview");
    let id = documents
        .insert_file(prepared, &descriptor, WorkspaceGeneration(1))
        .unwrap();
    let (clipboard, calls) = clipboard();
    let mut view = EditorView::new(
        id,
        &documents,
        QueuedEditorHost::new(descriptor),
        WorkspaceGeneration(1),
        clipboard.clone(),
        Arc::new(ClockZero),
    )
    .unwrap();
    view.command(
        EditorCommand::InsertText("edited ".into()),
        &mut documents,
        AREA,
    )
    .unwrap();
    view.command(EditorCommand::Save, &mut documents, AREA)
        .unwrap();
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::SaveFile(job) => Some(job),
            _ => None,
        })
        .unwrap();
    let mut saves = Scheduler::new(Notifications::new(Arc::new(ClockZero)));
    schedule_save(&mut saves, documents.save_owner(id).unwrap(), job, io).unwrap();
    view.detach(&mut focus(&view));
    let result = tokio::time::timeout(Duration::from_secs(2), saves.next())
        .await
        .unwrap()
        .unwrap();
    documents
        .complete_file_save(id, result.tag, result.result)
        .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"edited base");
    assert!(!documents.document(id).unwrap().is_dirty());
    assert!(saves.shutdown().await.is_empty());
    let epoch = clipboard.remember("raw\t😀").unwrap();
    let mut clips = Scheduler::new(Notifications::new(Arc::new(ClockZero)));
    schedule_clipboard(
        &mut clips,
        view.owner(),
        tag,
        ClipboardJob::Publish(epoch),
        clipboard.clone(),
    )
    .unwrap();
    assert!(matches!(
        clips.next().await.unwrap().result.unwrap(),
        ClipboardResult::Published { native: false, .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    schedule_clipboard(&mut clips, view.owner(), tag, ClipboardJob::Read, clipboard).unwrap();
    assert!(
        matches!(clips.next().await.unwrap().result.unwrap(),ClipboardResult::Read(text) if text == "raw\t😀")
    );
    assert!(clips.shutdown().await.is_empty());
}

#[test]
fn multiple_cursors_edit_undo_and_preserve_execution_primary_in_all_shared_hosts() {
    let (clipboard, _) = clipboard();
    let mut documents = EditorDocuments::default();
    for role in [
        HostRole::File,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
    ] {
        let mut view = virtual_view(
            role,
            &format!("multi-{role:?}"),
            "ab\ncd",
            &mut documents,
            clipboard.clone(),
        );
        let mut focus = focus(&view);
        view.command(
            EditorCommand::MoveTo {
                position: ByteOffset(1),
                extend: false,
            },
            &mut documents,
            AREA,
        )
        .unwrap();
        view.dispatch(
            &key(
                Key::Down,
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                },
            ),
            &mut documents,
            &mut focus,
            AREA,
        )
        .unwrap();
        assert_eq!(view.input.state.caret.secondary.len(), 1);
        let before = view.input.state.caret.clone();
        view.dispatch(
            &InputEvent::Paste("Ω".into()),
            &mut documents,
            &mut focus,
            AREA,
        )
        .unwrap();
        assert_eq!(text(&documents, view.document_id()), "aΩb\ncΩd");
        assert_eq!(
            documents
                .document(view.document_id())
                .unwrap()
                .history_depth(),
            (1, 0)
        );
        assert!(view.host.take_events().iter().any(|event| matches!(event, HostEvent::Changed(change) if change.deltas.as_ref().is_some_and(|deltas| deltas.len() == 2))));
        if role == HostRole::Sql || role == HostRole::RestBody || role == HostRole::RestHeaders {
            view.dispatch(&execute(), &mut documents, &mut focus, AREA)
                .unwrap();
            assert!(view.host.take_events().iter().any(|event| matches!(event, HostEvent::Execute(execution) if execution.selection == view.input.state.caret.selection)));
        }
        view.command(EditorCommand::Undo, &mut documents, AREA)
            .unwrap();
        assert_eq!(view.input.state.caret, before);
        assert_eq!(text(&documents, view.document_id()), "ab\ncd");
        view.command(EditorCommand::Redo, &mut documents, AREA)
            .unwrap();
        assert_eq!(text(&documents, view.document_id()), "aΩb\ncΩd");
    }
}

#[test]
fn readonly_hosts_allow_multiple_selection_and_copy_but_reject_all_multi_mutations() {
    let (clipboard, _) = clipboard();
    let mut documents = EditorDocuments::default();
    for role in [
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let mut view = virtual_view(
            role,
            &format!("multi-ro-{role:?}"),
            "ab\ncd",
            &mut documents,
            clipboard.clone(),
        );
        let mut focus = focus(&view);
        view.dispatch(
            &key(
                Key::Down,
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                },
            ),
            &mut documents,
            &mut focus,
            AREA,
        )
        .unwrap();
        assert_eq!(view.input.state.caret.secondary.len(), 1);
        let before = view.input.state.caret.clone();
        for command in [
            EditorCommand::InsertText("X".into()),
            EditorCommand::PasteText("X".into()),
            EditorCommand::InsertNewline,
            EditorCommand::InsertTab,
            EditorCommand::DeleteBackward,
            EditorCommand::DeleteForward,
            EditorCommand::Cut,
            EditorCommand::Paste,
            EditorCommand::Undo,
            EditorCommand::Redo,
        ] {
            assert!(matches!(
                view.command(command, &mut documents, AREA),
                Err(DomainError::ReadOnly)
            ));
            assert_eq!(view.input.state.caret, before);
            assert_eq!(text(&documents, view.document_id()), "ab\ncd");
        }
        view.dispatch(
            &key(
                Key::Right,
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &mut documents,
            &mut focus,
            AREA,
        )
        .unwrap();
        view.command(EditorCommand::Copy, &mut documents, AREA)
            .unwrap();
        assert_eq!(ClipboardService::read(&clipboard).unwrap(), "a\nc");
    }
}

#[test]
fn word_double_click_and_alt_double_click_share_editing_and_readonly_guards_in_every_host() {
    let (clipboard, _) = clipboard();
    let mut documents = EditorDocuments::default();
    for role in [
        HostRole::File,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let mut view = virtual_view(
            role,
            &format!("word-{role:?}"),
            "éclair tailword",
            &mut documents,
            clipboard.clone(),
        );
        let mut focus = focus(&view);
        for (byte, alt) in [(3, false), (10, true)] {
            let geometry = view.geometry(&documents, AREA).unwrap();
            let folds = [];
            let layout = view
                .input
                .layout(
                    documents.document(view.document_id()).unwrap(),
                    kode_tui::editor::input::InputViewport {
                        geometry,
                        folds: &folds,
                    },
                )
                .unwrap();
            let (row, column) = layout.position_to_cell(ByteOffset(byte)).unwrap();
            for _ in 0..2 {
                for action in [
                    PointerAction::Down(MouseButton::Left),
                    PointerAction::Up(MouseButton::Left),
                ] {
                    view.dispatch(
                        &InputEvent::Pointer(PointerInput {
                            x: geometry.text.x + column as u16,
                            y: geometry.text.y + row as u16,
                            action,
                            modifiers: Modifiers {
                                alt,
                                ..Default::default()
                            },
                        }),
                        &mut documents,
                        &mut focus,
                        AREA,
                    )
                    .unwrap();
                }
            }
            let (tag, job) = view
                .take_work()
                .into_iter()
                .find_map(|work| match work {
                    EditorWork::Word { tag, job } => Some((tag, job)),
                    _ => None,
                })
                .expect("word worker");
            let result = job.run(&kode_platform::jobs::CancellationToken::new());
            assert!(
                view.complete_word(tag, result, &documents, &focus, AREA)
                    .unwrap()
            );
        }
        assert_eq!(view.input.state.caret.selections().count(), 2);
        assert_eq!(
            view.input
                .state
                .caret
                .selected_text(&documents.document(view.document_id()).unwrap().snapshot())
                .unwrap(),
            "tailword"
        );
        let before = view.input.state.caret.clone();
        view.command(EditorCommand::Copy, &mut documents, AREA)
            .unwrap();
        assert_eq!(clipboard.read().unwrap(), "éclair\ntailword");
        let result = view.dispatch(
            &InputEvent::Paste("X".into()),
            &mut documents,
            &mut focus,
            AREA,
        );
        if documents
            .document(view.document_id())
            .unwrap()
            .is_read_only()
        {
            assert!(matches!(result, Err(DomainError::ReadOnly)));
            assert_eq!(text(&documents, view.document_id()), "éclair tailword");
        } else {
            result.unwrap();
            assert_eq!(text(&documents, view.document_id()), "X X");
            view.command(EditorCommand::Undo, &mut documents, AREA)
                .unwrap();
            assert_eq!(view.input.state.caret, before);
            assert_eq!(text(&documents, view.document_id()), "éclair tailword");
        }
    }
}
