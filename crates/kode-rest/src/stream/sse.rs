use super::*;
use eventsource_stream::{EventStreamError, Eventsource};
use futures_util::{StreamExt, stream};
use std::time::Duration;

#[derive(Default)]
struct Guard {
    line: usize,
    event: usize,
    cr: bool,
}
impl Guard {
    fn feed(&mut self, bytes: &[u8]) -> std::result::Result<(), Failure> {
        for &byte in bytes {
            self.event += 1;
            if self.event > MAX_EVENT_BYTES {
                return Err(Failure::StreamLimit);
            }
            if byte == b'\n' && self.cr {
                self.cr = false;
                continue;
            }
            self.cr = byte == b'\r';
            if byte == b'\r' || byte == b'\n' {
                if self.line == 0 {
                    self.event = 0;
                }
                self.line = 0;
            } else {
                self.line += 1;
                if self.line > MAX_FRAME_BYTES {
                    return Err(Failure::StreamLimit);
                }
            }
        }
        Ok(())
    }
}
/// The selected parser otherwise buffers permanently after an invalid byte and
/// slices an initial UTF-8 BOM at byte one. Supply valid complete UTF-8 prefixes,
/// strip the initial BOM for parsing only, and retain original raw bytes outside.
#[derive(Default)]
struct Decoder {
    tail: Vec<u8>,
    started: bool,
}
impl Decoder {
    fn feed(&mut self, chunk: &[u8]) -> std::result::Result<Vec<u8>, Failure> {
        let mut bytes = std::mem::take(&mut self.tail);
        bytes.extend_from_slice(chunk);
        let end = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Err(Failure::StreamProtocol),
        };
        self.tail = bytes.split_off(end);
        if !self.started && !bytes.is_empty() {
            self.started = true;
            if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                bytes.drain(..3);
            }
        }
        Ok(bytes)
    }
}
pub(super) async fn run(
    client: reqwest::Client,
    request: reqwest::Request,
    output: &mut Response,
    capture: &mut Capture,
    sink: &Sink,
) -> std::result::Result<(), Failure> {
    let timeout = Duration::from_millis(output.request.request.timeout_ms.max(1));
    let response = tokio::time::timeout(timeout, client.execute(request))
        .await
        .map_err(|_| Failure::Timeout)?
        .map_err(|error| http::classify(&error))?;
    http::apply_metadata(&response, output)?;
    sink.opened(output).await?;
    let content_type = output
        .content_type
        .as_deref()
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    if !content_type.eq_ignore_ascii_case("text/event-stream") || output.status_code != Some(200) {
        // Preserve ordinary failure/diagnostic bodies instead of interpreting
        // HTML/JSON as SSE or discarding a non-2xx HTTP response.
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| http::classify(&error))?
        {
            capture.record(&chunk, DataKind::Http, false, sink).await?;
        }
        return Ok(());
    }
    let mut typed_budget = capture.limit;
    let mut typed_events = 0;
    let mut display_limit_reported = false;
    let chunks = stream::try_unfold(
        (
            response,
            Guard::default(),
            Decoder::default(),
            capture,
            sink,
        ),
        |(mut response, mut guard, mut decoder, capture, sink)| async move {
            match response
                .chunk()
                .await
                .map_err(|error| http::classify(&error))?
            {
                Some(chunk) => {
                    capture
                        .record(&chunk, DataKind::SseRaw, false, sink)
                        .await?;
                    if chunk.len() > 1024 * 1024 {
                        return Err(Failure::StreamLimit);
                    }
                    guard.feed(&chunk)?;
                    let bytes = decoder.feed(&chunk)?;
                    Ok(Some((bytes, (response, guard, decoder, capture, sink))))
                }
                None if !decoder.tail.is_empty() => Err(Failure::StreamProtocol),
                None => Ok(None),
            }
        },
    );
    let events = chunks.eventsource();
    tokio::pin!(events);
    while let Some(event) = events.next().await {
        let event = event.map_err(|error| match error {
            EventStreamError::Transport(failure) => failure,
            _ => Failure::StreamProtocol,
        })?;
        let size = event.event.len() + event.data.len() + event.id.len();
        if size > MAX_EVENT_BYTES {
            return Err(Failure::StreamLimit);
        }
        if size <= typed_budget && typed_events < MAX_RETAINED_EVENTS {
            typed_events += 1;
            typed_budget -= size;
            sink.send(EventKind::Sse {
                name: event.event,
                data: event.data,
                id: event.id,
                retry: event.retry,
            })
            .await?;
        } else if !display_limit_reported {
            display_limit_reported = true;
            sink.send(EventKind::Notice("SSE event display limit reached; raw response prefix and received counts remain available")).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_bom_split_utf8_invalid_utf8_and_unterminated_line_limits() {
        let mut decoder = Decoder::default();
        assert!(decoder.feed(&[0xef]).unwrap().is_empty());
        assert!(decoder.feed(&[0xbb]).unwrap().is_empty());
        assert_eq!(decoder.feed(&[0xbf, b'd', b'a']).unwrap(), b"da");
        assert!(decoder.feed(&[0xc3]).unwrap().is_empty());
        assert_eq!(decoder.feed(&[0xa9]).unwrap(), "é".as_bytes());
        assert_eq!(decoder.feed(&[0xff]), Err(Failure::StreamProtocol));
        let mut guard = Guard::default();
        assert_eq!(
            guard.feed(&vec![b'a'; MAX_FRAME_BYTES + 1]),
            Err(Failure::StreamLimit)
        );
        let mut guard = Guard::default();
        for _ in 0..10 {
            guard.feed(b"data: a\r").unwrap();
            guard.feed(b"\n\r\n").unwrap();
        }
        assert_eq!(guard.event, 1); // final LF completes the preceding CRLF
    }
}
