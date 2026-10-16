pub mod projection;
use crate::{
    app::events::ResultGate,
    editor::{
        host::{
            EditorDocuments, EditorView, EditorWork, HostDescriptor, HostRole, QueuedEditorHost,
        },
        render::OverlaySpan,
    },
    focus::{FocusId, FocusState},
    input::terminal::*,
    theme::Theme,
    widgets::controls::draw_text,
};
use kode_core::{JobTag, RequestId, Result, WorkspaceGeneration};
use kode_git::diff::{DiffPlan, RestoreJob};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{Clock, OwnerId},
};
use projection::Projection;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use std::{path::PathBuf, sync::Arc};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutKey {
    pub width: u16,
    pub context: bool,
    pub unified: bool,
}
pub enum DiffWork {
    Layout {
        tag: JobTag,
        owner: OwnerId,
        plan: Arc<DiffPlan>,
        key: LayoutKey,
    },
    Restore {
        tag: JobTag,
        owner: OwnerId,
        job: RestoreJob,
        source_revision: Option<(kode_core::DocumentId, kode_core::Revision)>,
    },
    Editor {
        owner: OwnerId,
        work: EditorWork,
    },
}
pub struct DiffViewer {
    pub plan: Arc<DiffPlan>,
    pub editor: Option<EditorView<QueuedEditorHost>>,
    pub notice: String,
    pub restore_busy: Option<JobTag>,
    pub area: Rect,
    pub context: bool,
    pub unified: bool,
    pub source_revision: Option<(kode_core::DocumentId, kode_core::Revision)>,
    root: PathBuf,
    owner: OwnerId,
    gate: ResultGate,
    projection: Option<Projection>,
    layout: Option<LayoutKey>,
    pending: Option<(JobTag, LayoutKey)>,
    layout_failed: bool,
    work: Vec<DiffWork>,
    generation: WorkspaceGeneration,
    clipboard: Arc<ClipboardService>,
    clock: Arc<dyn Clock>,
}
impl DiffViewer {
    pub fn new(
        root: PathBuf,
        plan: Arc<DiffPlan>,
        generation: WorkspaceGeneration,
        clipboard: Arc<ClipboardService>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let mut result = Self {
            root,
            plan,
            generation,
            clipboard,
            clock,
            editor: None,
            notice: String::new(),
            restore_busy: None,
            area: Rect::default(),
            context: true,
            unified: false,
            source_revision: None,
            gate: ResultGate::new(generation),
            owner: OwnerId(RequestId::fresh()?.0),
            projection: None,
            layout: None,
            pending: None,
            layout_failed: false,
            work: vec![],
        };
        result.resize(Rect::default());
        Ok(result)
    }
    pub fn owner(&self) -> OwnerId {
        self.owner
    }
    pub fn replace_plan(&mut self, plan: Arc<DiffPlan>) {
        self.plan = plan;
        self.layout = None;
        self.pending = None;
        self.gate.dismiss("diff-layout");
        self.resize(self.area);
    }
    pub fn id(&self) -> FocusId {
        self.editor
            .as_ref()
            .map_or(FocusId(self.owner().0), |view| view.input.id)
    }
    pub fn body(&self) -> Rect {
        Rect::new(
            self.area.x,
            self.area.y + self.area.height.min(1),
            self.area.width,
            self.area.height.saturating_sub(2),
        )
    }
    pub fn resize(&mut self, area: Rect) {
        self.area = area;
        let key = LayoutKey {
            width: area.width,
            context: self.context,
            unified: self.unified,
        };
        if self.layout == Some(key) {
            self.pending = None;
            self.gate.dismiss("diff-layout");
            self.work
                .retain(|work| !matches!(work, DiffWork::Layout { .. }));
            return;
        }
        if self.pending.as_ref().is_some_and(|(_, old)| *old == key) {
            return;
        }
        let tag = self.gate.request("diff-layout".into(), None);
        self.pending = Some((tag, key));
        self.work
            .retain(|work| !matches!(work, DiffWork::Layout { .. }));
        self.work.push(DiffWork::Layout {
            tag,
            owner: self.owner(),
            plan: self.plan.clone(),
            key,
        });
    }
    pub fn complete_layout(
        &mut self,
        tag: JobTag,
        projection: Result<Projection>,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<()> {
        let Some((expected, key)) = self.pending else {
            return Ok(());
        };
        if tag != expected || !self.gate.accepts("diff-layout", tag) {
            return Ok(());
        }
        self.pending = None;
        let projection = match projection {
            Ok(projection) => projection,
            Err(error) => {
                self.notice = error.to_string();
                self.layout = Some(key);
                self.layout_failed = true;
                return Ok(());
            }
        };
        let previous_id = self.id();
        let focused = focus.focused == Some(previous_id);
        let mut row = 0;
        let mut selection = None;
        let mut top = None;
        if let Some(mut old) = self.editor.take() {
            row = old.input.state.scroll_rows;
            if let Some(previous) = &self.projection {
                let caret = old.input.state.caret.selection;
                selection = previous
                    .source_point(caret.anchor)
                    .zip(previous.source_point(caret.head));
                let snapshot = documents.document(old.document_id())?.snapshot();
                if let Ok(offset) = snapshot.line_start(kode_core::LineIndex(
                    row.min(snapshot.len_lines().saturating_sub(1)),
                )) {
                    top = previous.source_point(offset);
                }
            }
            old.detach(focus);
            let owner = old.owner();
            self.work.extend(
                old.take_work()
                    .into_iter()
                    .map(|work| DiffWork::Editor { owner, work }),
            );
            documents.remove(old.document_id())?;
        }
        let mut descriptor = HostDescriptor::new(
            HostRole::Historical,
            "git-diff".into(),
            format!("preview-{}", self.owner().0),
            "Git diff".into(),
            self.root.clone(),
        );
        descriptor.language = Some("diff".into());
        descriptor.mime = Some("text/x-diff".into());
        let document = documents.create_virtual(&descriptor, self.generation, &projection.text)?;
        let mut view = EditorView::new(
            document,
            documents,
            QueuedEditorHost::new(descriptor),
            self.generation,
            self.clipboard.clone(),
            self.clock.clone(),
        )?;
        view.chrome = false;
        if let Some((anchor, head)) = selection
            && let Some((anchor, head)) = projection
                .projected_offset(anchor)
                .zip(projection.projected_offset(head))
        {
            view.input.state.caret.set_selection(
                &documents.document(document)?.snapshot(),
                kode_core::Selection { anchor, head },
            )?;
        }
        if let Some(offset) = top.and_then(|point| projection.projected_offset(point)) {
            row = documents
                .document(document)?
                .snapshot()
                .byte_to_position(offset)?
                .line
                .0;
        }
        view.input.state.scroll_rows = row.min(projection.rows.len().saturating_sub(1));
        if focused {
            focus.focused = Some(view.input.id);
        }
        self.editor = Some(view);
        self.projection = Some(projection);
        self.layout = Some(key);
        self.layout_failed = false;
        Ok(())
    }
    pub fn take_work(&mut self) -> Vec<DiffWork> {
        if let Some(view) = &mut self.editor {
            let owner = view.owner();
            self.work.extend(
                view.take_work()
                    .into_iter()
                    .map(|work| DiffWork::Editor { owner, work }),
            );
        }
        std::mem::take(&mut self.work)
    }
    pub fn detach(
        &mut self,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<()> {
        self.pending = None;
        self.work
            .retain(|work| matches!(work, DiffWork::Restore { .. } | DiffWork::Editor { .. }));
        if let Some(view) = &mut self.editor {
            view.detach(focus);
            documents.remove(view.document_id())?;
        }
        Ok(())
    }
    fn restore(&mut self, hunk: usize) {
        if self.restore_busy.is_some() || self.pending.is_some() || self.layout_failed {
            return;
        }
        if !self.plan.restorable() {
            self.notice = "Restore is available only for unstaged text chunks".into();
            return;
        }
        if hunk >= self.plan.hunks().len() {
            return;
        }
        let tag = self.gate.request("diff-restore".into(), None);
        self.restore_busy = Some(tag);
        self.work.push(DiffWork::Restore {
            tag,
            owner: self.owner(),
            job: RestoreJob {
                root: self.root.clone(),
                plan: self.plan.clone(),
                hunk,
            },
            source_revision: self.source_revision,
        });
        self.notice = "Restoring captured chunk…".into();
    }
    pub fn restored(&mut self, tag: JobTag, message: String) {
        if self.restore_busy == Some(tag) {
            self.restore_busy = None;
            self.notice = message;
        }
    }
    pub fn dispatch(
        &mut self,
        event: &InputEvent,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<bool> {
        if let InputEvent::Key(raw) = event
            && let Some(input) =
                crate::input::shortcuts::command_key(crate::input::shortcuts::Scope::GitDiff, *raw)
            && focus.focused == Some(self.id())
            && !input.modifiers.ctrl
            && !input.modifiers.alt
            && !input.modifiers.meta
            && self
                .editor
                .as_ref()
                .is_none_or(|view| !view.input.state.search_has_focus)
        {
            if matches!(input.key, Key::Character('c' | 'C' | 'v' | 'V')) {
                if matches!(input.key, Key::Character('c' | 'C')) {
                    self.context = !self.context;
                } else {
                    self.unified = !self.unified;
                }
                self.resize(self.area);
                return Ok(true);
            }
            if matches!(input.key, Key::Character('r' | 'R') | Key::Enter) && !input.repeat {
                if let Some(view) = &self.editor {
                    let row = documents
                        .document(view.document_id())?
                        .snapshot()
                        .byte_to_position(view.input.state.caret.cursor())?
                        .line
                        .0;
                    if let Some(hunk) = self
                        .projection
                        .as_ref()
                        .and_then(|projection| projection.rows.get(row))
                        .and_then(|row| row.hunk)
                    {
                        self.restore(hunk);
                    }
                }
                return Ok(true);
            }
        }
        if let InputEvent::Pointer(pointer) = event
            && pointer.action == PointerAction::Down(crossterm::event::MouseButton::Left)
            && self.pending.is_none()
        {
            let footer = Rect::new(
                self.area.x,
                self.area.bottom().saturating_sub(1),
                self.area.width,
                u16::from(self.area.height > 1),
            );
            if self.notice.is_empty() && footer.contains(Position::new(pointer.x, pointer.y)) {
                let x = pointer.x - self.area.x;
                if x < 11 {
                    self.context = !self.context;
                    self.resize(self.area);
                } else if (14..22).contains(&x) {
                    self.unified = !self.unified;
                    self.resize(self.area);
                }
                return Ok(true);
            }
            let body = self.body();
            if let Some(view) = &self.editor {
                let geometry = view.geometry(documents, body)?;
                if geometry.text.contains(Position::new(pointer.x, pointer.y)) {
                    let row =
                        view.input.state.scroll_rows + usize::from(pointer.y - geometry.text.y);
                    let hit = self
                        .projection
                        .as_ref()
                        .and_then(|projection| projection.rows.get(row));
                    if let Some(row) = hit
                        && row.marker.as_ref().is_some_and(|range| {
                            range.contains(&usize::from(pointer.x - geometry.text.x))
                        })
                        && let Some(hunk) = row.hunk
                    {
                        self.restore(hunk);
                        return Ok(true);
                    }
                }
            }
        }
        let body = self.body();
        if let Some(view) = &mut self.editor {
            return match view.dispatch(event, documents, focus, body) {
                Err(
                    error @ (kode_core::DomainError::ReadOnly
                    | kode_core::DomainError::NoSaveTarget),
                ) => {
                    self.notice = error.to_string();
                    Ok(true)
                }
                result => result,
            };
        }
        Ok(false)
    }
    pub fn render(
        &mut self,
        area: Rect,
        documents: &EditorDocuments,
        focus: &FocusState,
        buffer: &mut Buffer,
        theme: &Theme,
    ) -> Result<Option<Position>> {
        self.resize(area);
        let comparison = if self.plan.contents().revision.is_some() {
            "historical"
        } else if self.plan.contents().staged {
            "staged"
        } else {
            "unstaged"
        };
        let label = format!(
            "Diff {} ({comparison}) [c] {} [v] {} | {}",
            self.plan.contents().path.display(),
            if self.context { "context" } else { "changes" },
            if self.unified {
                "unified"
            } else {
                "side-by-side"
            },
            if self.pending.is_some() {
                "preparing…"
            } else if self.notice.is_empty() && self.plan.restorable() {
                "Enter/r restore chunk"
            } else {
                &self.notice
            }
        );
        draw_text(
            Rect::new(area.x, area.y, area.width, area.height.min(1)),
            buffer,
            &crate::editor::display::display_text(&label),
            theme.style("diff-header"),
        );
        if area.height > 1 {
            let footer = if self.notice.is_empty() {
                if self.plan.restorable() {
                    "[c] context | [v] view | Enter/r restore selected chunk"
                } else {
                    "[c] context | [v] view | Read-only comparison"
                }
            } else {
                &self.notice
            };
            draw_text(
                Rect::new(area.x, area.bottom() - 1, area.width, 1),
                buffer,
                &crate::editor::display::display_text(footer),
                theme.style("diff-header"),
            );
        }
        let body = self.body();
        if let Some(view) = &mut self.editor {
            let overlays = self
                .projection
                .as_ref()
                .map_or_else(Vec::new, |projection| {
                    projection
                        .paint
                        .iter()
                        .map(|paint| OverlaySpan {
                            range: paint.range,
                            priority: if paint.class == "diff-gutter" { 30 } else { 20 },
                            style: theme.style(paint.class),
                        })
                        .collect()
                });
            return view.render(documents, focus, body, buffer, theme, &overlays);
        }
        Ok(None)
    }
}
