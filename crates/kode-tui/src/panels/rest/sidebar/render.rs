use super::*;
use crate::{editor::display::display_text, widgets::controls::draw_text};
use ratatui::widgets::{Clear, Widget};
impl Sidebar {
    fn buttons(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        buttons: Vec<(&str, Action)>,
    ) -> u16 {
        if area.is_empty() {
            return 0;
        }
        let mut x = area.x;
        let mut y = area.y;
        for (label, action) in buttons {
            let width = (label.len() as u16).min(area.width);
            if x > area.x && x.saturating_add(width) > area.right() {
                x = area.x;
                y = y.saturating_add(1);
            }
            if y >= area.bottom() {
                break;
            }
            let rect = Rect::new(x, y, width, 1);
            draw_text(rect, buffer, label, theme.style("lsp-button"));
            self.hits.push(Hit { area: rect, action });
            x = x.saturating_add(width).saturating_add(1);
        }
        y.saturating_sub(area.y).saturating_add(1)
    }
    pub(super) fn render_panel(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        model: &Workspace,
        focused: bool,
    ) -> Option<Position> {
        let area = area.intersection(buffer.area);
        self.geometry = Geometry {
            area,
            ..Default::default()
        };
        self.hits.clear();
        if area.is_empty() {
            return None;
        }
        self.refresh(model);
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("content"));
        let mut cursor = None;
        let title = Rect::new(area.x, area.y, area.width, 1);
        if self.filtering {
            draw_text(title, buffer, "Filter", theme.style("content"));
            self.geometry.filter = Rect::new(
                area.x + 6.min(area.width),
                area.y,
                area.width.saturating_sub(6),
                1,
            );
            cursor = self
                .filter
                .render(self.geometry.filter, buffer, theme, focused);
            if area.width <= 6 {
                self.geometry.filter = title;
                cursor = self.filter.render(title, buffer, theme, focused);
            }
        } else {
            draw_text(
                title,
                buffer,
                if self.environment_focus {
                    "REST environments"
                } else {
                    "REST API"
                },
                theme.style("content"),
            );
        }
        let tool_area = Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(1).min(3),
        );
        let buttons = vec![
            ("+Req", Action::AddRequest(NodePath::default())),
            ("+Dir", Action::AddFolder(NodePath::default())),
            ("Run", Action::Run(NodePath::default())),
            ("Import", Action::ImportPostman),
            ("OAS", Action::ImportOpenApi(NodePath::default())),
            ("Export", Action::Export),
            ("!", Action::Notices),
        ];
        let tool_rows = self.buttons(tool_area, buffer, theme, buttons);
        let message_y = tool_area.y.saturating_add(tool_rows);
        if message_y < area.bottom() {
            draw_text(
                Rect::new(area.x, message_y, area.width, 1),
                buffer,
                &display_text(&self.message),
                theme.style("content"),
            );
        }
        let top = message_y.saturating_add(1).min(area.bottom());
        let remaining = area.bottom().saturating_sub(top);
        let can_split = remaining >= 10;
        let environment_height = if can_split {
            (model.ui().environment_panel_height.clamp(5, 60) as u16)
                .clamp(5, remaining.saturating_sub(5))
        } else {
            0
        };
        let tree_bottom = if can_split {
            area.bottom() - environment_height - 1
        } else if self.environment_focus {
            top
        } else {
            area.bottom()
        };
        self.geometry.tree = Rect::new(area.x, top, area.width, tree_bottom.saturating_sub(top));
        let height = self.geometry.tree.height as usize;
        self.scroll = self.scroll.min(self.visible.len().saturating_sub(height));
        let selected = self
            .visible
            .iter()
            .position(|node| node.path == self.cursor)
            .unwrap_or(0);
        if self.follow_cursor && selected < self.scroll {
            self.scroll = selected;
        }
        if self.follow_cursor && height > 0 && selected >= self.scroll + height {
            self.scroll = selected + 1 - height;
        }
        self.follow_cursor = false;
        for (index, node) in self
            .visible
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(height)
        {
            let row = Rect::new(area.x, top + (index - self.scroll) as u16, area.width, 1);
            let marker = if node.has_children {
                if self.expanded.contains(&node.path) {
                    "[-]"
                } else {
                    "[+]"
                }
            } else {
                "   "
            };
            let indent = (node.depth * 2).min(128);
            let method = node.method.as_deref().unwrap_or("");
            let label = format!(
                "{}{marker} {}{}",
                " ".repeat(indent),
                if method.is_empty() {
                    String::new()
                } else {
                    format!("{method:<7}")
                },
                node.name
            );
            draw_text(
                row,
                buffer,
                &display_text(&label),
                theme.style(if node.path == self.cursor && !self.environment_focus {
                    "file-entry:selected"
                } else {
                    "content"
                }),
            );
            if row.width >= 16 {
                let parent = node.kind != NodeKind::Request;
                let menu = Rect::new(row.right() - if parent { 9 } else { 5 }, row.y, 5, 1);
                draw_text(menu, buffer, "[...]", theme.style("lsp-button"));
                self.hits.push(Hit {
                    area: menu,
                    action: Action::Menu(node.path.clone()),
                });
                if parent {
                    let add = Rect::new(row.right() - 3, row.y, 3, 1);
                    draw_text(add, buffer, "[+]", theme.style("lsp-button"));
                    self.hits.push(Hit {
                        area: add,
                        action: Action::AddRequest(node.path.clone()),
                    });
                }
            }
            if node.has_children {
                let x = area.x.saturating_add(indent as u16);
                let width = area.right().saturating_sub(x).min(3);
                if width > 0 {
                    self.hits.push(Hit {
                        area: Rect::new(x, row.y, width, 1),
                        action: Action::Toggle(node.path.clone()),
                    });
                }
            }
        }
        if can_split {
            self.geometry.divider = Rect::new(area.x, tree_bottom, area.width, 1);
            draw_text(
                self.geometry.divider,
                buffer,
                &"─".repeat(area.width as usize),
                theme.style("splitter"),
            );
            self.geometry.environments =
                Rect::new(area.x, tree_bottom + 1, area.width, environment_height);
        } else if self.environment_focus {
            self.geometry.environments = Rect::new(area.x, top, area.width, remaining);
        }
        if can_split || self.environment_focus {
            let env = self.geometry.environments;
            draw_text(
                Rect::new(env.x, env.y, env.width, 1),
                buffer,
                "Environments (Tab)",
                theme.style("content"),
            );
            if self
                .environment
                .as_ref()
                .is_none_or(|id| model.environment(id).is_none())
            {
                self.environment = model
                    .active_environment()
                    .map(|environment| environment.id.clone())
                    .or_else(|| {
                        model
                            .environments()
                            .first()
                            .map(|environment| environment.id.clone())
                    });
            }
            let selected = self.environment.clone();
            let mut buttons = vec![("+", Action::NewEnvironment)];
            if let Some(id) = selected.clone() {
                buttons.extend([
                    ("Copy", Action::CopyEnvironment(id.clone())),
                    ("Rename", Action::RenameEnvironment(id.clone())),
                    ("Activate", Action::ActivateEnvironment(id.clone())),
                    ("-", Action::RemoveEnvironment(id)),
                ]);
            }
            let rows = self.buttons(
                Rect::new(
                    env.x,
                    env.y + 1,
                    env.width,
                    env.height.saturating_sub(1).min(3),
                ),
                buffer,
                theme,
                buttons,
            );
            let start = env.y.saturating_add(1 + rows).min(env.bottom());
            let visible = env.bottom().saturating_sub(start) as usize;
            let selected_index = model
                .environments()
                .iter()
                .position(|environment| Some(&environment.id) == self.environment.as_ref())
                .unwrap_or(0);
            self.environment_scroll = self
                .environment_scroll
                .min(model.environments().len().saturating_sub(visible));
            if self.follow_environment {
                if selected_index < self.environment_scroll {
                    self.environment_scroll = selected_index;
                }
                if visible > 0 && selected_index >= self.environment_scroll + visible {
                    self.environment_scroll = selected_index + 1 - visible;
                }
            }
            self.follow_environment = false;
            for (index, environment) in model
                .environments()
                .iter()
                .enumerate()
                .skip(self.environment_scroll)
                .take(visible)
            {
                let row = Rect::new(
                    env.x,
                    start + (index - self.environment_scroll) as u16,
                    env.width,
                    1,
                );
                let active = model
                    .active_environment()
                    .is_some_and(|active| active.id == environment.id);
                let label = format!("[{}] {}", if active { "*" } else { " " }, environment.name);
                draw_text(
                    row,
                    buffer,
                    &display_text(&label),
                    theme.style(if Some(&environment.id) == self.environment.as_ref() {
                        "file-entry:selected"
                    } else {
                        "content"
                    }),
                );
                self.hits.push(Hit {
                    area: row,
                    action: Action::OpenEnvironment(environment.id.clone()),
                });
            }
        }
        cursor
    }
}
