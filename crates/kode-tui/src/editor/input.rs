use super::{
    layout::{CollapsedFold, LayoutCache, VisualLayout},
    render::{EditorGeometry, EditorHit, RenderState},
};
use crate::{
    focus::{FocusId, FocusState},
    input::{bindings::editor_command, terminal::*},
};
use crossterm::event::MouseButton;
use kode_core::{
    DocumentId, DomainError, RequestId, Result, Revision, Selection,
    commands::{EditKind, EditorCommand, Movement},
    document::{Document, EditDelta},
    editor::{edit, movement::move_caret, selection::CaretState},
};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteTicket {
    pub request: RequestId,
    pub document: DocumentId,
    pub revision: Revision,
    pub selection: Selection,
}

/// The host executes clipboard/save/intelligence effects away from drawing.
/// Copy must be remembered locally before scheduling its native clipboard write.
#[derive(Debug, Clone)]
pub enum EditorEffect {
    Copy(String),
    ReadClipboard(PasteTicket),
    Save,
    OpenSearch(String),
    RequestCompletion,
    DismissCompletion,
    Changed {
        revision: Revision,
        deltas: Option<Vec<EditDelta>>,
    },
    CaretMoved,
}
#[derive(Debug, Default)]
pub struct InputOutcome {
    pub handled: bool,
    pub repaint: bool,
    pub effects: Vec<EditorEffect>,
}
impl InputOutcome {
    fn consumed() -> Self {
        Self {
            handled: true,
            repaint: true,
            effects: vec![],
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct EditorState {
    pub caret: CaretState,
    pub scroll_rows: usize,
    pub search_has_focus: bool,
}

#[derive(Clone, Copy)]
pub struct InputViewport<'a> {
    pub geometry: EditorGeometry,
    pub folds: &'a [CollapsedFold],
}
#[derive(Clone, Copy)]
pub struct HookContext<'a> {
    pub geometry: EditorGeometry,
    pub layout: &'a VisualLayout,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputTarget {
    Popup,
    Search,
    Gutter,
    Minimap,
    Navigation,
}

/// Hooks receive the original normalized event, absolute pane rectangles and the
/// same source map as paint. An owner consumes input once; no modifiers are lost
/// or coordinates translated twice in nested editor hosts.
pub trait EditorInputHooks {
    fn context_changed(&mut self, _: &kode_core::document::TextSnapshot, _: &EditorState) {}
    fn collapsed_folds(&self, _: &kode_core::document::TextSnapshot) -> Option<Vec<CollapsedFold>> {
        None
    }
    fn reveal_caret(
        &mut self,
        _: &kode_core::document::TextSnapshot,
        _: kode_core::ByteOffset,
    ) -> Result<bool> {
        Ok(false)
    }
    fn dispatch(
        &mut self,
        target: InputTarget,
        event: &InputEvent,
        document: &mut Document,
        state: &mut EditorState,
        context: HookContext<'_>,
    ) -> Result<bool>;
}
impl EditorInputHooks for () {
    fn dispatch(
        &mut self,
        _: InputTarget,
        _: &InputEvent,
        _: &mut Document,
        _: &mut EditorState,
        _: HookContext<'_>,
    ) -> Result<bool> {
        Ok(false)
    }
}

/// View state belongs to a single document and focus owner; documents themselves
/// stay in the host registry. There is no I/O or native clipboard access here.
pub struct EditorInput {
    pub id: FocusId,
    pub document: DocumentId,
    pub state: EditorState,
    cache: LayoutCache,
    window_focused: bool,
    drag: Option<PointerInput>,
    pending_paste: Option<PasteTicket>,
}
impl EditorInput {
    pub fn new(id: FocusId, document: DocumentId) -> Self {
        Self {
            id,
            document,
            state: EditorState::default(),
            cache: LayoutCache::default(),
            window_focused: true,
            drag: None,
            pending_paste: None,
        }
    }
    pub fn is_dragging(&self, focus: &FocusState) -> bool {
        self.drag.is_some() && focus.captured == Some(self.id)
    }
    pub fn render_state(&self, focus: &FocusState) -> RenderState {
        RenderState {
            selection: self.state.caret.selection,
            scroll_rows: self.state.scroll_rows,
            focused: self.window_focused
                && focus.focused == Some(self.id)
                && !self.state.search_has_focus,
        }
    }
    pub fn has_focus(&self, focus: &FocusState) -> bool {
        self.window_focused && focus.focused == Some(self.id)
    }
    pub fn layout(
        &mut self,
        document: &Document,
        viewport: InputViewport<'_>,
    ) -> Result<Arc<VisualLayout>> {
        if document.id() != self.document {
            return Err(DomainError::Unsupported(
                "editor view belongs to another document".into(),
            ));
        }
        self.cache.get(
            &document.snapshot(),
            viewport.geometry.text.width as usize,
            viewport.folds,
        )
    }
    fn clamp_scroll(&mut self, layout: &VisualLayout, geometry: EditorGeometry) {
        self.state.scroll_rows = self.state.scroll_rows.min(max_scroll(layout, geometry));
    }
    pub fn ensure_caret_visible(
        &mut self,
        layout: &VisualLayout,
        geometry: EditorGeometry,
    ) -> Result<()> {
        self.clamp_scroll(layout, geometry);
        let height = geometry.text.height as usize;
        if height == 0 {
            return Ok(());
        }
        let (row, _) = layout.position_to_cell(self.state.caret.cursor())?;
        if row < self.state.scroll_rows {
            self.state.scroll_rows = row;
        } else if row >= self.state.scroll_rows + height {
            self.state.scroll_rows = row - height + 1;
        }
        self.clamp_scroll(layout, geometry);
        Ok(())
    }
    fn drag_to(
        &mut self,
        pointer: PointerInput,
        layout: &VisualLayout,
        geometry: EditorGeometry,
    ) -> Result<()> {
        if geometry.text.is_empty() {
            return Ok(());
        }
        if pointer.y < geometry.text.y {
            self.state.scroll_rows = self.state.scroll_rows.saturating_sub(1);
        } else if pointer.y >= geometry.text.bottom() {
            self.state.scroll_rows = (self.state.scroll_rows + 1).min(max_scroll(layout, geometry));
        }
        let row = self.state.scroll_rows
            + pointer
                .y
                .clamp(geometry.text.y, geometry.text.bottom() - 1)
                .saturating_sub(geometry.text.y) as usize;
        let column = pointer.x.saturating_sub(geometry.text.x) as usize;
        self.state
            .caret
            .move_to(&layout.snapshot, layout.cell_to_position(row, column), true)?;
        self.ensure_caret_visible(layout, geometry)
    }
    /// Call from the host's animation tick while captured, so a stationary
    /// pointer beyond a pane edge keeps extending and scrolling the selection.
    pub fn tick_drag(
        &mut self,
        document: &Document,
        focus: &FocusState,
        viewport: InputViewport<'_>,
    ) -> Result<InputOutcome> {
        if !self.is_dragging(focus) || !self.window_focused {
            return Ok(InputOutcome::default());
        }
        let pointer = self.drag.expect("captured drag has pointer");
        if pointer.y >= viewport.geometry.text.y && pointer.y < viewport.geometry.text.bottom() {
            return Ok(InputOutcome::default());
        }
        let layout = self.layout(document, viewport)?;
        let before = (self.state.caret.selection, self.state.scroll_rows);
        self.drag_to(pointer, &layout, viewport.geometry)?;
        let changed = before != (self.state.caret.selection, self.state.scroll_rows);
        Ok(InputOutcome {
            handled: true,
            repaint: changed,
            effects: if changed {
                vec![EditorEffect::CaretMoved]
            } else {
                vec![]
            },
        })
    }
    pub fn dispatch(
        &mut self,
        event: &InputEvent,
        document: &mut Document,
        focus: &mut FocusState,
        viewport: InputViewport<'_>,
        hooks: &mut dyn EditorInputHooks,
    ) -> Result<InputOutcome> {
        let folds = hooks.collapsed_folds(&document.snapshot());
        let viewport = InputViewport {
            geometry: viewport.geometry,
            folds: folds.as_deref().unwrap_or(viewport.folds),
        };
        let before = document.revision();
        let outcome = self.dispatch_inner(event, document, focus, viewport, hooks)?;
        hooks.context_changed(&document.snapshot(), &self.state);
        let reveal = document.revision() != before
            || outcome
                .effects
                .iter()
                .any(|effect| matches!(effect, EditorEffect::CaretMoved));
        if outcome.handled {
            self.reconcile(document, viewport, hooks, reveal)?;
        }
        Ok(outcome)
    }
    /// Apply after asynchronous search/navigation publication as well as input.
    pub fn reconcile(
        &mut self,
        document: &Document,
        viewport: InputViewport<'_>,
        hooks: &mut dyn EditorInputHooks,
        reveal: bool,
    ) -> Result<()> {
        if reveal {
            hooks.reveal_caret(&document.snapshot(), self.state.caret.cursor())?;
        }
        let folds = hooks.collapsed_folds(&document.snapshot());
        let viewport = InputViewport {
            geometry: viewport.geometry,
            folds: folds.as_deref().unwrap_or(viewport.folds),
        };
        let layout = self.layout(document, viewport)?;
        if reveal {
            self.ensure_caret_visible(&layout, viewport.geometry)?;
        } else {
            self.clamp_scroll(&layout, viewport.geometry);
        }
        Ok(())
    }
    fn dispatch_inner(
        &mut self,
        event: &InputEvent,
        document: &mut Document,
        focus: &mut FocusState,
        viewport: InputViewport<'_>,
        hooks: &mut dyn EditorInputHooks,
    ) -> Result<InputOutcome> {
        let layout = self.layout(document, viewport)?;
        let geometry = viewport.geometry;
        self.clamp_scroll(&layout, geometry);
        if let InputEvent::Focus(focused) = event {
            hooks.dispatch(
                InputTarget::Search,
                event,
                document,
                &mut self.state,
                HookContext {
                    geometry,
                    layout: &layout,
                },
            )?;
            self.window_focused = *focused;
            if !focused {
                self.drag = None;
                self.pending_paste = None;
                if focus.captured == Some(self.id) {
                    focus.captured = None;
                }
            }
            return Ok(InputOutcome::consumed());
        }
        if let InputEvent::Resize { .. } = event {
            self.ensure_caret_visible(&layout, geometry)?;
            return Ok(InputOutcome::consumed());
        }
        if !self.window_focused {
            return Ok(InputOutcome::default());
        }
        if let InputEvent::Pointer(pointer) = event {
            if focus.captured.is_some() && focus.captured != Some(self.id) {
                return Ok(InputOutcome::default());
            }
            if self.is_dragging(focus) {
                match pointer.action {
                    PointerAction::Move | PointerAction::Drag(MouseButton::Left) => {
                        self.drag = Some(*pointer);
                        self.drag_to(*pointer, &layout, geometry)?;
                        let mut outcome = InputOutcome::consumed();
                        outcome.effects.push(EditorEffect::CaretMoved);
                        return Ok(outcome);
                    }
                    PointerAction::Up(MouseButton::Left) => {
                        self.drag = None;
                        focus.captured = None;
                        return Ok(InputOutcome::consumed());
                    }
                    _ => {}
                }
            }
            if matches!(
                geometry.hit_test(&layout, self.state.scroll_rows, pointer.x, pointer.y),
                EditorHit::Outside
            ) {
                let before = (document.revision(), self.state.caret.selection);
                if hooks.dispatch(
                    InputTarget::Popup,
                    event,
                    document,
                    &mut self.state,
                    HookContext {
                        geometry,
                        layout: &layout,
                    },
                )? {
                    self.pending_paste = None;
                    return self.after_hook(document, viewport, before);
                }
                return Ok(InputOutcome::default());
            }
        } else if focus.focused != Some(self.id) {
            return Ok(InputOutcome::default());
        }
        let context = HookContext {
            geometry,
            layout: &layout,
        };
        let before = (document.revision(), self.state.caret.selection);
        if hooks.dispatch(
            InputTarget::Popup,
            event,
            document,
            &mut self.state,
            context,
        )? {
            self.pending_paste = None;
            return self.after_hook(document, viewport, before);
        }
        if let InputEvent::Key(input) = event
            && editor_command(*input) == Some(EditorCommand::OpenSearch)
        {
            return self.command(EditorCommand::OpenSearch, document, viewport);
        }
        let search_pointer = match event {
            InputEvent::Pointer(pointer) => geometry.search.contains((pointer.x, pointer.y).into()),
            _ => false,
        };
        if search_pointer
            || (self.state.search_has_focus
                && matches!(event, InputEvent::Key(_) | InputEvent::Paste(_)))
        {
            if search_pointer {
                focus.focused = Some(self.id);
                self.state.search_has_focus = true;
            }
            let handled = hooks.dispatch(
                InputTarget::Search,
                event,
                document,
                &mut self.state,
                context,
            )?;
            if handled || !search_pointer {
                self.pending_paste = None;
                return self.after_hook(document, viewport, before);
            }
            return Ok(InputOutcome::consumed());
        }
        match event {
            InputEvent::Pointer(pointer) => match pointer.action {
                PointerAction::Scroll { vertical, .. } if vertical != 0 => {
                    let previous = self.state.scroll_rows;
                    let delta = vertical as isize * if pointer.modifiers.alt { 9 } else { 3 };
                    self.state.scroll_rows = previous
                        .saturating_add_signed(delta)
                        .min(max_scroll(&layout, geometry));
                    Ok(InputOutcome {
                        handled: true,
                        repaint: previous != self.state.scroll_rows,
                        effects: vec![EditorEffect::DismissCompletion],
                    })
                }
                PointerAction::Down(MouseButton::Left) => {
                    let hit =
                        geometry.hit_test(&layout, self.state.scroll_rows, pointer.x, pointer.y);
                    let target = match hit {
                        EditorHit::Gutter { .. } | EditorHit::FoldToggle { .. } => {
                            Some(InputTarget::Gutter)
                        }
                        EditorHit::Minimap { .. } => Some(InputTarget::Minimap),
                        _ => None,
                    };
                    if let Some(target) = target
                        && hooks.dispatch(target, event, document, &mut self.state, context)?
                    {
                        self.pending_paste = None;
                        focus.focused = Some(self.id);
                        self.state.search_has_focus = false;
                        return self.after_hook(document, viewport, before);
                    }
                    if pointer.modifiers.ctrl
                        && matches!(hit, EditorHit::Text(_) | EditorHit::Gutter { .. })
                        && hooks.dispatch(
                            InputTarget::Navigation,
                            event,
                            document,
                            &mut self.state,
                            context,
                        )?
                    {
                        self.pending_paste = None;
                        focus.focused = Some(self.id);
                        self.state.search_has_focus = false;
                        return self.after_hook(document, viewport, before);
                    }
                    let position = match hit {
                        EditorHit::Text(position) => position,
                        EditorHit::Gutter { .. } => layout.cell_to_position(
                            self.state.scroll_rows + (pointer.y - geometry.gutter.y) as usize,
                            0,
                        ),
                        _ => return Ok(InputOutcome::default()),
                    };
                    self.state
                        .caret
                        .move_to(&layout.snapshot, position, false)?;
                    self.state.search_has_focus = false;
                    focus.focused = Some(self.id);
                    focus.captured = Some(self.id);
                    self.drag = Some(*pointer);
                    self.pending_paste = None;
                    self.ensure_caret_visible(&layout, geometry)?;
                    Ok(InputOutcome {
                        handled: true,
                        repaint: true,
                        effects: vec![EditorEffect::CaretMoved],
                    })
                }
                _ => Ok(InputOutcome::default()),
            },
            InputEvent::Key(input) => match editor_command(*input) {
                Some(command) => self.command(command, document, viewport),
                None => Ok(InputOutcome::default()),
            },
            InputEvent::Paste(text) => self.command(
                EditorCommand::Replace {
                    range: self.state.caret.selection.range(),
                    text: text.clone(),
                    kind: EditKind::Paste,
                },
                document,
                viewport,
            ),
            _ => Ok(InputOutcome::default()),
        }
    }
    fn after_hook(
        &mut self,
        document: &Document,
        viewport: InputViewport<'_>,
        before: (Revision, Selection),
    ) -> Result<InputOutcome> {
        let layout = self.layout(document, viewport)?;
        if before != (document.revision(), self.state.caret.selection) {
            self.ensure_caret_visible(&layout, viewport.geometry)?;
        } else {
            self.clamp_scroll(&layout, viewport.geometry);
        }
        let mut outcome = InputOutcome::consumed();
        if document.revision() != before.0 {
            outcome.effects.push(EditorEffect::Changed {
                revision: document.revision(),
                deltas: None,
            });
        }
        if self.state.caret.selection != before.1 {
            outcome.effects.push(EditorEffect::CaretMoved);
        }
        Ok(outcome)
    }
    /// Explicit host commands use the same mutation/scroll/history path as keys.
    pub fn command(
        &mut self,
        command: EditorCommand,
        document: &mut Document,
        viewport: InputViewport<'_>,
    ) -> Result<InputOutcome> {
        let layout = self.layout(document, viewport)?;
        let before = self.state.caret.selection;
        let previous_revision = document.revision();
        let mut outcome = InputOutcome::consumed();
        let mut edit_outcome = None;
        let movement_command = matches!(
            command,
            EditorCommand::Move { .. } | EditorCommand::MoveTo { .. }
        );
        let reveal = matches!(
            command,
            EditorCommand::Move { .. }
                | EditorCommand::MoveTo { .. }
                | EditorCommand::SelectAll
                | EditorCommand::InsertText(_)
                | EditorCommand::InsertTab
                | EditorCommand::InsertNewline
                | EditorCommand::DeleteBackward
                | EditorCommand::DeleteForward
                | EditorCommand::Replace { .. }
                | EditorCommand::Undo
                | EditorCommand::Redo
        );
        self.pending_paste = None;
        match command {
            EditorCommand::Move { movement, extend } => {
                let page = matches!(movement, Movement::PageUp | Movement::PageDown);
                let target = if page {
                    let (row, column) = layout.position_to_cell(self.state.caret.cursor())?;
                    let height = viewport.geometry.text.height as usize;
                    let delta = if movement == Movement::PageUp {
                        -(height as isize)
                    } else {
                        height as isize
                    };
                    let row = row.saturating_add_signed(delta).min(layout.rows.len() - 1);
                    self.state.scroll_rows = self
                        .state
                        .scroll_rows
                        .saturating_add_signed(delta)
                        .min(max_scroll(&layout, viewport.geometry));
                    Some(
                        layout
                            .cell_to_position(row, column)
                            .min(layout.rows[row].source.end),
                    )
                } else {
                    None
                };
                move_caret(
                    &layout.snapshot,
                    &mut self.state.caret,
                    movement,
                    extend,
                    target,
                )?;
            }
            EditorCommand::MoveTo { position, extend } => {
                self.state
                    .caret
                    .move_to(&layout.snapshot, position, extend)?
            }
            EditorCommand::SelectAll => self.state.caret.select_all(&layout.snapshot),
            EditorCommand::InsertText(text) => {
                edit_outcome =
                    edit::insert(document, &mut self.state.caret, &text, EditKind::Typing)?
            }
            EditorCommand::InsertTab => {
                edit_outcome =
                    edit::insert(document, &mut self.state.caret, "\t", EditKind::Typing)?
            }
            EditorCommand::InsertNewline => {
                edit_outcome = edit::newline(document, &mut self.state.caret)?
            }
            EditorCommand::DeleteBackward => {
                edit_outcome = edit::delete(document, &mut self.state.caret, true)?
            }
            EditorCommand::DeleteForward => {
                edit_outcome = edit::delete(document, &mut self.state.caret, false)?
            }
            EditorCommand::Replace { range, text, kind } => {
                edit_outcome = edit::replace(document, &mut self.state.caret, range, &text, kind)?
            }
            EditorCommand::Undo => {
                document.undo(&mut self.state.caret)?;
            }
            EditorCommand::Redo => {
                document.redo(&mut self.state.caret)?;
            }
            EditorCommand::Copy | EditorCommand::Cut => {
                if command == EditorCommand::Cut && document.is_read_only() {
                    return Err(DomainError::ReadOnly);
                }
                if !before.is_empty() {
                    let text = self.state.caret.selected_text(&layout.snapshot)?;
                    if command == EditorCommand::Cut {
                        edit_outcome =
                            edit::insert(document, &mut self.state.caret, "", EditKind::Cut)?;
                    }
                    outcome.effects.push(EditorEffect::Copy(text));
                }
            }
            EditorCommand::Paste => {
                if document.is_read_only() {
                    return Err(DomainError::ReadOnly);
                }
                let ticket = PasteTicket {
                    request: RequestId::fresh()?,
                    document: document.id(),
                    revision: document.revision(),
                    selection: before,
                };
                self.pending_paste = Some(ticket.clone());
                outcome.effects.push(EditorEffect::ReadClipboard(ticket));
            }
            EditorCommand::Save => {
                if document.is_read_only() {
                    return Err(DomainError::ReadOnly);
                }
                outcome.effects.push(EditorEffect::Save);
            }
            EditorCommand::OpenSearch => {
                self.state.search_has_focus = true;
                outcome.effects.push(EditorEffect::OpenSearch(
                    self.state.caret.selected_text(&layout.snapshot)?,
                ));
            }
            EditorCommand::RequestCompletion => {
                outcome.effects.push(EditorEffect::RequestCompletion)
            }
            _ => return Ok(InputOutcome::default()),
        }
        if document.revision() != previous_revision {
            outcome.effects.push(EditorEffect::Changed {
                revision: document.revision(),
                deltas: edit_outcome.map(|edit| edit.deltas),
            });
            outcome.effects.push(EditorEffect::DismissCompletion);
        }
        if movement_command || self.state.caret.selection != before {
            outcome.effects.push(EditorEffect::CaretMoved);
        }
        // Copy/save do not unexpectedly undo wheel scrolling. Navigation and edits
        // restore the caret; page commands already shift the viewport by a page.
        if reveal
            || document.revision() != previous_revision
            || self.state.caret.selection != before
        {
            let layout = self.layout(document, viewport)?;
            self.ensure_caret_visible(&layout, viewport.geometry)?;
        }
        Ok(outcome)
    }
    pub fn complete_paste(
        &mut self,
        ticket: &PasteTicket,
        text: &str,
        document: &mut Document,
        focus: &FocusState,
        viewport: InputViewport<'_>,
    ) -> Result<InputOutcome> {
        if self.pending_paste.as_ref() != Some(ticket)
            || ticket.document != document.id()
            || ticket.revision != document.revision()
            || ticket.selection != self.state.caret.selection
            || focus.focused != Some(self.id)
            || !self.window_focused
            || self.state.search_has_focus
        {
            return Ok(InputOutcome::default());
        }
        self.command(
            EditorCommand::Replace {
                range: ticket.selection.range(),
                text: text.into(),
                kind: EditKind::Paste,
            },
            document,
            viewport,
        )
    }
}
fn max_scroll(layout: &VisualLayout, geometry: EditorGeometry) -> usize {
    layout
        .rows
        .len()
        .saturating_sub(geometry.text.height as usize)
}
