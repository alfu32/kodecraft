//! Scrollable credits and the actual loaded language catalog.
use crate::{
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::{controls::draw_text, text_panel::TextPanel},
};
use crossterm::event::MouseButton;
use kode_core::{DomainError, Result};
use kode_intelligence::facade::IntelligenceResources;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use std::collections::BTreeSet;

pub struct About {
    pub credits: TextPanel,
    pub languages: TextPanel,
    pub selected_language: Option<String>,
    language_names: Vec<String>,
    language_focus: bool,
    language_area: Rect,
}
impl About {
    pub fn new() -> Result<Self> {
        let metadata: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../resources/credits-rust.json"))
                .map_err(|error| DomainError::Parse {
                    format: "dependency credits",
                    message: error.to_string(),
                })?;
        let mut lines=vec![
            "Kode source: https://github.com/alfu32/kodecraft".into(),
            "License: MIT | https://opensource.org/license/mit".into(),
            "Rust terminal application; external Java 21+ for JDBC only.".into(),
            "Java helper libraries:".into(),
            "H2 2.2.224 | MPL-2.0 / EPL-1.0 | https://github.com/h2database/h2database".into(),
            "Jackson 2.21.4 | Apache-2.0 | https://github.com/FasterXML/jackson-databind".into(),
            "Maven Resolver 2.0.23 | Apache-2.0 | https://github.com/apache/maven-resolver".into(),
            "SLF4J 2.0.17 | MIT | https://github.com/qos-ch/slf4j".into(),
            "Native browser terminal: ttyd 1.8.3 | MIT | https://github.com/alfu32/ttyd | https://github.com/tsl0922/ttyd".into(),
            "libuv | MIT | https://github.com/libuv/libuv".into(),
            "libwebsockets | MIT and third-party notices | https://libwebsockets.org".into(),
            "json-c | MIT | https://github.com/json-c/json-c".into(),
            "zlib | Zlib | https://github.com/madler/zlib".into(),
            "OpenSSL | Apache-2.0 | https://github.com/openssl/openssl".into(),
            "Rust dependency catalog (includes build/test and target dependencies):".into(),
        ];
        for package in metadata["packages"].as_array().into_iter().flatten() {
            let text = |key: &str| package[key].as_str().unwrap_or("See source");
            lines.push(format!(
                "{} {} | {}",
                text("name"),
                text("version"),
                text("license")
            ));
            if let Some(url) = package["repository"]
                .as_str()
                .or_else(|| package["homepage"].as_str())
            {
                lines.push(format!("  {url}"));
            }
        }
        Ok(Self {
            credits: TextPanel::new(lines)?,
            languages: TextPanel::new(vec!["Language catalog has not been loaded".into()])?,
            selected_language: None,
            language_names: vec![],
            language_focus: false,
            language_area: Rect::default(),
        })
    }
    pub fn load_languages(&mut self, resources: &IntelligenceResources) {
        let mut languages: BTreeSet<String> = resources
            .definitions
            .languages()
            .map(str::to_owned)
            .collect();
        languages.extend(
            resources
                .grammars
                .entries()
                .map(|grammar| grammar.id.to_owned()),
        );
        self.language_names = languages.into_iter().collect();
        self.refresh_languages();
    }
    fn refresh_languages(&mut self) {
        self.languages.set_lines(
            self.language_names
                .iter()
                .map(|name| {
                    format!(
                        "{} {name}",
                        if self.selected_language.as_ref() == Some(name) {
                            ">"
                        } else {
                            "-"
                        }
                    )
                })
                .collect(),
        );
    }
    pub fn focus_id(&self) -> FocusId {
        if self.language_focus {
            self.languages.id
        } else {
            self.credits.id
        }
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let header = Rect::new(area.x, area.y, area.width, area.height.min(1));
        let revision = option_env!("KODE_BUILD_SHA")
            .map(|sha| format!(" ({})", &sha[..sha.len().min(8)]))
            .unwrap_or_default();
        draw_text(
            header,
            buffer,
            &format!("Kode v{}{} | MIT", env!("CARGO_PKG_VERSION"), revision),
            theme.style("content"),
        );
        let body = Rect::new(
            area.x,
            header.bottom(),
            area.width,
            area.height.saturating_sub(1),
        );
        if body.height < 10 {
            draw_text(
                Rect::new(body.x, body.y, body.width, body.height.min(1)),
                buffer,
                if self.language_focus {
                    "Languages (Tab: credits)"
                } else {
                    "Credits (Tab: languages)"
                },
                theme.style("content"),
            );
            let content = Rect::new(
                body.x,
                body.y + body.height.min(1),
                body.width,
                body.height.saturating_sub(1),
            );
            self.language_area = if self.language_focus {
                content
            } else {
                Rect::default()
            };
            if self.language_focus {
                self.credits.hide();
                self.languages.render(content, buffer, theme);
            } else {
                self.languages.hide();
                self.credits.render(content, buffer, theme);
            }
        } else {
            let credit_height = body.height / 2;
            draw_text(
                Rect::new(body.x, body.y, body.width, 1),
                buffer,
                "Credits (Tab switches focus)",
                theme.style("content"),
            );
            self.credits.render(
                Rect::new(
                    body.x,
                    body.y + 1,
                    body.width,
                    credit_height.saturating_sub(1),
                ),
                buffer,
                theme,
            );
            let language_y = body.y + credit_height;
            draw_text(
                Rect::new(body.x, language_y, body.width, 1),
                buffer,
                "Supported languages",
                theme.style("content"),
            );
            self.language_area = Rect::new(
                body.x,
                language_y + 1,
                body.width,
                body.height - credit_height - 1,
            );
            self.languages.render(self.language_area, buffer, theme);
        }
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> bool {
        if matches!(event,InputEvent::Key(input)if input.key==Key::Tab&&!input.modifiers.ctrl&&!input.modifiers.alt&&!input.modifiers.meta&&focus.focused==Some(self.focus_id()))
        {
            self.language_focus = !self.language_focus;
            focus.focused = Some(self.focus_id());
            return true;
        }
        if let InputEvent::Pointer(pointer) = event
            && pointer.action == PointerAction::Down(MouseButton::Left)
            && self
                .language_area
                .contains(Position::new(pointer.x, pointer.y))
        {
            self.language_focus = true;
            self.selected_language = self
                .languages
                .line_at(Position::new(pointer.x, pointer.y))
                .and_then(|index| self.language_names.get(index))
                .cloned();
            self.refresh_languages();
            focus.focused = Some(self.languages.id);
            return true;
        }
        if self.languages.dispatch(event, focus) || self.credits.dispatch(event, focus) {
            self.language_focus = focus.focused == Some(self.languages.id);
            true
        } else {
            false
        }
    }
}
