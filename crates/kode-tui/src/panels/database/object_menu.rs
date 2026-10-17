//! SQL text actions from the frozen database explorer. No execution or I/O.
use crate::{
    editor::display::display_text,
    focus::EventTarget,
    input::terminal::{InputEvent, Key},
    layout::centered,
    theme::Theme,
    widgets::controls::{button, draw_text},
};
use kode_database::model::{DatabaseObject, DatabaseObjectType};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Block, Borders, Clear, Widget},
};

pub struct ObjectMenu {
    pub source: String,
    pub object: DatabaseObject,
    pub actions: Vec<(String, String)>,
    pub selected: usize,
    pub finished: bool,
    pub sql: Option<String>,
    pub source_requested: bool,
    source_action: Option<usize>,
    hits: Vec<Rect>,
    cancel: Rect,
}
impl ObjectMenu {
    pub fn new(source: String, object: DatabaseObject) -> Self {
        use DatabaseObjectType::*;
        let name = &object.qualified_name;
        let ddl = match object.object_type {
            Table | View => match object.attributes.get("driverId").map(String::as_str) {
                Some("mysql" | "mariadb") => format!("SHOW CREATE TABLE {name};"),
                Some("h2") => format!("SCRIPT NODATA TABLE {name};"),
                _ => format!("-- DDL extraction is driver-specific for {name}"),
            },
            _ => format!("-- DDL extraction is driver-specific for {name}"),
        };
        let mut actions = if object.object_type == Package {
            vec![]
        } else {
            vec![("Copy DDL".into(), ddl)]
        };
        let source_action = if object.object_type == Package
            && object
                .attributes
                .get("sourceAvailable")
                .is_some_and(|value| value == "true")
        {
            actions.push((
                if object
                    .attributes
                    .get("ddlAvailable")
                    .is_some_and(|value| value == "true")
                {
                    "View package DDL"
                } else {
                    "View package source"
                }
                .into(),
                String::new(),
            ));
            Some(actions.len() - 1)
        } else {
            None
        };
        match object.object_type {
            Table | View => actions.push(("Copy SELECT".into(), format!("SELECT * FROM {name};"))),
            Function => actions.push(("Copy CALL / SELECT".into(), format!("SELECT {name}();"))),
            Procedure => actions.push(("Copy CALL".into(), format!("CALL {name}();"))),
            Type | Package => actions.push(("Copy name".into(), name.clone())),
            _ => {}
        }
        Self {
            source,
            object,
            actions,
            selected: 0,
            finished: false,
            sql: None,
            source_requested: false,
            source_action,
            hits: vec![],
            cancel: Rect::default(),
        }
    }
    fn choose(&mut self) {
        if self.source_action == Some(self.selected) {
            self.source_requested = true;
        } else {
            self.sql = self.actions.get(self.selected).map(|a| a.1.clone());
        }
        self.finished = true;
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let modal = centered(area.intersection(buffer.area), 60, 9);
        self.hits.clear();
        self.cancel = Rect::default();
        if modal.is_empty() {
            return;
        }
        Clear.render(modal, buffer);
        Block::default()
            .borders(Borders::ALL)
            .title(display_text(&self.object.name))
            .style(theme.style("project-search-dialog"))
            .render(modal, buffer);
        let content = modal.inner(ratatui::layout::Margin::new(
            u16::from(modal.width > 2),
            u16::from(modal.height > 2),
        ));
        if content.is_empty() {
            return;
        }
        let available = content.height.saturating_sub(1) as usize;
        let start = self.selected.saturating_sub(available.saturating_sub(1));
        self.hits.resize(self.actions.len(), Rect::default());
        for (index, (label, _)) in self.actions.iter().enumerate().skip(start).take(available) {
            let hit = Rect::new(
                content.x,
                content.y + (index - start) as u16,
                content.width,
                1,
            );
            self.hits[index] = hit;
            draw_text(
                hit,
                buffer,
                label,
                theme.style(if index == self.selected {
                    "file-entry:selected"
                } else {
                    "lsp-button"
                }),
            );
        }
        self.cancel = Rect::new(content.x, content.bottom() - 1, content.width, 1);
        draw_text(self.cancel, buffer, " Cancel ", theme.style("lsp-button"));
    }
}
impl EventTarget for ObjectMenu {
    fn dispatch(&mut self, event: &InputEvent) -> bool {
        if self.finished {
            return true;
        }
        if let InputEvent::Key(input) = event {
            match input.key {
                Key::Escape => self.finished = true,
                Key::Up => self.selected = self.selected.saturating_sub(1),
                Key::Down => {
                    self.selected = (self.selected + 1).min(self.actions.len().saturating_sub(1))
                }
                Key::Enter => self.choose(),
                _ => {}
            }
        }
        if button(event, self.cancel, false) {
            self.finished = true;
        } else if let Some(index) = self.hits.iter().position(|hit| button(event, *hit, false)) {
            self.selected = index;
            self.choose();
        }
        true
    }
}
