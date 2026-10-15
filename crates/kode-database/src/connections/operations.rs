use super::*;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(60);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) fn info(value: Value, client: &Client, source: &str) -> Result<SessionInfo> {
    let mut info: SessionInfo = serde_json::from_value(value)
        .map_err(|_| error(Failure::Protocol, "Invalid JDBC connection response"))?;
    if info.connection_id.is_empty()
        || info.connection_id.len() > 128
        || info.data_source_id != source
        || [
            &info.product_name,
            &info.product_version,
            &info.driver_name,
            &info.driver_version,
        ]
        .iter()
        .any(|value| value.len() > 8192)
        || [&info.catalog, &info.schema]
            .iter()
            .any(|value| value.as_ref().is_some_and(|value| value.len() > 65536))
    {
        return Err(error(
            Failure::Protocol,
            "Invalid JDBC connection response bounds",
        ));
    }
    info.helper_epoch = client
        .info()
        .ok_or_else(|| error(Failure::Protocol, "JDBC helper epoch is unavailable"))?
        .helper_epoch
        .clone();
    Ok(info)
}
async fn cleanup(client: &Client, attempt: &str) -> Result<()> {
    let value = client
        .request_with_timeout(
            "connection.close",
            json!({"attemptId":attempt}),
            true,
            CLOSE_TIMEOUT,
            &CancellationToken::new(),
        )
        .await?;
    if value.get("closed").and_then(Value::as_bool) != Some(true) {
        return Err(error(
            Failure::Protocol,
            "Connection close was not confirmed",
        ));
    }
    Ok(())
}
fn confirmed(result: &Result<()>) -> bool {
    result.is_ok()
        || result.as_ref().is_err_and(|error| {
            error
                .bridge
                .remote_data()
                .and_then(|data| data.get("closed"))
                .and_then(Value::as_bool)
                == Some(true)
        })
}
impl Shared {
    pub(super) async fn open(
        self: Arc<Self>,
        definition: DataSourceDefinition,
        password: Option<Secret>,
        token: CancellationToken,
        tasks: TaskTracker,
    ) -> Result<SessionInfo> {
        let source = definition.id.clone();
        let attempt = self.attempt()?;
        {
            let mut entries = self.entries.lock().expect("connection state");
            if let Some(entry) = entries.get_mut(&source) {
                if entry
                    .client
                    .as_ref()
                    .is_some_and(|client| client.state() != crate::bridge::client::State::Running)
                {
                    entry.client = None;
                    entry.snapshot.phase = Phase::Error;
                    entry.snapshot.session = None;
                    entry.snapshot.unknown_outcome = true;
                }
                if entry.snapshot.phase == Phase::Connected
                    && entry.definition.as_ref() == Some(&definition)
                    && password.is_none()
                {
                    return entry.snapshot.session.clone().ok_or_else(|| {
                        error(Failure::Protocol, "Connected datasource has no session")
                    });
                }
                if matches!(
                    entry.snapshot.phase,
                    Phase::Connected | Phase::Connecting | Phase::Closing
                ) || entry.client.is_some()
                {
                    return Err(error(
                        Failure::Busy,
                        "Datasource is open or closing; disconnect before connecting again",
                    ));
                }
            }
            if !entries.contains_key(&source) && entries.len() >= 4096 {
                return Err(error(Failure::Busy, "Datasource state capacity exhausted"));
            }
            entries.insert(
                source.clone(),
                Entry {
                    snapshot: Snapshot {
                        phase: Phase::Connecting,
                        ..Default::default()
                    },
                    definition: Some(definition.clone()),
                    attempt: attempt.clone(),
                    client: None,
                },
            );
        }
        let mut opened_client = None;
        let result = async {
            let params = self.params(&definition, password, &attempt, &token).await?;
            let client = self.client(&token, &tasks).await?;
            {
                let mut entries = self.entries.lock().expect("connection state");
                let entry = entries.get_mut(&source).expect("opening entry");
                if entry.attempt != attempt || entry.snapshot.phase != Phase::Connecting {
                    return Err(error(
                        Failure::Cancelled,
                        "Datasource disconnected during preparation",
                    ));
                }
                entry.client = Some(client.clone());
            }
            opened_client = Some(client.clone());
            let value = client
                .request_with_timeout("connection.open", params, true, OPERATION_TIMEOUT, &token)
                .await?;
            let result = info(value, &client, &source)?;
            let mut entries = self.entries.lock().expect("connection state");
            let entry = entries.get_mut(&source).expect("opening entry");
            if entry.attempt != attempt
                || entry.snapshot.phase != Phase::Connecting
                || client.state() != crate::bridge::client::State::Running
            {
                return Err(error(
                    Failure::Transport,
                    "Datasource disconnected before connection confirmation",
                ));
            }
            entry.snapshot.phase = Phase::Connected;
            entry.snapshot.session = Some(result.clone());
            entry.snapshot.revision = entry.snapshot.revision.saturating_add(1);
            Ok(result)
        }
        .await;
        if let Err(failure) = &result {
            let closed = match opened_client {
                Some(ref client) => confirmed(&cleanup(client, &attempt).await),
                None => true,
            };
            let mut entries = self.entries.lock().expect("connection state");
            if let Some(entry) = entries.get_mut(&source).filter(|entry| {
                entry.attempt == attempt && entry.snapshot.phase != Phase::Disconnected
            }) {
                let dead = entry
                    .client
                    .as_ref()
                    .is_none_or(|client| client.state() != crate::bridge::client::State::Running);
                entry.snapshot.phase = if closed || dead {
                    Phase::Error
                } else {
                    Phase::Closing
                };
                entry.snapshot.error = Some(failure.clone());
                entry.snapshot.unknown_outcome = failure.bridge.unknown_outcome || !closed;
                entry.snapshot.session = None;
                entry.snapshot.revision = entry.snapshot.revision.saturating_add(1);
                if closed || dead {
                    entry.client = None;
                }
            }
        }
        result
    }
    pub(super) async fn test(
        self: Arc<Self>,
        definition: DataSourceDefinition,
        password: Option<Secret>,
        token: CancellationToken,
        tasks: TaskTracker,
    ) -> Result<TestResult> {
        let attempt = self.attempt()?;
        let params = self.params(&definition, password, &attempt, &token).await?;
        let client = self.client(&token, &tasks).await?;
        let result = client
            .request_with_timeout("connection.test", params, true, OPERATION_TIMEOUT, &token)
            .await;
        match result {
            Ok(value) => {
                let result: TestResult = serde_json::from_value(value)
                    .map_err(|_| error(Failure::Protocol, "Invalid connection test response"))?;
                if !result.success
                    || [
                        &result.product_name,
                        &result.product_version,
                        &result.driver_name,
                        &result.driver_version,
                    ]
                    .iter()
                    .any(|value| value.len() > 8192)
                {
                    return Err(error(
                        Failure::Protocol,
                        "Invalid connection test response bounds",
                    ));
                }
                Ok(result)
            }
            Err(failure) => {
                if failure.kind != Failure::Remote {
                    let _ = cleanup(&client, &attempt).await;
                }
                Err(failure.into())
            }
        }
    }
    pub(super) async fn close(&self, source: &str, token: CancellationToken) -> Result<()> {
        let (client, attempt) = {
            let mut entries = self.entries.lock().expect("connection state");
            let Some(entry) = entries.get_mut(source) else {
                return Ok(());
            };
            entry.snapshot.phase = Phase::Closing;
            entry.snapshot.revision = entry.snapshot.revision.saturating_add(1);
            (entry.client.clone(), entry.attempt.clone())
        };
        // Once requested, closure is tracked through its bounded authoritative
        // reply even if the UI stops waiting. Cancellation never means rollback.
        let result = if let Some(client) = client {
            cleanup(&client, &attempt).await
        } else {
            Ok(())
        };
        let mut entries = self.entries.lock().expect("connection state");
        if let Some(entry) = entries
            .get_mut(source)
            .filter(|entry| entry.attempt == attempt)
        {
            match &result {
                Ok(()) => {
                    entry.snapshot.phase = Phase::Disconnected;
                    entry.snapshot.session = None;
                    entry.snapshot.error = None;
                    entry.client = None;
                    entry.definition = None;
                }
                Err(failure) => {
                    let dead = entry.client.as_ref().is_none_or(|client| {
                        client.state() != crate::bridge::client::State::Running
                    });
                    let closed = confirmed(&result);
                    entry.snapshot.phase = if closed {
                        Phase::Disconnected
                    } else if dead {
                        Phase::Error
                    } else {
                        Phase::Closing
                    };
                    if dead || closed {
                        entry.client = None;
                        entry.snapshot.session = None;
                    }
                    entry.snapshot.error = Some(failure.clone());
                    entry.snapshot.unknown_outcome |= failure.bridge.unknown_outcome;
                }
            }
            entry.snapshot.revision = entry.snapshot.revision.saturating_add(1);
        }
        let _ = token;
        result
    }
    pub(super) async fn command(
        &self,
        source: &str,
        method: &'static str,
        mut params: Value,
        token: CancellationToken,
    ) -> Result<SessionInfo> {
        let (client, session, attempt) = {
            let entries = self.entries.lock().expect("connection state");
            let entry = entries
                .get(source)
                .filter(|entry| entry.snapshot.phase == Phase::Connected)
                .ok_or_else(|| {
                    error(
                        Failure::Unavailable,
                        "Datasource is disconnected; connect explicitly",
                    )
                })?;
            (
                entry
                    .client
                    .clone()
                    .ok_or_else(|| error(Failure::Transport, "Connection helper is unavailable"))?,
                entry
                    .snapshot
                    .session
                    .clone()
                    .ok_or_else(|| error(Failure::Protocol, "Connection has no session"))?,
                entry.attempt.clone(),
            )
        };
        if client.state() != crate::bridge::client::State::Running
            || client
                .info()
                .is_none_or(|hello| hello.helper_epoch != session.helper_epoch)
        {
            return Err(error(
                Failure::Transport,
                "Connection belongs to a stopped helper; reconnect explicitly",
            ));
        }
        params["connectionId"] = json!(session.connection_id);
        let result = client
            .request_with_timeout(method, params, true, OPERATION_TIMEOUT, &token)
            .await;
        let mut entries = self.entries.lock().expect("connection state");
        let entry = entries
            .get_mut(source)
            .filter(|entry| entry.attempt == attempt && entry.snapshot.phase == Phase::Connected);
        match result {
            Ok(value) => {
                let state = info(value, &client, source)?;
                if let Some(entry) = entry
                    && entry.snapshot.phase == Phase::Connected
                    && entry.snapshot.session.as_ref().is_some_and(|current| {
                        current.connection_id == state.connection_id
                            && current.state_revision <= state.state_revision
                    })
                {
                    entry.snapshot.session = Some(state.clone());
                    entry.snapshot.error = None;
                    entry.snapshot.revision = entry.snapshot.revision.saturating_add(1);
                }
                Ok(state)
            }
            Err(failure) => {
                if let Some(entry) = entry {
                    if let Some(state) = failure
                        .remote_data()
                        .and_then(|data| data.get("state"))
                        .and_then(|state| info(state.clone(), &client, source).ok())
                    {
                        if entry.snapshot.session.as_ref().is_some_and(|current| {
                            current.connection_id != state.connection_id
                                || current.state_revision > state.state_revision
                        }) {
                            return Err(failure.into());
                        }
                        entry.snapshot.session = Some(state);
                    }
                    entry.snapshot.error = Some(failure.clone().into());
                    entry.snapshot.unknown_outcome |= failure.unknown_outcome;
                    if let Some(state) = failure.remote_data().and_then(|data| data.get("state"))
                        && state.get("contextUnknown").and_then(Value::as_bool) == Some(true)
                        && let Some(session) = entry.snapshot.session.as_mut()
                    {
                        session.context_unknown = true;
                        session.transaction_usable = false;
                        session.context_revision = session.context_revision.max(
                            state
                                .get("contextRevision")
                                .and_then(Value::as_u64)
                                .unwrap_or_default(),
                        );
                        session.state_revision = session.state_revision.max(
                            state
                                .get("stateRevision")
                                .and_then(Value::as_u64)
                                .unwrap_or_default(),
                        );
                    }
                    if (failure.unknown_outcome || !matches!(failure.kind, Failure::Remote))
                        && let Some(session) = entry.snapshot.session.as_mut()
                    {
                        session.transaction_usable = false;
                    }
                    if !matches!(failure.kind, Failure::Remote) {
                        // An abandoned wait can leave JDBC running. Further
                        // transaction commands require explicit disconnect;
                        // they must not race an operation with no final reply.
                        entry.snapshot.phase = Phase::Error;
                        entry.snapshot.session = None;
                    }
                    entry.snapshot.revision = entry.snapshot.revision.saturating_add(1);
                }
                Err(failure.into())
            }
        }
    }
}
