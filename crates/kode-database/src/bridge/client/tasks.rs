use super::*;
use futures_util::FutureExt;
use std::{future::Future, panic::AssertUnwindSafe};
use tokio::process::{ChildStdin, ChildStdout};
pub(super) fn spawn(
    tracker: &TaskTracker,
    shared: Arc<Shared>,
    future: impl Future<Output = Result<()>> + Send + 'static,
) {
    tracker.spawn(async move {
        match AssertUnwindSafe(future).catch_unwind().await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => shared.fail(error),
            Err(_) => shared.fail(BridgeError::new(
                Failure::Transport,
                "JDBC supervisor task panicked",
            )),
        }
    });
}
pub(super) async fn write(
    shared: Arc<Shared>,
    output: ChildStdin,
    mut ordinary: mpsc::Receiver<request::Frame>,
    mut control: mpsc::Receiver<request::Frame>,
) -> Result<()> {
    let mut writer = protocol::FrameWriter::new(output);
    loop {
        let frame = tokio::select! {biased;_=shared.stop.cancelled()=>return Ok(()),frame=control.recv()=>frame,frame=ordinary.recv()=>frame};
        let Some(frame) = frame else { return Ok(()) };
        if frame
            .submitted
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        tokio::select! {biased;_=shared.stop.cancelled()=>return Ok(()),result=tokio::time::timeout(shared.options.write_timeout,writer.write_payload(frame.bytes))=>match result {Ok(Ok(()))=>{},_=>return Err(BridgeError::new(Failure::Transport,"JDBC pipe write failed or timed out"))}}
    }
}
pub(super) async fn read(shared: Arc<Shared>, input: ChildStdout) -> Result<()> {
    // A scope guard signals EOF even on malformed input/panic, so the reaper
    // never races buffered final responses after a normal process exit.
    struct Done(CancellationToken);
    impl Drop for Done {
        fn drop(&mut self) {
            self.0.cancel();
        }
    }
    let _done = Done(shared.reader_done.clone());
    let result = read_loop(shared.clone(), input).await;
    // Publish the exact protocol failure before signaling reader_done; the
    // natural-exit supervisor must not race it into a generic transport error.
    if let Err(error) = &result {
        shared.fail(error.clone());
    }
    result
}
async fn read_loop(shared: Arc<Shared>, input: ChildStdout) -> Result<()> {
    let mut reader = protocol::FrameReader::new(input);
    loop {
        let value = tokio::select! {biased;_=shared.stop.cancelled()=>return Ok(()),value=reader.read()=>value.map_err(|_|BridgeError::new(Failure::Protocol,"Corrupt or truncated JDBC frame; helper disconnected"))?};
        let Some(value) = value else {
            shared.fail(BridgeError::new(
                Failure::Transport,
                "JDBC helper closed stdout; reconnect explicitly",
            ));
            return Ok(());
        };
        if let Some(method) = value.get("method").and_then(Value::as_str) {
            if value.get("id").is_some() {
                return Err(BridgeError::new(
                    Failure::Protocol,
                    "JDBC helper sent an unsupported request",
                ));
            }
            let size = serde_json::to_vec(&value).map_or(protocol::MAX_FRAME_BYTES, |v| v.len());
            shared.receive_event(
                Event {
                    method: method.into(),
                    params: value.get("params").cloned().unwrap_or_else(|| json!({})),
                },
                size,
            )?;
        } else {
            let id = value["id"].as_str().expect("validated string ID");
            if let Some(waiting) = shared.pending.lock().expect("bridge pending").remove(id) {
                let result = if let Some(error) = value.get("error") {
                    let code = error["code"].as_i64();
                    let not_started = error["data"]["notStarted"].as_bool() == Some(true)
                        || matches!(code, Some(-32601 | -32602 | -32001 | -32002));
                    Err(BridgeError {
                        kind: Failure::Remote,
                        unknown_outcome: waiting.mutating && !not_started,
                        remote_code: code,
                        message: "JDBC helper rejected the operation",
                        remote_data: error.get("data").cloned(),
                        java_unavailable: false,
                    })
                } else {
                    Ok(value["result"].clone())
                };
                let _ = waiting.reply.send(result);
            }
            // Timed-out/dropped callers may have late responses. IDs are never
            // reused within this process; no late result can satisfy a new call.
        }
    }
}
pub(super) async fn supervise(shared: Arc<Shared>, child: ManagedChild) -> Result<()> {
    // F03 drains only a bounded raw tail. Never expose it: drivers/JVM launchers
    // may print credentials even when our own code never logs request bodies.
    let result = child.wait_or_cancel(shared.stop.clone()).await;
    if !shared.stop.is_cancelled() {
        let _ = tokio::time::timeout(Duration::from_secs(1), shared.reader_done.cancelled()).await;
    }
    match result {
        Ok(_) => {
            shared.fail(BridgeError::new(
                Failure::Transport,
                "JDBC helper exited; reconnect explicitly",
            ));
            Ok(())
        }
        Err(_) => Err(BridgeError::new(
            Failure::Transport,
            "JDBC helper could not be reaped cleanly",
        )),
    }
}
