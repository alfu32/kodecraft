//! General path chooser over the existing worker-backed folder tree.
use super::{
    controls::draw_text,
    field::{FieldAction, TextField},
    folder_picker::{FolderPicker, PickerWork},
};
use crate::{
    editor::display::display_text,
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    layout::centered,
    theme::Theme,
};
use crossterm::event::MouseButton;
use kode_core::{DomainError, JobTag, RequestId, Result, WorkspaceGeneration};
use kode_platform::jobs::CancellationToken;
use kode_workspace::{
    project::{Project, ProjectConfig, lexical_path},
    tree::{EntryKind, TreeJob, TreeSnapshot},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Block, Borders, Clear, Widget},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    OpenFile,
    SaveFile,
    Directory,
    AnyExisting,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: RequestId,
    pub title: String,
    pub mode: Mode,
    pub value: String,
}
impl Request {
    pub fn new(title: String, mode: Mode, value: String) -> Result<Self> {
        Ok(Self {
            id: RequestId::fresh()?,
            title,
            mode,
            value,
        })
    }
}
pub struct Prepared {
    pub project: Project,
    pub selected: PathBuf,
    pub value: String,
}
fn io(operation: &'static str, path: &Path, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
fn check(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(DomainError::Cancelled)
    } else {
        Ok(())
    }
}
/// Worker only. No project/session configuration is loaded to browse another folder.
pub fn prepare(
    base: &Path,
    value: &str,
    generation: WorkspaceGeneration,
    cancel: &CancellationToken,
) -> Result<Prepared> {
    check(cancel)?;
    let target = if value.is_empty() {
        base.to_path_buf()
    } else {
        base.join(value)
    };
    let root = target
        .ancestors()
        .last()
        .filter(|root| root.is_absolute())
        .ok_or_else(|| {
            DomainError::Unsupported("A path chooser needs an absolute base directory".into())
        })?;
    let target = lexical_path(root, &target)?;
    let mut directory = target.clone();
    loop {
        check(cancel)?;
        match fs::metadata(&directory) {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io("inspect picker directory", &directory, error)),
        }
        if !directory.pop() {
            return Err(DomainError::Unsupported(
                "No existing parent directory for this path".into(),
            ));
        }
    }
    let directory = fs::canonicalize(&directory)
        .map_err(|error| io("resolve picker directory", &directory, error))?;
    if directory.to_str().is_none() {
        return Err(DomainError::Unsupported(
            "Picker directory is not UTF-8".into(),
        ));
    }
    let root = directory
        .ancestors()
        .last()
        .expect("canonical absolute directory");
    let project = Project::configured(root, generation, ProjectConfig::default())?;
    check(cancel)?;
    Ok(Prepared {
        project,
        selected: directory,
        value: if value.is_empty() {
            String::new()
        } else {
            target
                .to_str()
                .ok_or_else(|| DomainError::Unsupported("Selected path is not UTF-8".into()))?
                .into()
        },
    })
}
/// Worker only; choosing never creates, overwrites or reads the contents of a file.
pub fn validate(mode: Mode, path: &Path, cancel: &CancellationToken) -> Result<PathBuf> {
    check(cancel)?;
    if !path.is_absolute() || path.to_str().is_none() || path.as_os_str().is_empty() {
        return Err(DomainError::Unsupported(
            "Choose an absolute UTF-8 path".into(),
        ));
    }
    let metadata = fs::metadata(path);
    match mode {
        Mode::OpenFile => {
            if !metadata
                .map_err(|error| io("inspect selected file", path, error))?
                .is_file()
            {
                return Err(DomainError::Unsupported(
                    "Choose an existing regular file".into(),
                ));
            }
        }
        Mode::Directory => {
            if !metadata
                .map_err(|error| io("inspect selected folder", path, error))?
                .is_dir()
            {
                return Err(DomainError::Unsupported(
                    "Choose an existing directory".into(),
                ));
            }
        }
        Mode::AnyExisting => {
            let metadata = metadata.map_err(|error| io("inspect selected path", path, error))?;
            if !metadata.is_file() && !metadata.is_dir() {
                return Err(DomainError::Unsupported(
                    "Choose a regular file or directory".into(),
                ));
            }
        }
        Mode::SaveFile => {
            match metadata {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    return Err(DomainError::Unsupported(
                        "Save target must be a regular file".into(),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io("inspect save target", path, error)),
            }
            let parent = path
                .parent()
                .ok_or_else(|| DomainError::Unsupported("Enter a file name".into()))?;
            if !fs::metadata(parent)
                .map_err(|error| io("inspect save directory", parent, error))?
                .is_dir()
            {
                return Err(DomainError::Unsupported(
                    "Save target needs an existing parent directory".into(),
                ));
            }
        }
    }
    check(cancel)?;
    Ok(path.into())
}
pub enum Work {
    Tree(TreeJob),
    Navigate {
        tag: JobTag,
        base: PathBuf,
        value: String,
        cancel: CancellationToken,
    },
    Validate {
        tag: JobTag,
        mode: Mode,
        path: PathBuf,
        cancel: CancellationToken,
    },
}
impl Work {
    pub fn tag(&self) -> JobTag {
        match self {
            Self::Tree(job) => job.tag,
            Self::Navigate { tag, .. } | Self::Validate { tag, .. } => *tag,
        }
    }
    pub fn run(self, cancel: &CancellationToken) -> Result<Report> {
        match self {
            Self::Tree(job) => Ok(Report::Tree(kode_workspace::tree::read_tree(&job, cancel))),
            Self::Navigate {
                tag,
                base,
                value,
                cancel: local,
            } => {
                check(&local)?;
                let result = prepare(&base, &value, tag.workspace, cancel)?;
                check(&local)?;
                Ok(Report::Prepared(result))
            }
            Self::Validate {
                mode,
                path,
                cancel: local,
                ..
            } => {
                check(&local)?;
                let result = validate(mode, &path, cancel)?;
                check(&local)?;
                Ok(Report::Chosen(result))
            }
        }
    }
}
pub enum Report {
    Tree(Result<TreeSnapshot>),
    Prepared(Prepared),
    Chosen(PathBuf),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    Navigate,
    Validate,
}
struct Pending {
    tag: JobTag,
    operation: Operation,
    focus: usize,
    cursor: usize,
    anchor: Option<usize>,
    filename: Option<String>,
    value: String,
    selected: PathBuf,
    cancel: CancellationToken,
}
#[derive(Default, Debug, Clone, Copy)]
pub struct Geometry {
    pub area: Rect,
    pub parent: Rect,
    pub field: Rect,
    pub go: Rect,
    pub rows: Rect,
    pub confirm: Rect,
    pub cancel: Rect,
    pub status: Rect,
}
pub struct FilePicker {
    pub request: Request,
    pub browser: FolderPicker,
    pub field: TextField,
    pub id: FocusId,
    pub finished: bool,
    pub result: Option<PathBuf>,
    pub error: Option<String>,
    pub geometry: Geometry,
    pub focus: usize, // tree, field, parent, go, select, cancel
    previous_focus: Option<FocusId>,
    directory: PathBuf,
    generation: WorkspaceGeneration,
    pending: Option<Pending>,
    work: Vec<Work>,
    field_action: Option<FieldAction>,
}
impl FilePicker {
    pub fn new(request: Request, prepared: Prepared, focus: &mut FocusState) -> Result<Self> {
        let previous_focus = focus.focused;
        let generation = prepared.project.generation;
        let mut browser = FolderPicker::new(
            prepared.project,
            "Paths".into(),
            request.mode == Mode::Directory,
            focus,
        )?;
        let id = browser.id;
        browser.take_work(); // the focused-path refresh supersedes the unopened root refresh
        browser.tree.open_directory(&prepared.selected)?;
        browser.focus_path(prepared.selected.clone())?;
        let mut field = TextField::new(if prepared.value.is_empty() {
            prepared
                .selected
                .to_str()
                .ok_or_else(|| DomainError::Unsupported("Picker directory is not UTF-8".into()))?
                .into()
        } else {
            prepared.value
        });
        field.framed = false;
        Ok(Self {
            request,
            browser,
            field,
            id,
            finished: false,
            result: None,
            error: None,
            geometry: Geometry::default(),
            focus: 0,
            previous_focus,
            directory: prepared.selected,
            generation,
            pending: None,
            work: vec![],
            field_action: None,
        })
    }
    pub fn take_work(&mut self) -> Vec<Work> {
        for work in self.browser.take_work() {
            if let PickerWork::Refresh(job) = work {
                self.work.push(Work::Tree(job));
            }
        }
        std::mem::take(&mut self.work)
    }
    fn invalidate(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.cancel.cancel();
        }
        self.work.retain(|work| matches!(work, Work::Tree(_)));
    }
    fn kind(&self) -> Option<EntryKind> {
        if self.browser.selected == self.browser.tree.root {
            return Some(EntryKind::Directory);
        }
        self.browser
            .tree
            .flattened()
            .into_iter()
            .find(|entry| entry.entry.path == self.browser.selected)
            .map(|entry| entry.entry.kind)
    }
    fn current_path(&self) -> PathBuf {
        self.directory.join(&self.field.value)
    }
    fn begin(&mut self, operation: Operation, value: String) -> Result<()> {
        self.invalidate();
        self.error = None;
        if operation == Operation::Validate && self.browser.selected.to_str().is_none() {
            self.error = Some("Selected path is not UTF-8".into());
            return Ok(());
        }
        let tag = JobTag {
            workspace: self.generation,
            request: RequestId::fresh()?,
            document: None,
        };
        let cancel = CancellationToken::new();
        let filename = (operation == Operation::Navigate
            && value != self.field.value
            && self.request.mode == Mode::SaveFile)
            .then(|| {
                Path::new(&self.field.value)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .flatten();
        self.work.push(match operation {
            Operation::Navigate => Work::Navigate {
                tag,
                base: self.directory.clone(),
                value,
                cancel: cancel.clone(),
            },
            Operation::Validate => Work::Validate {
                tag,
                mode: self.request.mode,
                path: self.current_path(),
                cancel: cancel.clone(),
            },
        });
        self.pending = Some(Pending {
            tag,
            operation,
            focus: self.focus,
            cursor: self.field.cursor,
            anchor: self.field.anchor,
            filename,
            value: self.field.value.clone(),
            selected: self.browser.selected.clone(),
            cancel,
        });
        Ok(())
    }
    pub fn cancel(&mut self, focus: &mut FocusState) {
        self.result = None;
        self.finish(focus);
    }
    pub fn suspend(&mut self) {
        self.invalidate();
    }
    fn finish(&mut self, focus: &mut FocusState) {
        self.invalidate();
        self.finished = true;
        if focus.focused == Some(self.id) {
            focus.focused = self.previous_focus;
        }
        if focus.captured == Some(self.id) {
            focus.captured = None;
        }
    }
    pub fn complete(
        &mut self,
        tag: JobTag,
        result: Result<Report>,
        focus: &mut FocusState,
    ) -> Result<bool> {
        if self.finished || tag.workspace != self.generation {
            return Ok(false);
        }
        // Tree publication has its own exact refresh request gate and never changes typed input.
        if matches!(&result, Ok(Report::Tree(_))) {
            let selected = self.browser.selected.clone();
            let accepted = self.browser.complete(
                tag,
                result.and_then(|report| match report {
                    Report::Tree(tree) => tree,
                    _ => unreachable!(),
                }),
            );
            self.browser.selected = selected;
            return Ok(accepted);
        }
        let Some(pending) = &self.pending else {
            return Ok(false);
        };
        if pending.tag != tag {
            return Ok(false);
        }
        if pending.value != self.field.value
            || pending.focus != self.focus
            || pending.cursor != self.field.cursor
            || pending.anchor != self.field.anchor
            || pending.selected != self.browser.selected
            || pending.cancel.is_cancelled()
            || focus.focused != Some(self.id)
        {
            self.invalidate();
            return Ok(false);
        }
        let operation = pending.operation;
        let filename = pending.filename.clone();
        self.pending = None;
        match result {
            Ok(Report::Prepared(mut prepared)) if operation == Operation::Navigate => {
                if let Some(name) = filename {
                    prepared.value = prepared
                        .selected
                        .join(name)
                        .to_str()
                        .ok_or_else(|| {
                            DomainError::Unsupported("Selected path is not UTF-8".into())
                        })?
                        .into();
                }
                let previous = self.previous_focus;
                let mut replacement = Self::new(self.request.clone(), prepared, focus)?;
                replacement.id = self.id;
                replacement.browser.id = self.id;
                replacement.previous_focus = previous;
                focus.focused = Some(self.id);
                *self = replacement;
            }
            Ok(Report::Chosen(path)) if operation == Operation::Validate => {
                self.result = Some(path);
                self.finish(focus);
            }
            Err(DomainError::Cancelled) => return Ok(false),
            Err(error) => self.error = Some(error.to_string()),
            _ => self.error = Some("Unexpected path chooser result".into()),
        }
        Ok(true)
    }
    pub fn take_field_action(&mut self) -> Option<FieldAction> {
        self.field_action.take()
    }
    pub fn paste(
        &mut self,
        value: &str,
        cursor: usize,
        anchor: Option<usize>,
        text: String,
    ) -> bool {
        if self.finished
            || self.focus != 1
            || self.field.value != value
            || self.field.cursor != cursor
            || self.field.anchor != anchor
        {
            return false;
        }
        self.field.handle(&InputEvent::Paste(text));
        self.invalidate();
        true
    }
    fn set_value(&mut self, path: &Path) -> bool {
        let Some(value) = path.to_str() else {
            self.error = Some("Selected path is not UTF-8".into());
            return false;
        };
        self.field.value = value.into();
        self.field.cursor = self.field.value.len();
        self.field.anchor = None;
        true
    }
    fn selection_changed(&mut self) {
        self.invalidate();
        self.error = None;
        let path = self.browser.selected.clone();
        if path.to_str().is_none() {
            self.error = Some("Selected path is not UTF-8".into());
            return;
        }
        if self.kind() == Some(EntryKind::Directory) {
            let target = if self.request.mode == Mode::SaveFile {
                Path::new(&self.field.value)
                    .file_name()
                    .map(|name| path.join(name))
                    .unwrap_or_else(|| path.clone())
            } else {
                path.clone()
            };
            self.directory = path;
            self.set_value(&target);
        } else {
            if let Some(parent) = path.parent() {
                self.directory = parent.into();
            }
            self.set_value(&path);
        }
    }
    fn parent(&mut self) -> Result<()> {
        let parent = self.directory.parent().unwrap_or(&self.directory);
        let value = parent
            .to_str()
            .ok_or_else(|| DomainError::Unsupported("Picker directory is not UTF-8".into()))?
            .into();
        self.begin(Operation::Navigate, value)
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> Result<bool> {
        if self.finished {
            return Ok(false);
        }
        if let InputEvent::Key(input) = event {
            match input.key {
                Key::Escape => {
                    self.finish(focus);
                    return Ok(true);
                }
                Key::Tab => {
                    self.focus = (self.focus + if input.modifiers.shift { 5 } else { 1 }) % 6;
                    return Ok(true);
                }
                Key::Function(4) => {
                    self.parent()?;
                    return Ok(true);
                }
                Key::Enter => {
                    match self.focus {
                        1 | 3 => self.begin(Operation::Navigate, self.field.value.clone())?,
                        2 => self.parent()?,
                        5 => self.finish(focus),
                        0 if matches!(self.request.mode, Mode::OpenFile | Mode::SaveFile)
                            && self.kind() == Some(EntryKind::Directory) =>
                        {
                            if let Some(path) = self.browser.selected.to_str() {
                                self.begin(Operation::Navigate, path.into())?;
                            } else {
                                self.error = Some("Selected path is not UTF-8".into());
                            }
                        }
                        _ => self.begin(Operation::Validate, String::new())?,
                    }
                    return Ok(true);
                }
                _ => {}
            }
        }
        if let InputEvent::Pointer(pointer) = event
            && pointer.action == PointerAction::Down(MouseButton::Left)
        {
            let point = Position::new(pointer.x, pointer.y);
            if self.geometry.cancel.contains(point) {
                self.finish(focus);
                return Ok(true);
            }
            if self.geometry.confirm.contains(point) {
                self.focus = 4;
                self.begin(Operation::Validate, String::new())?;
                return Ok(true);
            }
            if self.geometry.parent.contains(point) {
                self.focus = 2;
                self.parent()?;
                return Ok(true);
            }
            if self.geometry.go.contains(point) {
                self.focus = 3;
                self.begin(Operation::Navigate, self.field.value.clone())?;
                return Ok(true);
            }
            if self
                .field
                .hit_test(self.geometry.field, pointer.x, pointer.y)
            {
                self.focus = 1;
                return Ok(true);
            }
            if self.geometry.rows.contains(point) {
                self.focus = 0;
            }
        }
        if self.focus == 1 && !matches!(event, InputEvent::Pointer(_)) {
            let before = self.field.value.clone();
            self.field_action = self.field.handle(event);
            if before != self.field.value {
                self.invalidate();
                self.error = None;
            }
        } else if self.focus == 0 {
            if self.kind().is_none() && matches!(event, InputEvent::Key(_)) {
                self.error = Some(
                    "Tree selection unavailable; Go reloads the path, F4 opens its parent".into(),
                );
                return Ok(true);
            }
            let before = self.browser.selected.clone();
            self.browser.dispatch(event, focus)?;
            if before != self.browser.selected {
                self.selection_changed();
            }
        }
        Ok(true)
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) -> Option<Position> {
        let area = centered(area.intersection(buffer.area), 90, 26);
        self.geometry = Geometry {
            area,
            ..Default::default()
        };
        if self.finished || area.is_empty() {
            return None;
        }
        Clear.render(area, buffer);
        let inner = if area.width >= 4 && area.height >= 4 {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(display_text(&self.request.title))
                .style(theme.style("project-search-dialog"));
            let inner = block.inner(area);
            block.render(area, buffer);
            inner
        } else {
            area
        };
        if inner.is_empty() {
            return None;
        }
        let mut g = Geometry {
            area,
            ..Default::default()
        };
        if inner.height <= 3 || inner.width < 12 {
            match self.focus {
                0 => g.rows = inner,
                1 => g.field = inner,
                2 => g.parent = inner,
                3 => g.go = inner,
                4 => g.confirm = inner,
                _ => g.cancel = inner,
            }
        } else {
            let parent_width = inner.width.min(8);
            g.parent = Rect::new(inner.x, inner.y, parent_width, 1);
            g.status = Rect::new(
                inner.x + parent_width,
                inner.y,
                inner.width - parent_width,
                1,
            );
            if inner.height >= 3 {
                g.field = Rect::new(inner.x, inner.y + 1, inner.width.saturating_sub(5), 1);
                g.go = Rect::new(g.field.right(), inner.y + 1, inner.width - g.field.width, 1);
                g.rows = Rect::new(
                    inner.x,
                    inner.y + 2,
                    inner.width,
                    inner.height.saturating_sub(3),
                );
            } else if self.focus == 1 {
                g.field = Rect::new(inner.x, inner.y, inner.width, 1);
                g.parent = Rect::default();
                g.status = Rect::default();
            }
            let half = inner.width / 2;
            g.confirm = Rect::new(
                inner.x,
                inner.bottom() - 1,
                half.max(u16::from(self.focus != 5)),
                1,
            );
            g.cancel = Rect::new(
                g.confirm.right(),
                inner.bottom() - 1,
                inner.width - g.confirm.width,
                1,
            );
        }
        self.geometry = g;
        for (rect, label, control) in [
            (g.parent, "[Up F4]", 2),
            (g.go, "[Go]", 3),
            (g.confirm, "[Select]", 4),
            (g.cancel, "[Cancel]", 5),
        ] {
            draw_text(
                rect,
                buffer,
                label,
                theme.style(if self.focus == control {
                    "file-entry:selected"
                } else {
                    "lsp-button"
                }),
            );
        }
        let status = self
            .error
            .as_deref()
            .or(self.browser.error.as_deref())
            .unwrap_or_else(|| {
                if self.pending.is_some() {
                    "Checking path…"
                } else {
                    "Tab: controls; Enter: open/select; ←/→: expand; F4: parent"
                }
            });
        draw_text(
            g.status,
            buffer,
            &display_text(status),
            theme.style("project-search-dialog"),
        );
        self.browser.render_rows(g.rows, buffer, theme);
        self.field.render(g.field, buffer, theme, self.focus == 1)
    }
}
impl Drop for FilePicker {
    fn drop(&mut self) {
        self.invalidate();
    }
}
