use super::*;
use crate::{editor::display::display_text, widgets::controls::draw_text};
use ratatui::widgets::{Clear, Widget};
impl Form {
    fn add_hit(&mut self, area: Rect, action: Action) {
        if !area.is_empty() {
            self.hits.push(Hit { area, action });
        }
    }
    fn text_field(
        &mut self,
        mut area: Rect,
        field: Field,
        buffer: &mut Buffer,
        theme: &Theme,
        focused: bool,
    ) -> Option<Position> {
        if field == Field::File {
            let selected = self
                .controls
                .get(self.selection)
                .is_some_and(|(_, action)| *action == Action::Browse);
            if area.width >= 7 {
                let (width, label) = if area.width >= 18 {
                    (14, "[Browse Alt+P]")
                } else {
                    (5, "[...]")
                };
                let browse = Rect::new(area.right() - width, area.y, width, 1);
                area.width -= width;
                self.button(browse, label, Action::Browse, buffer, theme);
            } else if selected {
                self.button(area, "Browse Alt+P", Action::Browse, buffer, theme);
                return None;
            }
        }
        self.add_hit(area, Action::Focus(field));
        self.fields.get_mut(&field).and_then(|value| {
            value.render(area, buffer, theme, focused && self.focused == Some(field))
        })
    }
    fn button(
        &mut self,
        area: Rect,
        label: &str,
        action: Action,
        buffer: &mut Buffer,
        theme: &Theme,
    ) {
        let selected = self
            .controls
            .get(self.selection)
            .is_some_and(|(_, candidate)| candidate == &action);
        draw_text(
            area,
            buffer,
            &display_text(label),
            theme.style(if selected {
                "file-entry:selected"
            } else {
                "lsp-button"
            }),
        );
        self.add_hit(area, action);
    }
    pub(super) fn render_form(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        model: &Workspace,
        focused: bool,
    ) -> Result<Option<Position>> {
        let area = area.intersection(buffer.area);
        self.geometry = Geometry {
            area,
            ..Default::default()
        };
        self.hits.clear();
        if area.is_empty() {
            return Ok(None);
        }
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("content"));
        if self.closed || model.node(&self.path).is_none() {
            draw_text(area, buffer, "REST item removed", theme.style("content"));
            return Ok(None);
        }
        let rows = self.cached_rows(model)?;
        if self.layout_width != area.width
            || !Weak::ptr_eq(&self.layout_rows, &Arc::downgrade(&rows))
        {
            self.controls.clear();
            if self.request {
                self.controls.extend(
                    [
                        Action::Focus(Field::Method),
                        Action::Focus(Field::Url),
                        Action::Send,
                    ]
                    .into_iter()
                    .map(|action| (usize::MAX, action)),
                );
            }
            let mut virtual_y = 0usize;
            self.row_offsets.clear();
            for row in rows.iter() {
                self.row_offsets.push(virtual_y);
                match row {
                    Row::Field(_, field) => {
                        self.controls.push((virtual_y, Action::Focus(*field)));
                        if *field == Field::File {
                            self.controls.push((virtual_y, Action::Browse));
                        }
                    }
                    Row::Buttons(buttons) => {
                        let mut x = 0usize;
                        for (label, action) in buttons {
                            let width = label.len().min(area.width as usize).max(1);
                            if x > 0 && x + width > area.width as usize {
                                virtual_y += 1;
                                x = 0;
                            }
                            self.controls.push((virtual_y, action.clone()));
                            x += width + 1;
                        }
                    }
                    Row::Entry { kind, index, .. } => {
                        if *kind != Rows::Auth {
                            self.controls
                                .push((virtual_y, Action::Toggle(*kind, *index)));
                        }
                        self.controls.push((virtual_y, Action::Edit(*kind, *index)));
                        if *kind != Rows::Auth {
                            self.controls
                                .push((virtual_y, Action::Remove(*kind, *index)));
                        }
                    }
                    Row::Embed(_, kind) => self.controls.push((virtual_y, Action::Buffer(*kind))),
                    Row::Wrapped(text) => {
                        virtual_y += wrapped(text, area.width).len().saturating_sub(1)
                    }
                    Row::Info(_) => {}
                }
                virtual_y += 1;
            }
            self.virtual_height = virtual_y;
            self.layout_width = area.width;
            self.layout_rows = Arc::downgrade(&rows);
        }
        let virtual_y = self.virtual_height;
        self.selection = self.selection.min(self.controls.len().saturating_sub(1));
        if area.height < 5 || area.width < 8 {
            if let Some((_, action)) = self.controls.get(self.selection).cloned() {
                return Ok(match action {
                    Action::Focus(field) => self.text_field(area, field, buffer, theme, focused),
                    Action::Buffer(_) => {
                        self.geometry.editor = area;
                        self.add_hit(area, action);
                        None
                    }
                    action => {
                        let label = match action {
                            Action::Browse => "Browse Alt+P",
                            Action::Send => {
                                if self.running {
                                    "Stop"
                                } else {
                                    "Send"
                                }
                            }
                            Action::Tab(_) => "Tab",
                            Action::Edit(_, _) => "Edit",
                            Action::Toggle(_, _) => "Toggle",
                            Action::Remove(_, _) => "Remove",
                            Action::Add(_) => "Add",
                            _ => "Action",
                        };
                        self.button(area, label, action, buffer, theme);
                        None
                    }
                });
            }
            return Ok(None);
        }
        let title = format!(
            "{} | {}",
            if self.request {
                "REQUEST"
            } else if self.path.is_root() {
                "COLLECTION"
            } else {
                "FOLDER"
            },
            self.fields
                .get(&Field::Name)
                .map(|field| field.value.as_str())
                .unwrap_or("REST")
        );
        draw_text(
            Rect::new(area.x, area.y, area.width, 1),
            buffer,
            &display_text(&title),
            theme.style("content"),
        );
        let mut y = area.y + 1;
        let mut cursor = None;
        if self.request {
            if area.width >= 24 {
                let method = 8;
                let send = 6;
                let url = area.width - method - send;
                cursor = self.text_field(
                    Rect::new(area.x, y, method, 1),
                    Field::Method,
                    buffer,
                    theme,
                    focused,
                );
                cursor = self
                    .text_field(
                        Rect::new(area.x + method, y, url, 1),
                        Field::Url,
                        buffer,
                        theme,
                        focused,
                    )
                    .or(cursor);
                self.button(
                    Rect::new(area.right() - send, y, send, 1),
                    if self.running { "Stop" } else { "Send" },
                    Action::Send,
                    buffer,
                    theme,
                );
                y += 1;
            } else {
                for field in [Field::Method, Field::Url] {
                    if y < area.bottom() {
                        cursor = self
                            .text_field(
                                Rect::new(area.x, y, area.width, 1),
                                field,
                                buffer,
                                theme,
                                focused,
                            )
                            .or(cursor);
                        y += 1;
                    }
                }
                if y < area.bottom() {
                    self.button(
                        Rect::new(area.x, y, area.width, 1),
                        if self.running { "Stop" } else { "Send" },
                        Action::Send,
                        buffer,
                        theme,
                    );
                    y += 1;
                }
            }
        }
        let mut x = area.x;
        for &tab in self.tabs() {
            let label = match tab {
                Tab::Overview => "Overview",
                Tab::Params => "Params",
                Tab::Auth => "Auth",
                Tab::Headers => "Headers",
                Tab::Body => "Body",
                Tab::Variables => "Vars",
                Tab::Settings => "Settings",
                Tab::Raw => "Raw",
            };
            let width = (label.len() as u16).min(area.width);
            if x > area.x && x + width > area.right() {
                x = area.x;
                y += 1;
            }
            if y >= area.bottom() {
                break;
            }
            let rect = Rect::new(x, y, width, 1);
            draw_text(
                rect,
                buffer,
                label,
                theme.style(if self.tab == tab {
                    "file-entry:selected"
                } else {
                    "lsp-button"
                }),
            );
            self.add_hit(rect, Action::Tab(tab));
            self.geometry.tabs.push((tab, rect));
            x = x.saturating_add(width + 1);
        }
        y = y.saturating_add(1).min(area.bottom());
        if !self.message.is_empty() && y < area.bottom() {
            draw_text(
                Rect::new(area.x, y, area.width, 1),
                buffer,
                &display_text(&self.message),
                theme.style("content"),
            );
            y += 1;
        }
        let content = Rect::new(area.x, y, area.width, area.bottom().saturating_sub(y));
        let embed = rows.iter().find_map(|row| {
            if let Row::Embed(label, kind) = row {
                Some((*label, *kind))
            } else {
                None
            }
        });
        let content = if let Some((label, kind)) = embed {
            let prelude = virtual_y
                .saturating_sub(1)
                .min((content.height.saturating_sub(2) / 2) as usize)
                as u16;
            let label_y = content.y + prelude;
            if label_y < content.bottom() {
                draw_text(
                    Rect::new(content.x, label_y, content.width, 1),
                    buffer,
                    label,
                    theme.style("content"),
                );
                let editor = Rect::new(
                    content.x,
                    label_y + 1,
                    content.width,
                    content.bottom().saturating_sub(label_y + 1),
                );
                self.geometry.editor = editor;
                self.add_hit(editor, Action::Buffer(kind));
            }
            Rect::new(content.x, content.y, content.width, prelude)
        } else {
            content
        };
        self.geometry.content = content;
        if self.follow {
            if let Some((row, _)) = self
                .controls
                .get(self.selection)
                .filter(|(row, action)| *row != usize::MAX && !matches!(action, Action::Buffer(_)))
            {
                if *row < self.scroll {
                    self.scroll = *row;
                } else if *row >= self.scroll + content.height as usize {
                    self.scroll = row
                        .saturating_add(1)
                        .saturating_sub(content.height as usize);
                }
            }
            self.follow = false;
        }
        self.scroll = self.scroll.min(
            virtual_y
                .saturating_sub(usize::from(embed.is_some()))
                .saturating_sub(content.height as usize),
        );
        let scroll = self.scroll;
        let first = self
            .row_offsets
            .partition_point(|offset| *offset <= scroll)
            .saturating_sub(1);
        for (index, row) in rows.iter().enumerate().skip(first) {
            let mut offset = self.row_offsets[index];
            if offset >= scroll + content.height as usize {
                break;
            }
            let visible = |row: usize| {
                row.checked_sub(scroll)
                    .filter(|row| *row < content.height as usize)
                    .map(|row| content.y + row as u16)
            };
            match row {
                Row::Wrapped(text) => {
                    for line in wrapped(text, content.width) {
                        if let Some(y) = visible(offset) {
                            draw_text(
                                Rect::new(content.x, y, content.width, 1),
                                buffer,
                                &line,
                                theme.style("content"),
                            );
                        }
                        offset += 1;
                    }
                }
                Row::Info(text) => {
                    if let Some(y) = visible(offset) {
                        draw_text(
                            Rect::new(content.x, y, content.width, 1),
                            buffer,
                            &display_text(text),
                            theme.style("content"),
                        );
                    }
                }
                Row::Field(label, field) => {
                    if let Some(y) = visible(offset) {
                        let label_width = (label.len() + 2).min(content.width as usize / 3) as u16;
                        draw_text(
                            Rect::new(content.x, y, label_width, 1),
                            buffer,
                            label,
                            theme.style("content"),
                        );
                        cursor = self
                            .text_field(
                                Rect::new(
                                    content.x + label_width,
                                    y,
                                    content.width - label_width,
                                    1,
                                ),
                                *field,
                                buffer,
                                theme,
                                focused,
                            )
                            .or(cursor);
                    }
                }
                Row::Buttons(buttons) => {
                    let mut x = content.x;
                    for (label, action) in buttons {
                        let width = (label.len() as u16).min(content.width).max(1);
                        if x > content.x && x + width > content.right() {
                            offset += 1;
                            x = content.x;
                        }
                        if let Some(y) = visible(offset) {
                            self.button(
                                Rect::new(x, y, width, 1),
                                label,
                                action.clone(),
                                buffer,
                                theme,
                            );
                        }
                        x = x.saturating_add(width + 1);
                    }
                }
                Row::Entry {
                    kind,
                    index,
                    label,
                    enabled,
                } => {
                    if let Some(y) = visible(offset) {
                        if *kind == Rows::Auth {
                            self.button(
                                Rect::new(content.x, y, content.width, 1),
                                label,
                                Action::Edit(*kind, *index),
                                buffer,
                                theme,
                            );
                        } else {
                            let toggle = 4.min(content.width);
                            let remove = 7.min(content.width.saturating_sub(toggle) / 2);
                            let edit = content.width - toggle - remove;
                            self.button(
                                Rect::new(content.x, y, toggle, 1),
                                if *enabled { "[x]" } else { "[ ]" },
                                Action::Toggle(*kind, *index),
                                buffer,
                                theme,
                            );
                            self.button(
                                Rect::new(content.x + toggle, y, edit, 1),
                                label,
                                Action::Edit(*kind, *index),
                                buffer,
                                theme,
                            );
                            self.button(
                                Rect::new(content.right() - remove, y, remove, 1),
                                "Remove",
                                Action::Remove(*kind, *index),
                                buffer,
                                theme,
                            );
                        }
                    }
                }
                Row::Embed(_, _) => {}
            }
        }
        Ok(cursor)
    }
}

fn wrapped(text: &str, width: u16) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let text = display_text(text);
    let mut lines = vec![];
    let mut line = String::new();
    let mut used = 0;
    for c in text.chars() {
        let size = c.width().unwrap_or(0);
        if c == '\n' || used + size > usize::from(width.max(1)) {
            lines.push(std::mem::take(&mut line));
            used = 0;
            if c == '\n' {
                continue;
            }
        }
        line.push(c);
        used += size;
    }
    lines.push(line);
    lines
}
