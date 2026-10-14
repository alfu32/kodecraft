use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Character(char),
    Tab,
    Escape(char),
}
impl Glyph {
    pub fn for_scalar(ch: char) -> Self {
        if ch == '\t' {
            return Self::Tab;
        }
        if ch.is_ascii() {
            return if ch.is_ascii_control() {
                Self::Escape(ch)
            } else {
                Self::Character(ch)
            };
        }
        let category = get_general_category(ch);
        let cp = ch as u32;
        let unrenderable = matches!(
            category,
            GeneralCategory::Control
                | GeneralCategory::Unassigned
                | GeneralCategory::PrivateUse
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
        );
        let combining = matches!(
            category,
            GeneralCategory::NonspacingMark
                | GeneralCategory::SpacingMark
                | GeneralCategory::EnclosingMark
        );
        let wide = matches!(cp, 0x1100..=0x115f | 0x2329..=0x232a | 0x2e80..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe10..=0xfe19 | 0xfe30..=0xfe6f | 0xff00..=0xff60 | 0xffe0..=0xffe6);
        if unrenderable || combining || wide || cp > 0xffff || ch.width() != Some(1) {
            Self::Escape(ch)
        } else {
            Self::Character(ch)
        }
    }
    pub fn width(self) -> usize {
        match self {
            Self::Character(_) => 1,
            Self::Tab => 4,
            Self::Escape(ch) => {
                let digits = if ch as u32 <= 0xff {
                    2
                } else {
                    4.max(((32 - (ch as u32).leading_zeros()) as usize).div_ceil(4))
                };
                2 + digits
            }
        }
    }
    pub fn text(self) -> String {
        match self {
            Self::Character(ch) => ch.to_string(),
            Self::Tab => "|-->".into(),
            Self::Escape(ch) if ch as u32 <= 0xff => format!("\\x{:02X}", ch as u32),
            Self::Escape(ch) => format!("\\x{:04X}", ch as u32),
        }
    }
}
pub fn display_text(text: &str) -> String {
    text.chars()
        .map(|ch| Glyph::for_scalar(ch).text())
        .collect()
}
