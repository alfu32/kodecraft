//! Modal search with a registry-backed editor; worker commands remain owned.
use crate::{
    editor::{
        display::{Glyph, display_text},
        host::{
            ClipboardJob, ClipboardResult, EditorDocuments, EditorView, EditorWork,
            QueuedEditorHost,
        },
        render::OverlaySpan,
        search_bar::escape_literal,
    },
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    layout::{Axis, Panes, Splitter, centered, split},
    theme::Theme,
    widgets::{
        controls::draw_text,
        field::{FieldAction, TextField},
    },
};
use crossterm::event::MouseButton;
use kode_core::{
    DocumentOrigin, DomainError, JobTag, RequestId, Result, Selection, commands::EditorCommand,
};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{Clock, OwnerId},
};
use kode_workspace::{
    mime::MimeDetector,
    project::Project,
    search::{ProjectSearchHit, ProjectSearchJob, ProjectSearchReport, SEARCH_SCOPE, SearchStyler},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
    widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap},
};
use std::{path::Path, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFocus {
    Query,
    Replacement,
    Filter,
    List,
    Editor,
}
pub enum ProjectSearchWork {
    Search(ProjectSearchJob),
    OpenHit { tag: JobTag, hit: ProjectSearchHit },
    Editor { owner: OwnerId, work: EditorWork },
    Clipboard { tag: JobTag, job: ClipboardJob },
    Dismiss,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct SearchGeometry {
    pub modal: Rect,
    pub fields: [Rect; 3],
    pub status: Rect,
    pub body: Rect,
    pub list: Rect,
    pub divider: Rect,
    pub editor: Rect,
}
struct PasteStamp {
    tag: JobTag,
    field: SearchFocus,
    value: String,
    cursor: usize,
    anchor: Option<usize>,
}
pub struct ProjectSearchDialog {
    pub id: FocusId,
    pub owner: OwnerId,
    pub project: Project,
    pub query: TextField,
    pub replacement: TextField,
    pub filter: TextField,
    pub focused: SearchFocus,
    pub editor: Option<EditorView<QueuedEditorHost>>,
    pub hits: Vec<ProjectSearchHit>,
    pub selected: Option<usize>,
    pub scroll: usize,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub finished: bool,
    pub geometry: SearchGeometry,
    pub splitter: Splitter,
    pub styler: Option<Arc<dyn SearchStyler>>,
    previous_focus: Option<FocusId>,
    detector: MimeDetector,
    clipboard: Arc<ClipboardService>,
    clock: Arc<dyn Clock>,
    pending_search: Option<JobTag>,
    pending_open: Option<JobTag>,
    pending_paste: Option<PasteStamp>,
    highlight: Option<(
        kode_core::DocumentId,
        kode_core::Revision,
        kode_core::TextRange,
    )>,
    observed_revision: Option<kode_core::Revision>,
    changed_at: Option<u64>,
    center_preview: bool,
    help: bool,
    work: Vec<ProjectSearchWork>,
}
impl ProjectSearchDialog {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project: Project,
        initial_query: String,
        initial_filter: String,
        detector: MimeDetector,
        clipboard: Arc<ClipboardService>,
        clock: Arc<dyn Clock>,
        documents: &EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<Self> {
        let id = FocusId(RequestId::fresh()?.0);
        let owner = OwnerId(RequestId::fresh()?.0);
        let previous_focus = focus.focused;
        focus.focused = Some(id);
        focus.captured = None;
        let field = |value| {
            let mut field = TextField::new(value);
            field.framed = false;
            field
        };
        let mut dialog = Self {
            id,
            owner,
            project,
            query: field(initial_query),
            replacement: field(String::new()),
            filter: field(initial_filter),
            focused: SearchFocus::Query,
            editor: None,
            hits: vec![],
            selected: None,
            scroll: 0,
            error: None,
            warnings: vec![],
            finished: false,
            geometry: Default::default(),
            splitter: Splitter::new(Axis::Vertical, 4),
            styler: None,
            previous_focus,
            detector,
            clipboard,
            clock,
            pending_search: None,
            pending_open: None,
            pending_paste: None,
            highlight: None,
            observed_revision: None,
            changed_at: None,
            center_preview: false,
            help: false,
            work: vec![],
        };
        dialog.begin_search(documents)?;
        Ok(dialog)
    }
    pub fn prefill(selected: &str, path: Option<&Path>) -> (String, String) {
        let query = escape_literal(selected);
        let filter = path
            .and_then(Path::extension)
            .and_then(|extension| extension.to_str())
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!("\\.{}", escape_literal(extension)))
            .unwrap_or_default();
        (query, filter)
    }
    pub fn take_work(&mut self) -> Vec<ProjectSearchWork> {
        std::mem::take(&mut self.work)
    }
    fn tag(&self) -> Result<JobTag> {
        Ok(JobTag {
            workspace: self.project.generation,
            request: RequestId::fresh()?,
            document: None,
        })
    }
    pub fn begin_search(&mut self, documents: &EditorDocuments) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        let tag = self.tag()?;
        self.pending_search = Some(tag);
        self.pending_open = None;
        self.highlight = None;
        self.changed_at = None;
        self.error = None;
        self.warnings.clear();
        self.hits.clear();
        self.selected = None;
        self.scroll = 0;
        self.work.retain(|work| {
            !matches!(
                work,
                ProjectSearchWork::Search(_) | ProjectSearchWork::OpenHit { .. }
            )
        });
        let snapshots = documents.ids().filter_map(|id| documents.document(id).ok()).filter(|document| matches!(document.origin(), DocumentOrigin::File(path) if path.starts_with(&self.project.root))).map(|document| document.snapshot()).collect();
        self.work.push(ProjectSearchWork::Search(ProjectSearchJob {
            tag,
            project: self.project.clone(),
            query: self.query.value.clone(),
            file_filter: self.filter.value.clone(),
            open_documents: snapshots,
            detector: self.detector.clone(),
            styler: self.styler.clone(),
        }));
        Ok(())
    }
    pub fn complete_search(
        &mut self,
        tag: JobTag,
        result: Result<ProjectSearchReport>,
    ) -> Result<bool> {
        if self.finished || self.pending_search != Some(tag) {
            return Ok(false);
        }
        self.pending_search = None;
        match result {
            Err(error) => self.error = Some(error.to_string()),
            Ok(report)
                if report.tag == tag
                    && report.query == self.query.value
                    && report.file_filter == self.filter.value =>
            {
                self.hits = report.hits;
                self.warnings = report.warnings;
                if report.omitted_warnings > 0 {
                    self.warnings.push(format!(
                        "{} additional file warnings",
                        report.omitted_warnings
                    ));
                }
                self.selected = (!self.hits.is_empty()).then_some(0);
                self.request_selected()?;
            }
            Ok(_) => return Ok(false),
        }
        Ok(true)
    }
    fn request_selected(&mut self) -> Result<()> {
        self.work
            .retain(|work| !matches!(work, ProjectSearchWork::OpenHit { .. }));
        self.pending_open = None;
        if let Some(hit) = self
            .selected
            .and_then(|index| self.hits.get(index))
            .cloned()
        {
            let tag = self.tag()?;
            self.pending_open = Some(tag);
            self.work.push(ProjectSearchWork::OpenHit { tag, hit });
        }
        Ok(())
    }
    /// The shell reuses an existing registry document or prepares a real load in
    /// a worker before creating this view. No async load ever invents empty text.
    pub fn attach_editor(
        &mut self,
        tag: JobTag,
        mut view: EditorView<QueuedEditorHost>,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<bool> {
        if self.finished || self.pending_open != Some(tag) {
            return Ok(false);
        }
        let Some(hit) = self.selected.and_then(|index| self.hits.get(index)) else {
            return Ok(false);
        };
        let document = documents.document(view.document_id())?;
        let snapshot = document.snapshot();
        if document.origin() != &DocumentOrigin::File(hit.path.clone())
            || !hit.matches_snapshot(&snapshot)
        {
            self.error = Some("Selected match changed; refreshing project search".into());
            self.begin_search(documents)?;
            return Ok(false);
        }
        view.input.state.caret.set_selection(
            &snapshot,
            Selection {
                anchor: hit.range.start,
                head: hit.range.end,
            },
        )?;
        view.command(
            EditorCommand::MoveTo {
                position: hit.range.end,
                extend: true,
            },
            documents,
            self.geometry.editor,
        )?;
        // The real selection is already validated. Layout is reconciled by the
        // first render/input tick against the shared registry document.
        self.highlight = Some((snapshot.id, snapshot.revision, hit.range));
        self.observed_revision = Some(snapshot.revision);
        self.center_preview = true;
        if let Some(mut old) = self.editor.take() {
            old.detach(focus);
            self.drain_editor(&mut old);
        }
        if self.focused == SearchFocus::Editor {
            focus.focused = Some(view.input.id);
        }
        self.pending_open = None;
        self.drain_editor(&mut view);
        self.editor = Some(view);
        Ok(true)
    }
    pub fn load_failed(&mut self, tag: JobTag, error: DomainError) -> bool {
        if self.finished || self.pending_open != Some(tag) {
            return false;
        }
        self.pending_open = None;
        self.error = Some(error.to_string());
        true
    }
    fn drain_editor(&mut self, view: &mut EditorView<QueuedEditorHost>) {
        let owner = view.owner();
        self.work.extend(
            view.take_work()
                .into_iter()
                .map(|work| ProjectSearchWork::Editor { owner, work }),
        );
    }
    fn move_selected(&mut self, delta: isize) -> Result<()> {
        if self.hits.is_empty() {
            return Ok(());
        }
        let index = self
            .selected
            .unwrap_or(0)
            .saturating_add_signed(delta)
            .min(self.hits.len() - 1);
        self.selected = Some(index);
        let rows = self.geometry.list.height as usize;
        if rows > 0 {
            if index < self.scroll {
                self.scroll = index;
            } else if index >= self.scroll + rows {
                self.scroll = index + 1 - rows;
            }
        }
        self.request_selected()
    }
    fn set_focus(&mut self, target: SearchFocus, focus: &mut FocusState) {
        self.pending_paste = None;
        self.focused = target;
        if target != SearchFocus::Editor
            && let Some(view) = &mut self.editor
        {
            view.intelligence_changed();
        }
        focus.focused = if target == SearchFocus::Editor {
            self.editor
                .as_ref()
                .map(|view| view.input.id)
                .or(Some(self.id))
        } else {
            Some(self.id)
        };
    }
    fn field_mut(&mut self, field: SearchFocus) -> Option<&mut TextField> {
        match field {
            SearchFocus::Query => Some(&mut self.query),
            SearchFocus::Replacement => Some(&mut self.replacement),
            SearchFocus::Filter => Some(&mut self.filter),
            _ => None,
        }
    }
    fn field_event(&mut self, event: &InputEvent, documents: &EditorDocuments) -> Result<()> {
        let field_id = self.focused;
        let Some(field) = self.field_mut(field_id) else {
            return Ok(());
        };
        let before = field.value.clone();
        let action = field.handle(event);
        let changed = field.value != before;
        if let Some(action) = action {
            match action {
                FieldAction::Copy(text) => {
                    let epoch = self.clipboard.remember(&text)?;
                    let tag = self.tag()?;
                    self.work.push(ProjectSearchWork::Clipboard {
                        tag,
                        job: ClipboardJob::Publish(epoch),
                    });
                }
                FieldAction::PasteRequested => {
                    let tag = self.tag()?;
                    let field = self.field_mut(field_id).expect("focused field");
                    self.pending_paste = Some(PasteStamp {
                        tag,
                        field: field_id,
                        value: field.value.clone(),
                        cursor: field.cursor,
                        anchor: field.anchor,
                    });
                    self.work.push(ProjectSearchWork::Clipboard {
                        tag,
                        job: ClipboardJob::Read,
                    });
                }
                FieldAction::LimitReached => {
                    self.error = Some("Search field exceeds its character limit".into())
                }
                FieldAction::Changed => {}
            }
        }
        if changed {
            self.pending_paste = None;
            if field_id != SearchFocus::Replacement {
                self.begin_search(documents)?;
            }
        }
        Ok(())
    }
    pub fn complete_clipboard(
        &mut self,
        tag: JobTag,
        result: Result<ClipboardResult>,
        documents: &EditorDocuments,
    ) -> Result<bool> {
        let Some(stamp) = self.pending_paste.take() else {
            return Ok(false);
        };
        if tag != stamp.tag {
            self.pending_paste = Some(stamp);
            return Ok(false);
        }
        if self.finished || self.focused != stamp.field {
            return Ok(false);
        }
        let Some(field) = self.field_mut(stamp.field) else {
            return Ok(false);
        };
        if field.value != stamp.value
            || field.cursor != stamp.cursor
            || field.anchor != stamp.anchor
        {
            return Ok(false);
        }
        match result {
            Ok(ClipboardResult::Read(text)) => {
                self.field_event(&InputEvent::Paste(text), documents)?
            }
            Err(error) => self.error = Some(error.to_string()),
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn dismiss(&mut self, focus: &mut FocusState) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.pending_search = None;
        self.pending_open = None;
        self.pending_paste = None;
        if let Some(mut view) = self.editor.take() {
            view.detach(focus);
            self.drain_editor(&mut view);
        }
        self.work.retain(|work| {
            matches!(
                work,
                ProjectSearchWork::Editor { .. }
                    | ProjectSearchWork::Clipboard {
                        job: ClipboardJob::Publish(_),
                        ..
                    }
            )
        });
        focus.focused = self.previous_focus;
        focus.captured = None;
        self.work.push(ProjectSearchWork::Dismiss);
    }
    fn editor_event(
        &mut self,
        event: &InputEvent,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<bool> {
        let Some(mut view) = self.editor.take() else {
            return Ok(false);
        };
        let result = view.dispatch(event, documents, focus, self.geometry.editor);
        self.drain_editor(&mut view);
        self.editor = Some(view);
        self.observe(documents)?;
        result
    }
    fn observe(&mut self, documents: &EditorDocuments) -> Result<()> {
        if let Some(view) = &self.editor {
            let revision = documents.document(view.document_id())?.revision();
            if self.observed_revision != Some(revision) {
                self.observed_revision = Some(revision);
                self.highlight = None;
                self.pending_search = None;
                self.pending_open = None;
                self.changed_at = Some(self.clock.now_millis());
            }
        }
        Ok(())
    }
    pub fn tick(&mut self, documents: &EditorDocuments, focus: &FocusState) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        if let Some(mut view) = self.editor.take() {
            let result = view.tick(documents, focus, self.geometry.editor);
            self.drain_editor(&mut view);
            self.editor = Some(view);
            result?;
        }
        self.observe(documents)?;
        if self
            .changed_at
            .is_some_and(|changed| self.clock.now_millis().saturating_sub(changed) >= 150)
        {
            self.begin_search(documents)?;
        }
        Ok(())
    }
    fn cycle_focus(&mut self, backwards: bool, focus: &mut FocusState) {
        let order = [
            SearchFocus::Query,
            SearchFocus::Replacement,
            SearchFocus::Filter,
            SearchFocus::List,
            SearchFocus::Editor,
        ];
        let index = order
            .iter()
            .position(|field| *field == self.focused)
            .unwrap_or(0);
        self.set_focus(order[(index + if backwards { 4 } else { 1 }) % 5], focus);
    }
    pub fn dispatch(
        &mut self,
        event: &InputEvent,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<bool> {
        if self.finished {
            return Ok(false);
        }
        if matches!(event, InputEvent::Key(input) if input.key == Key::Function(1) && crate::input::shortcuts::command_key(crate::input::shortcuts::Scope::ProjectSearch,*input).is_some())
        {
            self.help = !self.help;
            return Ok(true);
        }
        if self.help {
            if matches!(event, InputEvent::Key(input) if matches!(input.key, Key::Escape | Key::Enter))
            {
                self.help = false;
            }
            return Ok(true);
        }
        if matches!(event, InputEvent::Focus(_)) {
            self.pending_paste = None;
            self.editor_event(event, documents, focus)?;
            return Ok(true);
        }
        let popup = self.editor.as_ref().is_some_and(|view| {
            !view.completion.entries.is_empty()
                || view.navigation.definition.is_some()
                || view.navigation.usages.is_some()
        });
        let captured = self
            .editor
            .as_ref()
            .is_some_and(|view| view.input.is_dragging(focus));
        if (popup || captured) && self.editor_event(event, documents, focus)? {
            return Ok(true);
        }
        if self.splitter.handle(event, self.geometry.body) {
            return Ok(true);
        }
        if let InputEvent::Key(raw) = event
            && let Some(input) = crate::input::shortcuts::command_key(
                crate::input::shortcuts::Scope::ProjectSearch,
                *raw,
            )
        {
            if input.key == Key::Escape {
                if self.focused == SearchFocus::Editor
                    && self.editor.as_ref().is_some_and(|view| view.search.visible)
                {
                    self.editor_event(event, documents, focus)?;
                } else {
                    self.dismiss(focus);
                }
                return Ok(true);
            }
            if input.key == Key::Tab
                && (self.focused != SearchFocus::Editor || input.modifiers.ctrl)
            {
                self.cycle_focus(input.modifiers.shift, focus);
                return Ok(true);
            }
            if input.modifiers.ctrl
                && !input.modifiers.alt
                && !input.modifiers.meta
                && matches!(input.key, Key::Character('s' | 'S'))
            {
                if let Some(mut view) = self.editor.take() {
                    let result = view.command(EditorCommand::Save, documents, self.geometry.editor);
                    self.drain_editor(&mut view);
                    self.editor = Some(view);
                    result?;
                }
                return Ok(true);
            }
        }
        if let InputEvent::Pointer(pointer) = event {
            let point = Position::new(pointer.x, pointer.y);
            if pointer.action == PointerAction::Down(MouseButton::Left)
                && !self.geometry.modal.contains(point)
            {
                self.dismiss(focus);
                return Ok(true);
            }
            if !self.geometry.modal.contains(point) {
                return Ok(true);
            }
            if pointer.action == PointerAction::Down(MouseButton::Left) {
                if let Some(index) = self
                    .geometry
                    .fields
                    .iter()
                    .position(|area| area.contains(point))
                {
                    let target = [
                        SearchFocus::Query,
                        SearchFocus::Replacement,
                        SearchFocus::Filter,
                    ][index];
                    self.set_focus(target, focus);
                    let area = self.geometry.fields[index];
                    if let Some(field) = self.field_mut(target) {
                        field.pointer(event, area);
                    }
                    return Ok(true);
                }
                if self.geometry.list.contains(point) {
                    let index = self.scroll + (pointer.y - self.geometry.list.y) as usize;
                    self.set_focus(SearchFocus::List, focus);
                    if index < self.hits.len() {
                        self.selected = Some(index);
                        self.request_selected()?;
                    }
                    return Ok(true);
                }
                if self.geometry.editor.contains(point) {
                    self.set_focus(SearchFocus::Editor, focus);
                }
            }
            if self.geometry.list.contains(point)
                && let PointerAction::Scroll { vertical, .. } = pointer.action
            {
                self.scroll = self.scroll.saturating_add_signed(vertical as isize).min(
                    self.hits
                        .len()
                        .saturating_sub(self.geometry.list.height as usize),
                );
                return Ok(true);
            }
            if self.geometry.editor.contains(point) {
                self.editor_event(event, documents, focus)?;
                return Ok(true);
            }
            return Ok(true);
        }
        match self.focused {
            SearchFocus::Query | SearchFocus::Replacement | SearchFocus::Filter => {
                if matches!(event, InputEvent::Key(input) if input.key == Key::Enter) {
                    if matches!(event, InputEvent::Key(input) if input.modifiers.ctrl) {
                        self.begin_search(documents)?;
                    } else {
                        self.move_selected(1)?;
                    }
                } else {
                    self.field_event(event, documents)?;
                }
            }
            SearchFocus::List => {
                if let InputEvent::Key(raw) = event
                    && let Some(input) = crate::input::shortcuts::command_key(
                        crate::input::shortcuts::Scope::ProjectSearch,
                        *raw,
                    )
                {
                    match input.key {
                        Key::Up => self.move_selected(-1)?,
                        Key::Down => self.move_selected(1)?,
                        Key::PageUp => self.move_selected(-(self.geometry.list.height as isize))?,
                        Key::PageDown => self.move_selected(self.geometry.list.height as isize)?,
                        Key::Home => self.move_selected(-(self.hits.len() as isize))?,
                        Key::End => self.move_selected(self.hits.len() as isize)?,
                        Key::Enter => {
                            self.set_focus(SearchFocus::Editor, focus);
                            self.request_selected()?;
                        }
                        _ => {}
                    }
                }
            }
            SearchFocus::Editor => {
                self.editor_event(event, documents, focus)?;
            }
        }
        Ok(true)
    }
    pub fn layout(&self, area: Rect) -> SearchGeometry {
        let modal = centered(
            area,
            (area.width / 2).max(10),
            area.height.saturating_sub(2).max(4),
        );
        let inner = Block::default().borders(Borders::ALL).inner(modal);
        let mut fields = [Rect::default(); 3];
        for (index, label) in ["Find: ", "Replace: ", "File regex: "].iter().enumerate() {
            if index >= inner.height as usize {
                break;
            }
            let label = (label.len() as u16).min(inner.width);
            fields[index] = Rect::new(
                inner.x + label,
                inner.y + index as u16,
                inner.width - label,
                1,
            );
        }
        let status = if inner.height > 3 {
            Rect::new(inner.x, inner.y + 3, inner.width, 1)
        } else {
            Rect::default()
        };
        let body = Rect::new(
            inner.x,
            inner.y + inner.height.min(4),
            inner.width,
            inner.height.saturating_sub(4),
        );
        let Panes {
            first: list,
            splitter: divider,
            second: editor,
        } = split(body, Axis::Vertical, self.splitter.size);
        SearchGeometry {
            modal,
            fields,
            status,
            body,
            list,
            divider,
            editor,
        }
    }
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        documents: &EditorDocuments,
        focus: &FocusState,
    ) -> Result<Option<Position>> {
        if self.finished {
            return Ok(None);
        }
        self.geometry = self.layout(area.intersection(buffer.area));
        let geometry = self.geometry;
        Clear.render(geometry.modal, buffer);
        Block::default()
            .borders(Borders::ALL)
            .title("Project search [F1: scope]")
            .style(theme.style("project-search-dialog"))
            .border_style(theme.style("project-search-dialog-border"))
            .render(geometry.modal, buffer);
        let inner = Block::default().borders(Borders::ALL).inner(geometry.modal);
        let mut caret = None;
        for (index, (label, target)) in [
            ("Find: ", SearchFocus::Query),
            ("Replace: ", SearchFocus::Replacement),
            ("File regex: ", SearchFocus::Filter),
        ]
        .into_iter()
        .enumerate()
        {
            if index >= inner.height as usize {
                break;
            }
            draw_text(
                Rect::new(
                    inner.x,
                    inner.y + index as u16,
                    inner.width.min(label.len() as u16),
                    1,
                ),
                buffer,
                label,
                theme.style("code-search-label"),
            );
            let focused = self.focused == target;
            let field = self.field_mut(target).expect("search field");
            let position = field.render(geometry.fields[index], buffer, theme, focused);
            if position.is_some() {
                caret = position;
            }
        }
        let status = if let Some(error) = &self.error {
            display_text(error)
        } else if self.pending_search.is_some() {
            "Searching…".into()
        } else {
            format!(
                "{}/{} matches{}",
                self.selected.map_or(0, |index| index + 1),
                self.hits.len(),
                if self.warnings.is_empty() {
                    String::new()
                } else {
                    format!("; {} file warnings", self.warnings.len())
                }
            )
        };
        draw_text(
            geometry.status,
            buffer,
            &status,
            theme.style("code-search-label"),
        );
        Clear.render(geometry.list, buffer);
        self.scroll = self.scroll.min(
            self.hits
                .len()
                .saturating_sub(geometry.list.height as usize),
        );
        for (row, hit) in self
            .hits
            .iter()
            .skip(self.scroll)
            .take(geometry.list.height as usize)
            .enumerate()
        {
            paint_hit(
                hit,
                Rect::new(
                    geometry.list.x,
                    geometry.list.y + row as u16,
                    geometry.list.width,
                    1,
                ),
                self.selected == Some(row + self.scroll),
                buffer,
                theme,
            );
        }
        if self.hits.is_empty() {
            draw_text(
                geometry.list,
                buffer,
                "(no matches)",
                theme.style("project-search-list"),
            );
        }
        self.splitter.render(geometry.body, buffer, theme);
        if let Some(view) = &mut self.editor {
            view.synchronize(documents)?;
            if self.center_preview && geometry.editor.height > 0 {
                let document = documents.document(view.document_id())?;
                let snapshot = document.snapshot();
                let editor_geometry = view.geometry(documents, geometry.editor)?;
                let folds = view.visuals.folds.collapsed(&snapshot);
                let layout = view.input.layout(
                    document,
                    crate::editor::input::InputViewport {
                        geometry: editor_geometry,
                        folds: &folds,
                    },
                )?;
                let (row, _) = layout.position_to_cell(view.input.state.caret.selection.head)?;
                view.input.state.scroll_rows = row
                    .saturating_sub(editor_geometry.text.height as usize / 2)
                    .min(
                        layout
                            .rows
                            .len()
                            .saturating_sub(editor_geometry.text.height as usize),
                    );
                self.center_preview = false;
            }
            let overlays = self
                .highlight
                .filter(|(id, revision, _)| {
                    *id == view.document_id()
                        && documents
                            .document(*id)
                            .is_ok_and(|document| document.revision() == *revision)
                })
                .map(|(_, _, range)| OverlaySpan {
                    range,
                    style: theme.style("project-search-list-highlight"),
                    priority: 25,
                });
            let editor_caret = view.render(
                documents,
                focus,
                geometry.editor,
                buffer,
                theme,
                &overlays.into_iter().collect::<Vec<_>>(),
            )?;
            if self.focused == SearchFocus::Editor {
                caret = editor_caret;
            }
        }
        if self.help {
            let area = centered(area.intersection(buffer.area), 72, 12);
            Clear.render(area, buffer);
            let block = Block::default()
                .borders(Borders::ALL)
                .title("Search scope")
                .style(theme.style("project-search-dialog"));
            let inner = block.inner(area);
            block.render(area, buffer);
            Paragraph::new(format!("{SEARCH_SCOPE}\n\nGlobal replace actions do not change files. Edit/replace in the shared preview, then Ctrl+S to save. Tab moves through fields/list; Ctrl+Tab leaves the editor. Esc closes this help." )).wrap(Wrap { trim: false }).render(inner, buffer);
            return Ok(None);
        }
        Ok(caret)
    }
}
fn paint_hit(
    hit: &ProjectSearchHit,
    area: Rect,
    selected: bool,
    buffer: &mut Buffer,
    theme: &Theme,
) {
    if area.is_empty() {
        return;
    }
    use unicode_width::UnicodeWidthStr;
    let base = theme.style(if selected {
        "project-search-list-selected"
    } else {
        "project-search-list"
    });
    draw_text(area, buffer, "", base);
    let filename = display_text(&hit.path.file_name().unwrap_or_default().to_string_lossy());
    let filename = format!(" {filename}");
    let right = (filename.width() as u16).min(area.width);
    let path_x = area.right() - right;
    draw_text(
        Rect::new(path_x, area.y, right, 1),
        buffer,
        &filename,
        base.patch(theme.style("project-search-list-path")),
    );
    let marker_width = 3.min(area.width.saturating_sub(right));
    draw_text(
        Rect::new(area.x, area.y, marker_width, 1),
        buffer,
        if selected { "[*]" } else { "[ ]" },
        base.patch(theme.style("project-search-list-marker")),
    );
    let mut x = area.x + marker_width + u16::from(area.width > marker_width + right);
    for (byte, character) in hit.text.char_indices() {
        if x >= path_x {
            break;
        }
        let glyph = Glyph::for_scalar(character);
        let mut style: Style = base;
        for span in &hit.syntax {
            if span.range.contains(kode_core::ByteOffset(byte)) {
                style = style.patch(theme.style(&span.class));
            }
        }
        if hit.range_in_line.contains(kode_core::ByteOffset(byte)) {
            style = style.patch(theme.style("project-search-list-highlight"));
        }
        buffer.set_stringn(x, area.y, glyph.text(), (path_x - x) as usize, style);
        x = x.saturating_add(glyph.width() as u16);
    }
}
