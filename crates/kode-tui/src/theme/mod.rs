use ratatui::style::{Color, Modifier, Style};
use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StyleProperties {
    pub style: Style,
    pub top: Option<i32>,
    pub left: Option<i32>,
    pub bottom: Option<i32>,
    pub right: Option<i32>,
    pub border_set: Option<String>,
    pub line_set: Option<String>,
    pub extended: BTreeMap<String, String>,
}
impl StyleProperties {
    pub fn merge(&mut self, other: &Self) {
        self.style = self.style.patch(other.style);
        if other.top.is_some() {
            self.top = other.top;
        }
        if other.left.is_some() {
            self.left = other.left;
        }
        if other.bottom.is_some() {
            self.bottom = other.bottom;
        }
        if other.right.is_some() {
            self.right = other.right;
        }
        if other.border_set.is_some() {
            self.border_set.clone_from(&other.border_set);
        }
        if other.line_set.is_some() {
            self.line_set.clone_from(&other.line_set);
        }
        self.extended.extend(other.extended.clone());
    }
    fn parse(body: &str) -> Self {
        let mut output = Self::default();
        for definition in body.split([';', '\n', '\r']) {
            let Some((key, value)) = definition.split_once(':') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if value.is_empty() || value == "none" {
                continue;
            }
            output.extended.insert(key.into(), value.into());
            match key {
                "fg" => {
                    if let Some(color) = color(value) {
                        output.style = output.style.fg(color);
                    }
                }
                "bg" => {
                    if let Some(color) = color(value) {
                        output.style = output.style.bg(color);
                    }
                }
                "top" => output.top = value.parse().ok(),
                "left" => output.left = value.parse().ok(),
                "bottom" => output.bottom = value.parse().ok(),
                "right" => output.right = value.parse().ok(),
                "border-set" => output.border_set = Some(value.into()),
                "line-set" => output.line_set = Some(value.into()),
                "text-decoration" => {
                    let mut modifier = Modifier::empty();
                    for word in value.split(|ch: char| ch.is_whitespace() || ch == ',') {
                        modifier |= match word {
                            "bold" => Modifier::BOLD,
                            "italic" => Modifier::ITALIC,
                            "underline" => Modifier::UNDERLINED,
                            "reverse" => Modifier::REVERSED,
                            "strikethrough" => Modifier::CROSSED_OUT,
                            _ => Modifier::empty(),
                        };
                    }
                    output.style = output
                        .style
                        .remove_modifier(Modifier::all())
                        .add_modifier(modifier);
                }
                _ => {}
            }
        }
        output
    }
}
fn color(value: &str) -> Option<Color> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 {
        return None;
    }
    let rgb = u32::from_str_radix(value, 16).ok()?;
    Some(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
}

#[derive(Debug, Default, Clone)]
pub struct Theme {
    pub defaults: StyleProperties,
    pub rules: BTreeMap<String, StyleProperties>,
}
impl Theme {
    pub fn compiled() -> Self {
        Self::parse(include_str!("../../../../resources/styles/app.css"))
    }
    pub fn parse(css: &str) -> Self {
        let mut clean = String::new();
        let mut rest = css;
        while let Some(start) = rest.find("/*") {
            clean.push_str(&rest[..start]);
            let Some(end) = rest[start + 2..].find("*/") else {
                rest = "";
                break;
            };
            rest = &rest[start + 2 + end + 2..];
        }
        clean.push_str(rest);
        let mut theme = Self::default();
        for block in clean.split('}') {
            let Some((selector, body)) = block.split_once('{') else {
                continue;
            };
            let selector = selector
                .replace('>', " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(".");
            if selector.is_empty() {
                continue;
            }
            let style = StyleProperties::parse(body);
            if selector == "default" {
                theme.defaults.merge(&style);
            } else {
                theme.rules.entry(selector).or_default().merge(&style);
            }
        }
        theme
    }
    pub fn merge(&mut self, other: &Self) {
        self.defaults.merge(&other.defaults);
        for (key, value) in &other.rules {
            self.rules.entry(key.clone()).or_default().merge(value);
        }
    }
    pub fn properties(&self, id: &str) -> StyleProperties {
        let mut result = self.defaults.clone();
        if let Some(rule) = self.rules.get(id) {
            result.merge(rule);
        }
        result
    }
    pub fn style(&self, id: &str) -> Style {
        self.properties(id).style
    }
}
