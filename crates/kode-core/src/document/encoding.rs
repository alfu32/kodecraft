use crate::{DomainError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
    Mixed,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFormat {
    pub utf8_bom: bool,
    pub line_ending: LineEnding,
}
#[derive(Debug, Clone)]
pub struct DecodedText {
    pub text: String,
    pub format: FileFormat,
}

pub fn decode(bytes: &[u8]) -> Result<DecodedText> {
    let utf8_bom = bytes.starts_with(b"\xef\xbb\xbf");
    let body = if utf8_bom { &bytes[3..] } else { bytes };
    let text = std::str::from_utf8(body).map_err(|error| DomainError::Parse {
        format: "UTF-8 text",
        message: error.to_string(),
    })?;
    let mut lf = false;
    let mut crlf = false;
    for (index, byte) in body.iter().enumerate() {
        if *byte == b'\n' {
            if index > 0 && body[index - 1] == b'\r' {
                crlf = true;
            } else {
                lf = true;
            }
        }
    }
    let line_ending = match (lf, crlf) {
        (false, true) => LineEnding::CrLf,
        (true, true) => LineEnding::Mixed,
        _ => LineEnding::Lf,
    };
    Ok(DecodedText {
        text: text.into(),
        format: FileFormat {
            utf8_bom,
            line_ending,
        },
    })
}
pub fn encode(text: &str, format: FileFormat) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len().saturating_add(3));
    if format.utf8_bom {
        bytes.extend_from_slice(b"\xef\xbb\xbf");
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes
}
