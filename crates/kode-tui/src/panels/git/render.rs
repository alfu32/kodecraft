use super::*;
use crate::{editor::display::display_text, layout::centered, widgets::controls::draw_text};
use ratatui::widgets::{Block, Borders, Clear, Widget};
impl GitPanel {
    pub fn layout(&self, area: Rect) -> GitGeometry {
        let toolbar = Rect::new(area.x, area.y, area.width, area.height.min(1));
        let notice = Rect::new(
            area.x,
            area.bottom().saturating_sub(1),
            area.width,
            u16::from(area.height > 1),
        );
        let available = area
            .height
            .saturating_sub(toolbar.height)
            .saturating_sub(notice.height);
        let divider = u16::from(available >= 5);
        let usable = available.saturating_sub(divider * 2);
        let minimum = if usable >= 6 { 2 } else { 0 };
        let status_h = self
            .status_height
            .unwrap_or(usable / 3)
            .max(minimum)
            .min(usable.saturating_sub(minimum * 2));
        let remaining = usable.saturating_sub(status_h);
        let message_h = self
            .message_height
            .unwrap_or(remaining / 2)
            .max(minimum)
            .min(remaining.saturating_sub(minimum));
        let status = Rect::new(area.x, toolbar.bottom(), area.width, status_h);
        let first_divider = Rect::new(area.x, status.bottom(), area.width, divider);
        let message_top = first_divider.bottom();
        let commit_h = message_h.min(1);
        let editor = Rect::new(
            area.x,
            message_top,
            area.width,
            message_h.saturating_sub(commit_h),
        );
        let commit = Rect::new(area.x, editor.bottom(), area.width, commit_h);
        let second_divider = Rect::new(area.x, commit.bottom(), area.width, divider);
        let history = Rect::new(
            area.x,
            second_divider.bottom(),
            area.width,
            remaining.saturating_sub(message_h),
        );
        GitGeometry {
            area,
            toolbar,
            status,
            first_divider,
            editor,
            commit,
            second_divider,
            history,
            notice,
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
        let area = area.intersection(buffer.area);
        self.geometry = self.layout(area);
        self.init_button = Rect::default();
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("file-entry"));
        draw_text(
            self.geometry.toolbar,
            buffer,
            "[r] [b] [t] [T]",
            theme.style("file-entry"),
        );
        if self.repo_available == Some(false) {
            let status = self.geometry.status;
            if !status.is_empty() {
                draw_text(
                    Rect::new(status.x, status.y, status.width, 1),
                    buffer,
                    "Not a Git repository",
                    theme.style("file-entry"),
                );
                if status.height > 1 {
                    draw_text(
                        Rect::new(status.x, status.y + 1, status.width, 1),
                        buffer,
                        &display_text(&self.read.root.to_string_lossy()),
                        theme.style("file-entry"),
                    );
                }
                let y = status.y + status.height.saturating_sub(1).min(2);
                self.init_button = Rect::new(status.x, y, status.width.min(12), 1);
                draw_text(
                    self.init_button,
                    buffer,
                    "[ git init ]",
                    theme.style("button"),
                );
            }
        } else {
            self.read.render_status(self.geometry.status, buffer, theme);
        }
        for divider in [self.geometry.first_divider, self.geometry.second_divider] {
            draw_text(
                divider,
                buffer,
                &"─".repeat(divider.width as usize),
                theme.style("file-entry"),
            );
        }
        let mut caret = None;
        if let Some(view) = &mut self.commit_editor {
            caret = view.render(documents, focus, self.geometry.editor, buffer, theme, &[])?;
        }
        draw_text(
            self.geometry.commit,
            buffer,
            if self.write_busy.is_some() {
                "[ Working… ]"
            } else {
                "[ Commit ] Ctrl+Enter"
            },
            theme.style(if self.can_write() {
                "button"
            } else {
                "file-entry"
            }),
        );
        let history = self.geometry.history;
        let limited = self
            .read
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.commits.truncated)
            || self.files.values().any(|files| files.truncated);
        if !history.is_empty() {
            draw_text(
                Rect::new(history.x, history.y, history.width, 1),
                buffer,
                if limited {
                    "Commits (limited)"
                } else {
                    "Commits"
                },
                theme.style("file-entry"),
            );
        }
        let rows = self.history_rows();
        self.read.history_scroll = self.read.history_scroll.min(
            rows.len()
                .saturating_sub(history.height.saturating_sub(1) as usize),
        );
        if let Some(snapshot) = &self.read.snapshot {
            for (line, (index, row)) in rows
                .iter()
                .enumerate()
                .skip(self.read.history_scroll)
                .take(history.height.saturating_sub(1) as usize)
                .enumerate()
            {
                let text = if let Some(path) = &row.path {
                    format!(" - {}", path.to_string_lossy())
                } else if let Some(commit) = snapshot
                    .commits
                    .entries
                    .iter()
                    .find(|commit| commit.hash == row.commit)
                {
                    let marker = if self.expanded.contains(&row.commit) {
                        "[-]"
                    } else {
                        "[+]"
                    };
                    let tag = commit
                        .tag
                        .as_ref()
                        .map_or(String::new(), |tag| format!(" [{tag}]"));
                    format!(
                        "{marker} {}{tag}",
                        commit.message.lines().next().unwrap_or("")
                    )
                } else {
                    String::new()
                };
                let style = if index == self.history_selected && self.selected_commit.is_some() {
                    theme.style("file-entry:selected")
                } else {
                    theme.style("file-entry")
                };
                draw_text(
                    Rect::new(history.x, history.y + line as u16 + 1, history.width, 1),
                    buffer,
                    &display_text(&text),
                    style,
                );
            }
        }
        draw_text(
            self.geometry.notice,
            buffer,
            &display_text(if self.message.is_empty() {
                "Tab/Ctrl+Tab sections"
            } else {
                &self.message
            }),
            theme.style("file-entry"),
        );
        Ok(caret.take())
    }
    pub fn render_modal(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
    ) -> Option<Position> {
        self.modal_viewport = area.intersection(buffer.area);
        match &mut self.menu {
            Some(Menu::Tag { dialog, .. }) => dialog.render(area, buffer, theme),
            Some(Menu::Branches {
                snapshot,
                selected,
                scroll,
            }) => {
                let branches = &snapshot.branches.entries;
                let modal = centered(self.modal_viewport, 56, (branches.len() + 2).min(18) as u16);
                Clear.render(modal, buffer);
                Block::default()
                    .title("Checkout branch")
                    .borders(Borders::ALL)
                    .style(theme.style("project-search-dialog"))
                    .render(modal, buffer);
                let visible = modal.height.saturating_sub(2) as usize;
                if *selected < *scroll {
                    *scroll = *selected;
                } else if *selected >= *scroll + visible.max(1) {
                    *scroll = selected.saturating_sub(visible.saturating_sub(1));
                }
                for (line, (index, branch)) in branches
                    .iter()
                    .enumerate()
                    .skip(*scroll)
                    .take(visible)
                    .enumerate()
                {
                    let text = display_text(&format!(
                        "{} {}",
                        if branch.current { "*" } else { " " },
                        branch.name
                    ));
                    draw_text(
                        Rect::new(
                            modal.x.saturating_add(1),
                            modal.y.saturating_add(line as u16 + 1),
                            modal.width.saturating_sub(2),
                            1,
                        ),
                        buffer,
                        &text,
                        theme.style(if index == *selected {
                            "file-entry:selected"
                        } else {
                            "file-entry"
                        }),
                    );
                }
                None
            }
            None => None,
        }
    }
}
