use super::*;
use crate::transport::{FrameReader, protocol};
use futures_util::FutureExt;
use std::{future::Future, panic::AssertUnwindSafe};
use tokio::{
    io::AsyncWriteExt,
    process::{ChildStdin, ChildStdout},
};
pub(super) fn spawn(
    tracker: &TaskTracker,
    shared: Arc<Shared>,
    future: impl Future<Output = Result<()>> + Send + 'static,
) {
    tracker.spawn(async move {
        match AssertUnwindSafe(future).catch_unwind().await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => shared.fail(error.to_string()),
            Err(_) => shared.fail("LSP task panicked"),
        }
    });
}
pub(super) async fn write(
    shared: Arc<Shared>,
    mut output: ChildStdin,
    mut messages: mpsc::Receiver<rpc::Frame>,
) -> Result<()> {
    loop {
        let frame = tokio::select! { biased;_=shared.stop.cancelled()=>break,frame=messages.recv()=>match frame { Some(frame)=>frame,None=>break } };
        let result = tokio::select! {
            biased;_=shared.stop.cancelled()=>break,
            result=tokio::time::timeout(shared.options.write_timeout,async {output.write_all(&frame.bytes).await?;output.flush().await})=>match result { Ok(result)=>result.map_err(|e|shared.error(format!("write server pipe: {e}"))),Err(_)=>Err(shared.error("server pipe write timed out")) },
        };
        match result {
            Ok(()) => {
                if let Some(ack) = frame.ack {
                    let _ = ack.send(Ok(()));
                }
            }
            Err(error) => {
                let message = error.to_string();
                if let Some(ack) = frame.ack {
                    let _ = ack.send(Err(error));
                }
                return Err(shared.error(message));
            }
        }
    }
    drop(output);
    Ok(())
}
pub(super) async fn read(shared: Arc<Shared>, input: ChildStdout) -> Result<()> {
    let mut reader = FrameReader::new(input);
    loop {
        let value = tokio::select! { biased;_=shared.stop.cancelled()=>return Ok(()),value=reader.read()=>value? };
        let Some(value) = value else {
            return Err(shared.error("server closed stdout"));
        };
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            if let Some(id) = value.get("id") {
                let valid = id.as_i64().is_some() || id.as_str().is_some_and(|id| id.len() <= 1024);
                let response = if valid {
                    server_request(&shared, method, params)
                } else {
                    Err((-32600, "Invalid request ID".into()))
                };
                let id = if valid { id.clone() } else { Value::Null };
                shared
                    .send(match response {
                        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                        Err((code, message)) => {
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
                        }
                    })
                    .await?;
            } else {
                notification(&shared, method, params);
            }
        } else if let Some(id) = value.get("id").and_then(Value::as_u64) {
            let reply = shared.pending.lock().expect("LSP pending lock").remove(&id);
            if let Some(reply) = reply {
                let result = if let Some(error) = value.get("error") {
                    let code = error
                        .get("code")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| protocol("response error has no integer code"))?;
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .ok_or_else(|| protocol("response error has no message"))?;
                    if code == -32800 {
                        Err(DomainError::Cancelled)
                    } else {
                        Err(shared.error(format!(
                            "LSP response error {code}: {}",
                            limited(message, 2048)
                        )))
                    }
                } else if let Some(result) = value.get("result") {
                    Ok(result.clone())
                } else {
                    Err(protocol("response has neither result nor error"))
                };
                let _ = reply.send(result);
            }
        } else if !value.get("id").is_some_and(Value::is_string) {
            return Err(protocol("message has no method or response ID"));
        }
    }
}
fn server_request(
    shared: &Shared,
    method: &str,
    params: Value,
) -> std::result::Result<Value, (i32, String)> {
    Ok(match method {
        "workspace/configuration" => {
            let items = params
                .get("items")
                .and_then(Value::as_array)
                .filter(|items| items.len() <= 1024)
                .ok_or_else(|| (-32602, "Expected at most 1024 configuration items".into()))?;
            Value::Array(
                items
                    .iter()
                    .map(|item| {
                        let Some(section) = item.get("section").and_then(Value::as_str) else {
                            return shared.options.configuration.clone();
                        };
                        section
                            .split('.')
                            .try_fold(&shared.options.configuration, |value, key| value.get(key))
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect(),
            )
        }
        "workspace/workspaceFolders" => json!([{"uri":shared.root,"name":"workspace"}]),
        "window/workDoneProgress/create" => Value::Null,
        "window/showMessageRequest" => Value::Null,
        "window/showDocument" => json!({"success":false}),
        "workspace/applyEdit" => {
            json!({"applied":false,"failureReason":"Server-initiated edits are not enabled"})
        }
        _ => {
            return Err((
                -32601,
                format!("Unsupported client method: {}", limited(method, 256)),
            ));
        }
    })
}
fn limited(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
fn notification(shared: &Shared, method: &str, params: Value) {
    let params = match method {
        "window/logMessage" | "window/showMessage" => {
            json!({"type":params.get("type"),"message":limited(params.get("message").and_then(Value::as_str).unwrap_or(""),2048)})
        }
        "$/progress" => {
            let value = &params["value"];
            json!({"token":params.get("token").filter(|token|token.as_i64().is_some() || token.as_str().is_some_and(|token|token.len()<=1024)),"value":{"kind":value.get("kind").and_then(Value::as_str).map(|kind|limited(kind,32)),"title":value.get("title").and_then(Value::as_str).map(|title|limited(title,512)),"message":value.get("message").and_then(Value::as_str).map(|message|limited(message,512)),"percentage":value.get("percentage").and_then(Value::as_u64)}})
        }
        "textDocument/publishDiagnostics" => {
            let Some(uri) = params
                .get("uri")
                .and_then(Value::as_str)
                .filter(|uri| uri.len() <= 4096)
            else {
                return;
            };
            let diagnostics:Vec<_>=params.get("diagnostics").and_then(Value::as_array).into_iter().flatten().take(1000).filter_map(|diagnostic| {
                let range:lsp_types::Range=serde_json::from_value(diagnostic.get("range")?.clone()).ok()?;
                Some(json!({"range":range,"severity":diagnostic.get("severity").and_then(Value::as_i64),"message":limited(diagnostic.get("message")?.as_str()?,2048)}))
            }).collect();
            json!({"uri":uri,"version":params.get("version").and_then(Value::as_i64),"diagnostics":diagnostics})
        }
        _ => return,
    };
    shared.event(method, params);
}
pub(super) async fn supervise(shared: Arc<Shared>, child: ManagedChild) -> Result<()> {
    let result = child.wait_or_cancel(shared.stop.clone()).await;
    match result {
        Ok((status, cancelled)) => {
            if matches!(shared.state(), ClientState::Stopping | ClientState::Stopped) {
                shared.set_state(ClientState::Stopped);
                shared.cancel_pending("server stopped");
                shared.stop.cancel();
            } else {
                shared.fail(format!(
                    "server exited {status}{}; stderr: {}",
                    if cancelled {
                        " after transport cancellation"
                    } else {
                        ""
                    },
                    limited(&shared.stderr.text(), 2048)
                ));
            }
            Ok(())
        }
        Err(error) => {
            *shared.supervisor_error.lock().expect("LSP supervisor lock") = Some(error.to_string());
            shared.fail(error.to_string());
            Err(error)
        }
    }
}
pub(super) enum DocumentWork {
    Synchronize {
        snapshot: TextSnapshot,
        save: bool,
        reply: oneshot::Sender<Result<DocumentStamp>>,
    },
    Close {
        origin: DocumentOrigin,
        reply: oneshot::Sender<Result<()>>,
    },
    CloseAll {
        reply: oneshot::Sender<Result<()>>,
    },
}
pub(super) async fn documents(
    shared: Arc<Shared>,
    mut commands: mpsc::Receiver<DocumentWork>,
) -> Result<()> {
    let mut tracker = DocumentTracker::default();
    loop {
        let command = tokio::select! {biased;_=shared.stop.cancelled()=>return Ok(()),command=commands.recv()=>match command { Some(command)=>command,None=>return Ok(()) }};
        let info = shared
            .info
            .get()
            .cloned()
            .ok_or_else(|| protocol("document command before initialization"))?;
        let mut stamp_uri = None;
        let (prepared, reply) = match command {
            DocumentWork::Synchronize {
                snapshot,
                save,
                reply,
            } => {
                if shared.state() != ClientState::Running {
                    let _ = reply.send(Err(DomainError::Cancelled));
                    continue;
                }
                let uri = match &snapshot.origin {
                    DocumentOrigin::File(path) => file_uri(path).ok(),
                    _ => None,
                };
                stamp_uri = uri;
                let prepared = if save {
                    tracker.save(snapshot, info.synchronization, info.encoding)
                } else {
                    tracker.synchronize(snapshot, info.synchronization, info.encoding)
                };
                (prepared, DocumentReply::Stamp(reply))
            }
            DocumentWork::Close { origin, reply } => (
                tracker.close(&origin, info.synchronization),
                DocumentReply::Unit(reply),
            ),
            DocumentWork::CloseAll { reply } => {
                let origins: Vec<_> = tracker
                    .stamps()
                    .keys()
                    .filter_map(|uri| {
                        uri.parse::<Uri>()
                            .ok()
                            .and_then(|uri| uri_path(&uri).ok())
                            .map(DocumentOrigin::File)
                    })
                    .collect();
                let mut messages = vec![];
                for origin in origins {
                    messages.extend(tracker.close(&origin, info.synchronization)?);
                }
                (Ok(messages), DocumentReply::Unit(reply))
            }
        };
        let messages = match prepared {
            Ok(messages) => messages,
            Err(error) => {
                reply.error(error);
                continue;
            }
        };
        // The actor owns replica mutation. Accepted writes finish even if their
        // caller drops the reply future; a wire failure terminates this replica.
        for message in messages {
            if let Err(error) = shared
                .send(json!({"jsonrpc":"2.0","method":message.method,"params":message.params}))
                .await
            {
                let text = error.to_string();
                reply.error(error);
                return Err(shared.error(text));
            }
        }
        let published = tracker
            .stamps()
            .into_iter()
            .filter_map(|(key, stamp)| {
                key.parse::<Uri>()
                    .ok()
                    .and_then(|uri| tracker.mapper(&uri))
                    .map(|mapper| (key, (stamp, mapper)))
            })
            .collect();
        *shared.documents.write().expect("LSP document lock") = published;
        match reply {
            DocumentReply::Stamp(reply) => {
                let stamp = stamp_uri
                    .and_then(|uri| tracker.stamp(&uri))
                    .ok_or_else(|| protocol("synchronized document has no stamp"));
                let _ = reply.send(stamp);
            }
            DocumentReply::Unit(reply) => {
                let _ = reply.send(Ok(()));
            }
        }
    }
}
enum DocumentReply {
    Stamp(oneshot::Sender<Result<DocumentStamp>>),
    Unit(oneshot::Sender<Result<()>>),
}
impl DocumentReply {
    fn error(self, error: DomainError) {
        match self {
            Self::Stamp(reply) => {
                let _ = reply.send(Err(error));
            }
            Self::Unit(reply) => {
                let _ = reply.send(Err(error));
            }
        }
    }
}
