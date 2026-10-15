//! Application-thread Git read reducer. Refresh work contains only owned inputs;
//! rendering never opens a repository or requests blob content.
use crate::{
    app::events::ResultGate, editor::display::display_text, focus::FocusId, theme::Theme,
    widgets::controls::draw_text,
};
use kode_core::{DomainError, JobTag, RequestId, Result, WorkspaceGeneration};
use kode_git::{
    model::RepositorySnapshot,
    read::{ReadJob, ReadOutput, ReadRequest},
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::path::PathBuf;
pub struct GitReadPanel {
    pub id: FocusId,
    pub root: PathBuf,
    pub snapshot: Option<RepositorySnapshot>,
    pub error: Option<String>,
    pub busy: Option<JobTag>,
    pub selected_status: usize,
    pub status_scroll: usize,
    pub history_scroll: usize,
    gate: ResultGate,
}
impl GitReadPanel {
    pub fn new(root: PathBuf, workspace: WorkspaceGeneration) -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            root,
            snapshot: None,
            error: None,
            busy: None,
            selected_status: 0,
            status_scroll: 0,
            history_scroll: 0,
            gate: ResultGate::new(workspace),
        })
    }
    pub fn begin_refresh(&mut self) -> Option<(JobTag, ReadJob)> {
        if self.busy.is_some() {
            return None;
        }
        let tag = self.gate.request("git-read".into(), None);
        self.busy = Some(tag);
        Some((
            tag,
            ReadJob::new(
                self.root.clone(),
                ReadRequest::Snapshot {
                    history_limit: None,
                },
            ),
        ))
    }
    pub fn complete_refresh(&mut self, tag: JobTag, result: Result<ReadOutput>) -> bool {
        if self.busy != Some(tag) || !self.gate.accepts("git-read", tag) {
            return false;
        }
        self.busy = None;
        let result = result.and_then(|output| match output {
            ReadOutput::Snapshot(snapshot) => Ok(*snapshot),
            _ => Err(DomainError::Unsupported("wrong Git refresh result".into())),
        });
        match result {
            Ok(snapshot) => {
                let selected = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.status.entries.get(self.selected_status))
                    .map(|entry| (entry.path.clone(), entry.layer));
                self.selected_status = selected
                    .and_then(|selected| {
                        snapshot
                            .status
                            .entries
                            .iter()
                            .position(|entry| (entry.path.clone(), entry.layer) == selected)
                    })
                    .unwrap_or(
                        self.selected_status
                            .min(snapshot.status.entries.len().saturating_sub(1)),
                    );
                self.status_scroll = self
                    .status_scroll
                    .min(snapshot.status.entries.len().saturating_sub(1));
                self.history_scroll = self
                    .history_scroll
                    .min(snapshot.commits.entries.len().saturating_sub(1));
                self.error = (!snapshot.errors.is_empty()).then(|| snapshot.errors.join(" | "));
                self.snapshot = Some(snapshot);
            }
            Err(DomainError::Cancelled) => {}
            Err(error) => self.error = Some(error.to_string()),
        }
        true
    }
    pub fn cancel_refresh(&mut self) {
        self.gate.dismiss("git-read");
        self.busy = None;
    }
    pub fn set_root(&mut self, root: PathBuf, workspace: WorkspaceGeneration) {
        self.root = root;
        self.gate = ResultGate::new(workspace);
        self.busy = None;
        self.snapshot = None;
        self.error = None;
        self.selected_status = 0;
        self.status_scroll = 0;
        self.history_scroll = 0;
    }
    pub fn render_status(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let area = area.intersection(buffer.area);
        let normal = theme.style("file-entry");
        for y in area.y..area.bottom() {
            draw_text(Rect::new(area.x, y, area.width, 1), buffer, "", normal);
        }
        if area.is_empty() {
            return;
        }
        let branch = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.head.as_ref())
            .map_or("unavailable", |head| head.label());
        let suffix = format!(
            "{}{}",
            if self.busy.is_some() {
                " | refreshing"
            } else {
                ""
            },
            if self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.status.truncated)
            {
                " | status limited"
            } else {
                ""
            }
        );
        draw_text(
            Rect::new(area.x, area.y, area.width, 1),
            buffer,
            &display_text(&format!("Branch: {branch}{suffix}")),
            normal,
        );
        let mut row = 1;
        if let Some(error) = &self.error
            && row < area.height
        {
            draw_text(
                Rect::new(area.x, area.y + row, area.width, 1),
                buffer,
                &display_text(error),
                theme.style("error"),
            );
            row += 1;
        }
        if let Some(snapshot) = &self.snapshot {
            for (index, entry) in snapshot
                .status
                .entries
                .iter()
                .enumerate()
                .skip(self.status_scroll)
                .take(area.height.saturating_sub(row) as usize)
            {
                let marker = if entry.staged() { "[S]" } else { "[ ]" };
                let text = display_text(&format!(
                    "{marker} {:<3} {}",
                    entry.code,
                    entry.path.to_string_lossy()
                ));
                let style = if index == self.selected_status {
                    theme.style("file-entry:selected")
                } else {
                    normal
                };
                draw_text(
                    Rect::new(area.x, area.y + row, area.width, 1),
                    buffer,
                    &text,
                    style,
                );
                row += 1;
            }
        }
    }
    pub fn render_history(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let area = area.intersection(buffer.area);
        let style = theme.style("file-entry");
        for y in area.y..area.bottom() {
            draw_text(Rect::new(area.x, y, area.width, 1), buffer, "", style);
        }
        if area.is_empty() {
            return;
        }
        let truncated = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.commits.truncated);
        draw_text(
            Rect::new(area.x, area.y, area.width, 1),
            buffer,
            if truncated {
                "Commits (limited)"
            } else {
                "Commits"
            },
            style,
        );
        if let Some(snapshot) = &self.snapshot {
            for (row, commit) in snapshot
                .commits
                .entries
                .iter()
                .skip(self.history_scroll)
                .take(area.height.saturating_sub(1) as usize)
                .enumerate()
            {
                let message = commit.message.lines().next().unwrap_or("");
                let tag = commit
                    .tag
                    .as_ref()
                    .map_or(String::new(), |tag| format!(" [{tag}]"));
                draw_text(
                    Rect::new(area.x, area.y + row as u16 + 1, area.width, 1),
                    buffer,
                    &display_text(&format!("{message}{tag}")),
                    style,
                );
            }
        }
    }
}
