//! LSP byte-counted stdio frames, independent of JDBC's framing.
use kode_core::{DomainError, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
pub const MAX_HEADER_BYTES: usize = 8192;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub fn protocol(message: impl Into<String>) -> DomainError {
    DomainError::Parse {
        format: "LSP frame/protocol",
        message: message.into(),
    }
}
pub fn encode(value: &Value) -> Result<Vec<u8>> {
    if !value.is_object() {
        return Err(protocol("JSON-RPC message must be an object"));
    }
    let body = serde_json::to_vec(value).map_err(|e| protocol(e.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(protocol("message exceeds 16 MiB frame limit"));
    }
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(&body);
    Ok(frame)
}
pub struct FrameReader<R> {
    input: BufReader<R>,
}
impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(input: R) -> Self {
        Self {
            input: BufReader::new(input),
        }
    }
    async fn line(&mut self, remaining: usize) -> Result<Option<Vec<u8>>> {
        let mut line = vec![];
        loop {
            let available = self
                .input
                .fill_buf()
                .await
                .map_err(|e| protocol(format!("read header: {e}")))?;
            if available.is_empty() {
                if line.is_empty() {
                    return Ok(None);
                }
                return Err(protocol("truncated header"));
            }
            let count = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if count > remaining.saturating_sub(line.len()) {
                return Err(protocol("header exceeds 8 KiB"));
            }
            line.extend_from_slice(&available[..count]);
            self.input.consume(count);
            if line.last() == Some(&b'\n') {
                if !line.ends_with(b"\r\n") {
                    return Err(protocol("expected CRLF header line"));
                }
                return Ok(Some(line));
            }
        }
    }
    pub async fn read(&mut self) -> Result<Option<Value>> {
        let mut length = None;
        let mut headers = 0;
        loop {
            let Some(line) = self.line(MAX_HEADER_BYTES - headers).await? else {
                if headers == 0 {
                    return Ok(None);
                }
                return Err(protocol("EOF inside header block"));
            };
            headers += line.len();
            if line == b"\r\n" {
                break;
            }
            let line = std::str::from_utf8(&line[..line.len() - 2])
                .map_err(|_| protocol("header is not UTF-8/ASCII"))?;
            if !line.is_ascii() {
                return Err(protocol("header is not ASCII"));
            }
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| protocol("header field has no colon"))?;
            if name.eq_ignore_ascii_case("Content-Length") {
                if length.is_some() {
                    return Err(protocol("duplicate Content-Length"));
                }
                let value = value.trim();
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(protocol("invalid Content-Length"));
                }
                let value = value
                    .parse::<usize>()
                    .map_err(|_| protocol("Content-Length overflow"))?;
                if value == 0 || value > MAX_FRAME_BYTES {
                    return Err(protocol("Content-Length is outside 1..16 MiB"));
                }
                length = Some(value);
            } else if name.eq_ignore_ascii_case("Content-Type") {
                for parameter in value.split(';').skip(1) {
                    if let Some((name, value)) = parameter.trim().split_once('=')
                        && name.eq_ignore_ascii_case("charset")
                        && !matches!(value.trim().to_ascii_lowercase().as_str(), "utf-8" | "utf8")
                    {
                        return Err(protocol("only UTF-8 LSP messages are supported"));
                    }
                }
            }
        }
        let length = length.ok_or_else(|| protocol("missing Content-Length"))?;
        let mut body = vec![0; length];
        self.input
            .read_exact(&mut body)
            .await
            .map_err(|e| protocol(format!("truncated body: {e}")))?;
        let value: Value =
            serde_json::from_slice(&body).map_err(|e| protocol(format!("invalid JSON: {e}")))?;
        if !value.is_object() || value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(protocol("expected JSON-RPC 2.0 object"));
        }
        Ok(Some(value))
    }
}
