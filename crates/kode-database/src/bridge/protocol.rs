use futures_util::{SinkExt, StreamExt};
use kode_core::{DomainError, Result};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::{
    bytes::Bytes,
    codec::{FramedRead, FramedWrite, LengthDelimitedCodec},
};
pub const VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DEPTH: usize = 64;
pub fn failure(message: impl Into<String>) -> DomainError {
    DomainError::Parse {
        format: "JDBC frame/protocol",
        message: message.into(),
    }
}
fn codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .big_endian()
        .length_field_type::<u32>()
        .max_frame_length(MAX_FRAME_BYTES)
        .new_codec()
}
fn depth(value: &Value, remaining: usize) -> bool {
    if remaining == 0 {
        return false;
    }
    match value {
        Value::Array(values) => values.iter().all(|value| depth(value, remaining - 1)),
        Value::Object(values) => values.values().all(|value| depth(value, remaining - 1)),
        Value::Number(number) => number.to_string().len() <= 1000,
        _ => true,
    }
}
/// Validate only the envelope here; method-specific params/results belong to
/// their typed operation. Diagnostics never include untrusted payload values.
pub fn validate(value: &Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| failure("expected one JSON-RPC object, batches are unsupported"))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(failure("expected JSON-RPC version 2.0"));
    }
    if !depth(value, MAX_DEPTH) {
        return Err(failure("JSON nesting or number limit exceeded"));
    }
    let id = object.get("id");
    if id.is_some_and(|id| {
        !id.as_str().is_some_and(|id| {
            !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)
        })
    }) {
        return Err(failure(
            "request/response ID must be a bounded nonempty string",
        ));
    }
    if let Some(method) = object.get("method") {
        if !method.as_str().is_some_and(|method| {
            !method.is_empty() && method.len() <= 128 && !method.chars().any(char::is_control)
        }) {
            return Err(failure("method must be a bounded nonempty string"));
        }
        if object.contains_key("result") || object.contains_key("error") {
            return Err(failure("method envelope cannot contain a result/error"));
        }
        if object
            .get("params")
            .is_some_and(|params| !params.is_object())
        {
            return Err(failure("method params must be an object"));
        }
    } else {
        if id.is_none() || object.contains_key("result") == object.contains_key("error") {
            return Err(failure("response needs an ID and exactly one result/error"));
        }
        if let Some(error) = object.get("error") {
            if error.get("code").and_then(Value::as_i64).is_none()
                || error.get("message").and_then(Value::as_str).is_none()
            {
                return Err(failure("malformed JSON-RPC error"));
            }
        }
    }
    Ok(())
}
pub fn decode(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(failure("payload size outside 1..8 MiB"));
    }
    std::str::from_utf8(bytes).map_err(|_| failure("payload is not UTF-8"))?;
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| failure("malformed UTF-8 JSON payload"))?;
    validate(&value)?;
    Ok(value)
}
pub fn payload(value: &Value) -> Result<Vec<u8>> {
    validate(value)?;
    let bytes = serde_json::to_vec(value).map_err(|_| failure("cannot encode JSON envelope"))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(failure("payload exceeds 8 MiB"));
    }
    Ok(bytes)
}
pub fn encode(value: &Value) -> Result<Vec<u8>> {
    let bytes = payload(value)?;
    let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
    frame.extend(bytes);
    Ok(frame)
}
pub struct FrameReader<R> {
    framed: FramedRead<R, LengthDelimitedCodec>,
}
impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            framed: FramedRead::new(reader, codec()),
        }
    }
    pub async fn read(&mut self) -> Result<Option<Value>> {
        match self.framed.next().await {
            None => Ok(None),
            Some(Err(_)) => Err(failure(
                "invalid length, truncated frame or pipe read failure",
            )),
            Some(Ok(bytes)) => decode(&bytes).map(Some),
        }
    }
}
pub struct FrameWriter<W> {
    framed: FramedWrite<W, LengthDelimitedCodec>,
}
impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            framed: FramedWrite::new(writer, codec()),
        }
    }
    pub async fn write(&mut self, value: &Value) -> Result<()> {
        self.write_payload(payload(value)?).await
    }
    pub async fn write_payload(&mut self, bytes: Vec<u8>) -> Result<()> {
        if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
            return Err(failure("payload size outside 1..8 MiB"));
        }
        self.framed
            .send(Bytes::from(bytes))
            .await
            .map_err(|_| failure("pipe write/flush failed"))
    }
    pub async fn close(&mut self) -> Result<()> {
        SinkExt::<Bytes>::close(&mut self.framed)
            .await
            .map_err(|_| failure("pipe close failed"))
    }
}
