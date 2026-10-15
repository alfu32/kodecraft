use super::usages::{UsageEntry, compact_path};
use crate::{
    app::events::ResultGate,
    editor::{
        display::Glyph,
        input::{EditorInputHooks, EditorState, HookContext, InputTarget},
        render::EditorHit,
    },
    input::terminal::*,
    theme::Theme,
};
use crossterm::event::MouseButton;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, JobTag, LineIndex, Result, TextRange,
    document::{Document, TextSnapshot, Utf16Position},
    intelligence::{HighlightSpan, IntelligenceProvider, IntelligenceRequest, NavigationTarget},
    services::DocumentIo,
};
use kode_core::{Selection, WorkspaceGeneration};
use kode_platform::jobs::Clock;
use kode_platform::jobs::{Delivery, OwnerId, WorkerScheduler};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Modifier,
    widgets::{Clear, Widget},
};
use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub const HOVER_DELAY_MS: u64 = 500;
pub const MAX_PREVIEW_LINES: usize = 24;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationOperation {
    Click,
    Hover,
    Definitions,
    References,
}
#[derive(Debug, Clone)]
pub struct Identifier {
    pub range: TextRange,
    pub name: String,
}
fn identifier_part(ch: char) -> bool {
    use unicode_general_category::{GeneralCategory::*, get_general_category};
    ch == '_'
        || ch as u32 <= 0xFFFF
            && matches!(
                get_general_category(ch),
                UppercaseLetter
                    | LowercaseLetter
                    | TitlecaseLetter
                    | ModifierLetter
                    | OtherLetter
                    | DecimalNumber
            )
}
pub fn lexical_identifier(
    snapshot: &TextSnapshot,
    position: ByteOffset,
) -> Result<Option<Identifier>> {
    let point = snapshot.byte_to_position(position)?;
    let content = snapshot.line_content(point.line)?;
    let mut column = point.column.0.min(content.len_chars());
    if column == content.len_chars() || !identifier_part(content.char(column)) {
        if column == 0 || !identifier_part(content.char(column - 1)) {
            return Ok(None);
        }
        column -= 1;
    }
    let mut start = column;
    let mut end = column + 1;
    while start > 0 && identifier_part(content.char(start - 1)) {
        start -= 1;
    }
    while end < content.len_chars() && identifier_part(content.char(end)) {
        end += 1;
    }
    let base = snapshot.line_start(point.line)?.0;
    Ok(Some(Identifier {
        range: TextRange::new(
            ByteOffset(base + content.char_to_byte(start)),
            ByteOffset(base + content.char_to_byte(end)),
        )?,
        name: content.slice(start..end).to_string(),
    }))
}
#[derive(Debug, Clone)]
pub struct NavigationJob {
    pub tag: JobTag,
    pub snapshot: TextSnapshot,
    pub operation: NavigationOperation,
    pub position: ByteOffset,
    pub identifier: Identifier,
    pub declaration_hint: bool,
    pub project_root: PathBuf,
}
pub trait PreviewSourceReader: Send + Sync {
    fn read(&self, target: &NavigationTarget, tag: JobTag) -> Result<TextSnapshot>;
}
pub struct NativePreviewSourceReader {
    pub io: Arc<dyn DocumentIo>,
}
impl PreviewSourceReader for NativePreviewSourceReader {
    fn read(&self, target: &NavigationTarget, tag: JobTag) -> Result<TextSnapshot> {
        let loaded = self
            .io
            .load(&DocumentOrigin::File(target.path.clone()), tag)?;
        let decoded = kode_core::document::encoding::decode(&loaded.bytes)?;
        Ok(Document::from_decoded(
            target.document.map_or_else(DocumentId::fresh, Ok)?,
            loaded.origin,
            target.language.clone(),
            decoded,
        )
        .snapshot())
    }
}
pub trait PreviewHighlighter: Send + Sync {
    fn highlights(&self, snapshot: &TextSnapshot) -> Result<Vec<HighlightSpan>>;
}
#[derive(Debug)]
pub struct PreviewWindow {
    pub snapshot: TextSnapshot,
    pub start: LineIndex,
    pub lines: usize,
    pub highlights: Vec<HighlightSpan>,
    pub max_width: usize,
}
#[derive(Debug)]
pub struct DefinitionTarget {
    pub target: NavigationTarget,
    pub location: String,
    pub preview: Result<PreviewWindow>,
}
#[derive(Debug)]
pub enum NavigationResult {
    Navigate(NavigationTarget),
    Usages(Vec<UsageEntry>),
    Definitions(Vec<DefinitionTarget>),
    Empty,
}

fn normalized(path: &Path, root: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(component.as_os_str()),
        }
    }
    out
}
fn same_document(job: &NavigationJob, target: &NavigationTarget) -> bool {
    target.document == Some(job.snapshot.id)
        || match &job.snapshot.origin {
            DocumentOrigin::File(path) => {
                normalized(path, &job.project_root) == normalized(&target.path, &job.project_root)
            }
            _ => false,
        }
}
fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}
fn window(
    snapshot: TextSnapshot,
    target: &NavigationTarget,
    highlighter: Option<&dyn PreviewHighlighter>,
) -> Result<PreviewWindow> {
    let range = target.declaration_range.unwrap_or(target.identifier_range);
    let first = snapshot
        .byte_to_position(snapshot.clamp_byte(range.start))?
        .line;
    let end = snapshot.byte_to_position(snapshot.clamp_byte(range.end))?;
    let last = if target.declaration_range.is_none() {
        first.0
    } else if end.line.0 > first.0 && end.column.0 == 0 {
        end.line.0 - 1
    } else {
        end.line.0
    };
    let lines = (last.saturating_sub(first.0) + 1).min(MAX_PREVIEW_LINES);
    let highlights = highlighter
        .map(|highlighter| highlighter.highlights(&snapshot))
        .transpose()?
        .unwrap_or_default();
    for span in &highlights {
        snapshot.slice(span.range)?;
    }
    let mut max_width = 0;
    for line in first.0..first.0 + lines {
        max_width = max_width.max(
            snapshot
                .line_content(LineIndex(line))?
                .chars()
                .map(|ch| Glyph::for_scalar(ch).width())
                .sum(),
        );
    }
    Ok(PreviewWindow {
        snapshot,
        start: first,
        lines,
        highlights,
        max_width,
    })
}
fn usage_label(
    target: &NavigationTarget,
    snapshot: Option<&TextSnapshot>,
    root: &Path,
    identifier: &str,
) -> Result<String> {
    let location = target.location.unwrap_or_default();
    let mut snippet = String::new();
    if let Some(snapshot) = snapshot {
        let content =
            snapshot.line_content(LineIndex(location.line.0.min(snapshot.len_lines() - 1)))?;
        let total = content.chars().map(char::len_utf16).sum::<usize>();
        let column = location.column.0 as usize;
        let from = column.saturating_sub(30);
        let until = (column + identifier.encode_utf16().count().max(1) + 30).min(total);
        let start = snapshot.utf16_to_byte_clamped(Utf16Position {
            line: location.line,
            column: kode_core::Utf16Column(from.min(u32::MAX as usize) as u32),
        });
        let end = snapshot.utf16_to_byte_clamped(Utf16Position {
            line: location.line,
            column: kode_core::Utf16Column(until.min(u32::MAX as usize) as u32),
        });
        snippet = format!(
            "{}{}{}",
            if from > 0 { "..." } else { "" },
            snapshot
                .slice(TextRange::new(start, end)?)?
                .to_string()
                .trim(),
            if until < total { "..." } else { "" }
        );
    }
    let line = target
        .location
        .map_or("?".into(), |position| (position.line.0 + 1).to_string());
    let column = target.location.map_or("?".into(), |position| {
        (position.column.0 as usize + 1).to_string()
    });
    Ok(format!(
        "{}:{}:{} | {snippet}",
        compact_path(&relative(&target.path, root), 48),
        line,
        column
    ))
}
/// All source reads, provider calls and preview styling happen in this worker.
/// Current unsaved content is taken from the captured immutable snapshot.
pub fn prepare(
    job: &NavigationJob,
    provider: &dyn IntelligenceProvider,
    reader: &dyn PreviewSourceReader,
    highlighter: Option<&dyn PreviewHighlighter>,
    cancelled: impl Fn() -> bool,
) -> Result<NavigationResult> {
    if cancelled() {
        return Err(DomainError::Cancelled);
    }
    let request = IntelligenceRequest {
        tag: job.tag,
        snapshot: job.snapshot.to_service_snapshot(),
        position: job.position,
    };
    let definitions = if job.operation == NavigationOperation::References
        || job.declaration_hint && job.operation == NavigationOperation::Click
    {
        vec![]
    } else {
        provider.definitions(&request)?
    };
    if cancelled() {
        return Err(DomainError::Cancelled);
    }
    let declaration = job.declaration_hint
        || definitions.iter().any(|target| {
            same_document(job, target)
                && target.identifier_range.start <= job.identifier.range.start
                && job.identifier.range.start < target.identifier_range.end
        });
    let references = job.operation == NavigationOperation::References
        || job.operation == NavigationOperation::Click && declaration;
    if job.operation == NavigationOperation::Click && !references
        || job.operation == NavigationOperation::Definitions
    {
        let lower = job.identifier.name.to_lowercase();
        let best = definitions
            .into_iter()
            .filter(|target| !target.path.as_os_str().is_empty() || target.document.is_some())
            .min_by_key(|target| {
                usize::from(
                    !target
                        .path
                        .file_stem()
                        .is_some_and(|name| name.to_string_lossy().to_lowercase().contains(&lower)),
                ) + usize::from(same_document(job, target))
            });
        return Ok(best.map_or(NavigationResult::Empty, NavigationResult::Navigate));
    }
    let targets = if references {
        provider.references(&request)?
    } else {
        definitions
    };
    let mut cache: HashMap<PathBuf, TextSnapshot> = HashMap::new();
    let mut distinct = HashSet::new();
    let mut previews = vec![];
    let mut usages = vec![];
    for mut target in targets {
        if cancelled() {
            return Err(DomainError::Cancelled);
        }
        if !target.path.as_os_str().is_empty() {
            target.path = normalized(&target.path, &job.project_root);
        }
        if !references
            && !distinct.insert((
                target.path.clone(),
                target.document,
                target.identifier_range,
            ))
        {
            continue;
        }
        let source = if same_document(job, &target) {
            Ok(job.snapshot.clone())
        } else if let Some(snapshot) = cache.get(&target.path) {
            Ok(snapshot.clone())
        } else {
            reader.read(&target, job.tag).inspect(|snapshot| {
                cache.insert(target.path.clone(), snapshot.clone());
            })
        };
        if let Ok(snapshot) = &source {
            target.location =
                Some(snapshot.byte_to_utf16(snapshot.clamp_byte(target.identifier_range.start))?);
        }
        if references {
            let label = if target.path.as_os_str().is_empty() {
                target.name.clone().unwrap_or_default()
            } else {
                usage_label(
                    &target,
                    source.as_ref().ok(),
                    &job.project_root,
                    &job.identifier.name,
                )?
            };
            usages.push(UsageEntry { target, label });
        } else {
            let location = format!(
                "{}:{}",
                relative(&target.path, &job.project_root),
                target
                    .location
                    .map_or("?".into(), |position| (position.line.0 + 1).to_string())
            );
            let preview = source.and_then(|snapshot| window(snapshot, &target, highlighter));
            previews.push(DefinitionTarget {
                target,
                location,
                preview,
            });
        }
    }
    Ok(if references {
        NavigationResult::Usages(usages)
    } else {
        NavigationResult::Definitions(previews)
    })
}
pub fn schedule(
    scheduler: &mut dyn WorkerScheduler<NavigationResult>,
    owner: OwnerId,
    job: NavigationJob,
    provider: Arc<dyn IntelligenceProvider>,
    reader: Arc<dyn PreviewSourceReader>,
    highlighter: Option<Arc<dyn PreviewHighlighter>>,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        job.tag,
        Delivery::Cancellable,
        Box::new(move |cancel| {
            prepare(
                &job,
                provider.as_ref(),
                reader.as_ref(),
                highlighter.as_deref(),
                || cancel.is_cancelled(),
            )
        }),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionControl {
    Previous,
    Next,
    Location,
}
#[derive(Debug, Clone, Copy)]
pub struct DefinitionGeometry {
    pub area: Rect,
    pub previous: Rect,
    pub next: Rect,
    pub location: Rect,
    pub source: Rect,
}
pub struct DefinitionPopup {
    pub anchor: ByteOffset,
    pub targets: Vec<DefinitionTarget>,
    pub selected: usize,
    pub hovered: Option<DefinitionControl>,
}
impl DefinitionPopup {
    pub fn new(anchor: ByteOffset, targets: Vec<DefinitionTarget>) -> Self {
        Self {
            anchor,
            targets,
            selected: 0,
            hovered: None,
        }
    }
    fn nav(&self) -> String {
        format!("<< {}/{} >>", self.selected + 1, self.targets.len())
    }
    pub fn geometry(
        &self,
        context: HookContext<'_>,
        state: &EditorState,
    ) -> Option<DefinitionGeometry> {
        let target = self.targets.get(self.selected)?;
        let body = context.geometry.text;
        if body.width == 0 || body.height < 4 {
            return None;
        }
        let mut label_width = crate::editor::display::display_text(&target.location)
            .chars()
            .count()
            .max(self.nav().len())
            .max(18);
        let rows = match &target.preview {
            Ok(preview) => {
                label_width = label_width.max(preview.max_width);
                preview.lines
            }
            Err(_) => 1,
        };
        let width = (label_width + 2).min(body.width as usize) as u16;
        let height = (rows + 4).min(body.height as usize) as u16;
        let (row, column) = context.layout.position_to_cell(self.anchor).ok()?;
        let screen_y = body.y
            + row
                .saturating_sub(state.scroll_rows)
                .min(body.height as usize - 1) as u16;
        let x = (body.x + column.min(body.width as usize - 1) as u16).min(body.right() - width);
        let y = screen_y
            .saturating_sub(height)
            .max(body.y)
            .min(body.bottom() - height);
        let area = Rect::new(x, y, width, height);
        let inner = width.saturating_sub(2);
        let left = x + width.min(1);
        let arrows = self.targets.len() > 1;
        let right_offset = self.nav().len() - 2;
        Some(DefinitionGeometry {
            area,
            previous: Rect::new(left, y + 1, if arrows { inner.min(2) } else { 0 }, 1),
            next: Rect::new(
                left + right_offset.min(inner as usize) as u16,
                y + 1,
                if arrows {
                    inner.saturating_sub(right_offset as u16).min(2)
                } else {
                    0
                },
                1,
            ),
            location: Rect::new(
                left,
                y + 2,
                inner.min(
                    crate::editor::display::display_text(&target.location)
                        .chars()
                        .count()
                        .min(u16::MAX as usize) as u16,
                ),
                1,
            ),
            source: Rect::new(left, y + 3, inner, height.saturating_sub(4)),
        })
    }
    pub fn render(
        &self,
        context: HookContext<'_>,
        state: &EditorState,
        buffer: &mut Buffer,
        theme: &Theme,
    ) {
        let Some(geometry) = self.geometry(context, state) else {
            return;
        };
        let area = geometry.area.intersection(buffer.area);
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("code-search-bar"));
        if area.is_empty() {
            return;
        }
        buffer.set_stringn(
            area.x,
            area.y,
            "Definition",
            area.width as usize,
            theme.style("definition-title"),
        );
        let inner_width = geometry.area.width.saturating_sub(2) as usize;
        if inner_width > 0 {
            let nav = Rect::new(
                geometry.source.x,
                geometry.area.y + 1,
                inner_width as u16,
                1,
            )
            .intersection(buffer.area);
            if !nav.is_empty() {
                buffer.set_stringn(
                    nav.x,
                    nav.y,
                    self.nav(),
                    nav.width as usize,
                    theme.style("code-search-bar"),
                );
            }
            let target = &self.targets[self.selected];
            let location = geometry.location.intersection(buffer.area);
            if !location.is_empty() {
                buffer.set_stringn(
                    location.x,
                    location.y,
                    crate::editor::display::display_text(&target.location),
                    location.width as usize,
                    theme.style("code-search-bar"),
                );
            }
            match &target.preview {
                Ok(preview) => {
                    for index in 0..preview.lines.min(geometry.source.height as usize) {
                        let line = LineIndex(preview.start.0 + index);
                        let mut byte = preview
                            .snapshot
                            .line_start(line)
                            .expect("validated preview line");
                        let mut column = 0usize;
                        for scalar in preview
                            .snapshot
                            .line_content(line)
                            .expect("validated preview")
                            .chars()
                        {
                            if column >= geometry.source.width as usize {
                                break;
                            }
                            let range = TextRange {
                                start: byte,
                                end: ByteOffset(byte.0 + scalar.len_utf8()),
                            };
                            let glyph = Glyph::for_scalar(scalar);
                            let mut style = theme.style("code-search-bar");
                            for span in &preview.highlights {
                                if span.range.start < range.end && range.start < span.range.end {
                                    style = style
                                        .patch(crate::editor::overlays::rule(theme, &span.class));
                                }
                            }
                            if scalar == '\t' {
                                style = style.patch(theme.style("code-tab"));
                            }
                            let y = geometry.source.y + index as u16;
                            let x = geometry.source.x + column as u16;
                            if buffer.area.contains(Position::new(x, y)) {
                                buffer.set_stringn(
                                    x,
                                    y,
                                    glyph.text(),
                                    geometry.source.width as usize - column,
                                    style,
                                );
                            }
                            column += glyph.width();
                            byte = range.end;
                        }
                    }
                }
                Err(_)
                    if geometry.source.height > 0
                        && buffer
                            .area
                            .contains(Position::new(geometry.source.x, geometry.source.y)) =>
                {
                    buffer.set_stringn(
                        geometry.source.x,
                        geometry.source.y,
                        "source unavailable",
                        geometry.source.width as usize,
                        theme.style("code-search-bar"),
                    );
                }
                Err(_) => {}
            }
        }
        if let Some(control) = self.hovered {
            let rect = match control {
                DefinitionControl::Previous => geometry.previous,
                DefinitionControl::Next => geometry.next,
                DefinitionControl::Location => geometry.location,
            }
            .intersection(buffer.area);
            buffer.set_style(
                rect,
                theme
                    .style("code-search-bar")
                    .add_modifier(Modifier::UNDERLINED),
            );
        }
        let delimiter = Rect::new(
            geometry.area.x,
            geometry.area.bottom() - 1,
            geometry.area.width,
            1,
        )
        .intersection(buffer.area);
        buffer.set_style(delimiter, theme.style("definition-delimiter"));
    }
    pub fn control_at(
        &self,
        geometry: DefinitionGeometry,
        point: Position,
    ) -> Option<DefinitionControl> {
        if geometry.previous.contains(point) {
            Some(DefinitionControl::Previous)
        } else if geometry.next.contains(point) {
            Some(DefinitionControl::Next)
        } else if geometry.location.contains(point) {
            Some(DefinitionControl::Location)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub enum NavigationEffect {
    Run(NavigationJob),
    Navigate(NavigationTarget),
}
struct PendingNavigation {
    job: NavigationJob,
    selection: Selection,
}
struct HoverCandidate {
    identifier: Identifier,
    revision: kode_core::Revision,
    document: DocumentId,
    selection: Selection,
    due: u64,
    fired: bool,
}
pub struct NavigationController {
    pub definition: Option<DefinitionPopup>,
    pub usages: Option<super::usages::UsagePopup>,
    pub error: Option<DomainError>,
    pub enabled: bool,
    declaration_spans: Vec<TextRange>,
    declaration_stamp: Option<(DocumentId, kode_core::Revision)>,
    clock: Arc<dyn Clock>,
    gate: ResultGate,
    project_root: PathBuf,
    pending: Option<PendingNavigation>,
    active_stamp: Option<(DocumentId, kode_core::Revision, Selection)>,
    hover: Option<HoverCandidate>,
    effects: Vec<NavigationEffect>,
}
impl NavigationController {
    pub fn new(
        workspace: WorkspaceGeneration,
        project_root: PathBuf,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            definition: None,
            usages: None,
            error: None,
            enabled: true,
            declaration_spans: vec![],
            declaration_stamp: None,
            clock,
            gate: ResultGate::new(workspace),
            project_root,
            pending: None,
            active_stamp: None,
            hover: None,
            effects: vec![],
        }
    }
    pub fn take_effects(&mut self) -> Vec<NavigationEffect> {
        std::mem::take(&mut self.effects)
    }
    pub fn dismiss(&mut self) {
        self.definition = None;
        self.usages = None;
        self.pending = None;
        self.active_stamp = None;
        self.hover = None;
        self.effects
            .retain(|effect| matches!(effect, NavigationEffect::Navigate(_)));
        self.gate.dismiss("navigation");
        self.error = None;
    }
    pub fn switch_workspace(&mut self) {
        self.dismiss();
        self.gate.switch_workspace();
        self.declaration_spans.clear();
        self.declaration_stamp = None;
    }
    pub fn set_declaration_spans(
        &mut self,
        snapshot: &TextSnapshot,
        spans: Vec<TextRange>,
    ) -> Result<()> {
        for span in &spans {
            snapshot.slice(*span)?;
        }
        self.declaration_spans = spans;
        self.declaration_stamp = Some((snapshot.id, snapshot.revision));
        Ok(())
    }
    pub fn hover_range(&self, snapshot: &TextSnapshot) -> Option<TextRange> {
        self.hover
            .as_ref()
            .filter(|hover| (hover.document, hover.revision) == (snapshot.id, snapshot.revision))
            .map(|hover| hover.identifier.range)
    }
    fn request(
        &mut self,
        snapshot: &TextSnapshot,
        state: &EditorState,
        position: ByteOffset,
        identifier: Identifier,
        operation: NavigationOperation,
    ) {
        let tag = self
            .gate
            .request("navigation".into(), Some((snapshot.id, snapshot.revision)));
        let declaration_hint = self.declaration_stamp == Some((snapshot.id, snapshot.revision))
            && self
                .declaration_spans
                .iter()
                .any(|span| span.start < identifier.range.end && identifier.range.start < span.end);
        let job = NavigationJob {
            tag,
            snapshot: snapshot.clone(),
            operation,
            position,
            identifier,
            declaration_hint,
            project_root: self.project_root.clone(),
        };
        self.pending = Some(PendingNavigation {
            job: job.clone(),
            selection: state.caret.selection,
        });
        self.active_stamp = Some((snapshot.id, snapshot.revision, state.caret.selection));
        self.effects
            .retain(|effect| !matches!(effect, NavigationEffect::Run(_)));
        self.effects.push(NavigationEffect::Run(job));
    }
    pub fn begin(
        &mut self,
        snapshot: &TextSnapshot,
        state: &EditorState,
        position: ByteOffset,
        operation: NavigationOperation,
    ) -> Result<bool> {
        if !self.enabled {
            return Ok(false);
        }
        let Some(identifier) = lexical_identifier(snapshot, position)? else {
            return Ok(false);
        };
        self.dismiss();
        self.request(snapshot, state, position, identifier, operation);
        Ok(true)
    }
    pub fn hover_at(
        &mut self,
        snapshot: &TextSnapshot,
        state: &EditorState,
        position: Option<ByteOffset>,
    ) -> Result<()> {
        if !self.enabled || self.usages.is_some() {
            return Ok(());
        }
        let identifier = position
            .map(|position| lexical_identifier(snapshot, position))
            .transpose()?
            .flatten();
        let Some(identifier) = identifier else {
            self.dismiss();
            return Ok(());
        };
        if self.hover.as_ref().is_some_and(|hover| {
            hover.identifier.range == identifier.range
                && hover.document == snapshot.id
                && hover.revision == snapshot.revision
        }) {
            return Ok(());
        }
        self.dismiss();
        self.hover = Some(HoverCandidate {
            identifier,
            revision: snapshot.revision,
            document: snapshot.id,
            selection: state.caret.selection,
            due: self.clock.now_millis().saturating_add(HOVER_DELAY_MS),
            fired: false,
        });
        Ok(())
    }
    /// Called by the host timer; no additional mouse movement is required.
    pub fn tick(&mut self, snapshot: &TextSnapshot, state: &EditorState) {
        if !self.enabled || state.search_has_focus {
            self.dismiss();
            return;
        }
        let Some(hover) = &mut self.hover else {
            return;
        };
        if (hover.document, hover.revision, hover.selection)
            != (snapshot.id, snapshot.revision, state.caret.selection)
        {
            self.dismiss();
            return;
        }
        if hover.fired || self.clock.now_millis() < hover.due || self.usages.is_some() {
            return;
        }
        hover.fired = true;
        let identifier = hover.identifier.clone();
        self.request(
            snapshot,
            state,
            identifier.range.start,
            identifier,
            NavigationOperation::Hover,
        );
    }
    pub fn complete(
        &mut self,
        tag: JobTag,
        result: Result<NavigationResult>,
        snapshot: &TextSnapshot,
        state: &EditorState,
        owner_active: bool,
    ) -> bool {
        if !owner_active
            || !self.gate.accepts("navigation", tag)
            || self.pending.as_ref().is_none_or(|pending| {
                pending.job.tag != tag
                    || (
                        pending.job.snapshot.id,
                        pending.job.snapshot.revision,
                        pending.selection,
                    ) != (snapshot.id, snapshot.revision, state.caret.selection)
            })
            || state.search_has_focus
        {
            return false;
        }
        let anchor = self
            .pending
            .take()
            .expect("validated request")
            .job
            .identifier
            .range
            .start;
        self.gate.dismiss("navigation");
        match result {
            Ok(NavigationResult::Navigate(target)) => {
                self.dismiss();
                self.effects.push(NavigationEffect::Navigate(target));
            }
            Ok(NavigationResult::Usages(entries)) if !entries.is_empty() => {
                self.definition = None;
                self.usages = Some(super::usages::UsagePopup::new(anchor, entries));
            }
            Ok(NavigationResult::Definitions(targets)) if !targets.is_empty() => {
                self.usages = None;
                self.definition = Some(DefinitionPopup::new(anchor, targets));
            }
            Ok(_) => {}
            Err(error) => {
                self.error = Some(error);
            }
        }
        true
    }
    pub fn render(
        &mut self,
        context: HookContext<'_>,
        state: &EditorState,
        buffer: &mut Buffer,
        theme: &Theme,
    ) {
        if let Some(definition) = &self.definition {
            definition.render(context, state, buffer, theme);
        }
        if let Some(usages) = &mut self.usages {
            usages.render(context, state, buffer, theme);
        }
    }
}
impl EditorInputHooks for NavigationController {
    fn context_changed(&mut self, snapshot: &TextSnapshot, state: &EditorState) {
        if self.pending.as_ref().is_some_and(|pending| {
            (
                pending.job.snapshot.id,
                pending.job.snapshot.revision,
                pending.selection,
            ) != (snapshot.id, snapshot.revision, state.caret.selection)
        }) || self
            .active_stamp
            .is_some_and(|stamp| stamp != (snapshot.id, snapshot.revision, state.caret.selection))
            || self.hover.as_ref().is_some_and(|hover| {
                (hover.document, hover.revision, hover.selection)
                    != (snapshot.id, snapshot.revision, state.caret.selection)
            })
            || state.search_has_focus
        {
            self.dismiss();
        }
    }
    fn dispatch(
        &mut self,
        target: InputTarget,
        event: &InputEvent,
        document: &mut Document,
        state: &mut EditorState,
        context: HookContext<'_>,
    ) -> Result<bool> {
        if matches!(event, InputEvent::Focus(false)) {
            self.dismiss();
            return Ok(false);
        }
        if !self.enabled {
            return Ok(false);
        }
        self.context_changed(&document.snapshot(), state);
        if target == InputTarget::Navigation {
            if let InputEvent::Pointer(pointer) = event
                && let EditorHit::Text(position) = context.geometry.hit_test(
                    context.layout,
                    state.scroll_rows,
                    pointer.x,
                    pointer.y,
                )
            {
                return self.begin(
                    &document.snapshot(),
                    state,
                    position,
                    NavigationOperation::Click,
                );
            }
            return Ok(false);
        }
        if target != InputTarget::Popup {
            return Ok(false);
        }
        if let Some(usages) = &mut self.usages {
            match usages.dispatch(event, context, state) {
                super::usages::UsageAction::Consumed => {}
                super::usages::UsageAction::Dismissed => self.dismiss(),
                super::usages::UsageAction::Navigate(target) => {
                    self.dismiss();
                    self.effects.push(NavigationEffect::Navigate(target));
                }
            }
            return Ok(true);
        }
        if matches!(event,InputEvent::Key(input) if input.key == Key::Escape)
            && (self.definition.is_some() || self.pending.is_some() || self.hover.is_some())
        {
            self.dismiss();
            return Ok(true);
        }
        if let InputEvent::Pointer(pointer) = event {
            let point = Position::new(pointer.x, pointer.y);
            if let Some(definition) = &mut self.definition
                && let Some(geometry) = definition.geometry(context, state)
            {
                if geometry.area.contains(point) {
                    definition.hovered = definition.control_at(geometry, point);
                    if pointer.action == PointerAction::Down(MouseButton::Left) {
                        match definition.hovered {
                            Some(DefinitionControl::Previous) => {
                                definition.selected = definition.selected.saturating_sub(1)
                            }
                            Some(DefinitionControl::Next) => {
                                definition.selected =
                                    (definition.selected + 1).min(definition.targets.len() - 1)
                            }
                            Some(DefinitionControl::Location) => {
                                let target = definition.targets[definition.selected].target.clone();
                                self.dismiss();
                                self.effects.push(NavigationEffect::Navigate(target));
                            }
                            _ => {}
                        }
                    }
                    return Ok(true);
                }
                if matches!(pointer.action, PointerAction::Down(_)) {
                    self.dismiss();
                    return Ok(true);
                }
                if matches!(pointer.action, PointerAction::Scroll { .. }) {
                    self.dismiss();
                    return Ok(false);
                }
            }
            if matches!(pointer.action, PointerAction::Down(_)) {
                self.dismiss();
            }
            if pointer.action == PointerAction::Move {
                let position = match context.geometry.hit_test(
                    context.layout,
                    state.scroll_rows,
                    pointer.x,
                    pointer.y,
                ) {
                    EditorHit::Text(position) => Some(position),
                    _ => None,
                };
                self.hover_at(&document.snapshot(), state, position)?;
            }
        }
        Ok(false)
    }
}
