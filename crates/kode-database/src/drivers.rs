//! Native installation state and client. Construction is inert; feature progress
//! has its own consumer and never drains SQL notifications.
use crate::{
    bridge::{
        client::{BridgeError, Client, Failure, Options, Result, State},
        runtime::Runtime,
    },
    config::DatabasePaths,
    model::DriverSpec,
};
use kode_platform::jobs::CancellationToken;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::task::TaskTracker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Resolving,
    Downloading,
    Publishing,
    Cancelling,
    Ready,
    Cancelled,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub completed_bytes: u64,
    pub total_bytes: Option<u64>,
}
impl Default for Progress {
    fn default() -> Self {
        Self {
            phase: Phase::Queued,
            completed_bytes: 0,
            total_bytes: None,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub provider_id: String,
    pub resolved_version: String,
    pub classpath_digest: String,
    pub source: String,
    pub manifest_path: PathBuf,
    pub classpath: Vec<PathBuf>,
    #[serde(default)]
    pub reused: bool,
    #[serde(default)]
    pub cleanup_warning: bool,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspected {
    #[serde(default)]
    pub resolved_version: Option<String>,
    #[serde(default)]
    pub manifest_path: Option<PathBuf>,
    #[serde(default)]
    pub load_errors: Vec<String>,
    #[serde(skip)]
    pub helper_epoch: String,
    pub provider_id: String,
    pub classpath_digest: String,
    pub loader_id: String,
    pub driver_classes: Vec<String>,
    pub selected_driver_class: String,
    pub driver_major_version: i32,
    pub driver_minor_version: i32,
    pub source: String,
    pub classpath: Vec<PathBuf>,
    #[serde(default)]
    pub restart_required: bool,
    #[serde(default)]
    pub restart_recommended: bool,
}
struct Epoch {
    epoch: String,
    cancel: CancellationToken,
}
struct Inner {
    runtime: Runtime,
    pending: Mutex<BTreeMap<String, watch::Sender<Progress>>>,
    next_id: AtomicU64,
    epoch: tokio::sync::Mutex<Option<Epoch>>,
    stop: CancellationToken,
    tasks: TaskTracker,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.cancel();
        self.tasks.close();
    }
}
#[derive(Clone)]
pub struct Installer {
    inner: Arc<Inner>,
}
impl Installer {
    pub fn configure(mut options: Options, paths: &DatabasePaths) -> Result<Options> {
        let path_text = |path: &std::path::Path| {
            path.to_str().map(str::to_owned).ok_or_else(|| {
                BridgeError::new(Failure::Unavailable, "JDBC plugin paths must be Unicode")
            })
        };
        options
            .helper_environment
            .retain(|(key, _)| key != "KODE_RS_JDBC_HOME" && key != "KODE_LEGACY_JDBC_HOME");
        options
            .helper_environment
            .push(("KODE_RS_JDBC_HOME".into(), path_text(&paths.plugin_root)?));
        options.helper_environment.push((
            "KODE_LEGACY_JDBC_HOME".into(),
            paths
                .legacy_plugin_root
                .as_ref()
                .map(|path| path_text(path))
                .transpose()?
                .unwrap_or_default(),
        ));
        Ok(options)
    }
    pub fn new(runtime: Runtime) -> Self {
        Self {
            inner: Arc::new(Inner {
                runtime,
                pending: Mutex::new(BTreeMap::new()),
                next_id: AtomicU64::new(1),
                epoch: tokio::sync::Mutex::new(None),
                stop: CancellationToken::new(),
                tasks: TaskTracker::new(),
            }),
        }
    }
    pub fn runtime(&self) -> &Runtime {
        &self.inner.runtime
    }
    async fn client(&self, cancel: &CancellationToken) -> Result<Client> {
        if self.inner.stop.is_cancelled() {
            return Err(BridgeError::new(
                Failure::Cancelled,
                "Driver service stopped",
            ));
        }
        let client = self.inner.runtime.get(cancel).await?;
        let info = client
            .info()
            .ok_or_else(|| BridgeError::new(Failure::Protocol, "JDBC helper has no hello"))?;
        let mut epoch = tokio::select! {biased;_=cancel.cancelled()=>return Err(BridgeError::new(Failure::Cancelled,"Driver operation cancelled")),epoch=self.inner.epoch.lock()=>epoch};
        if epoch
            .as_ref()
            .is_some_and(|epoch| epoch.epoch == info.helper_epoch)
        {
            return Ok(client);
        }
        if let Some(old) = epoch.take() {
            old.cancel.cancel();
        }
        let mut events = client.subscribe_events("driver.progress")?;
        let stop = self.inner.stop.child_token();
        let token = stop.clone();
        let weak = Arc::downgrade(&self.inner);
        self.inner.tasks.spawn(async move {
            loop {
                let event=tokio::select! {biased;_=token.cancelled()=>break,event=events.recv()=>match event {Some(event)=>event,None=>break}};
                let Some(owner)=weak.upgrade()else{break;};
                let Some(id)=event.params.get("resolutionId").and_then(Value::as_str).filter(|id|id.len()<=128)else{continue;};
                let sender=owner.pending.lock().expect("driver progress").get(id).cloned();drop(owner);
                if let Some(sender)=sender {let phase=match event.params.get("phase").and_then(Value::as_str){Some("resolve-version"|"resolve-graph")=>Phase::Resolving,Some("download")=>Phase::Downloading,Some("publish")=>Phase::Publishing,_=>continue};
                    sender.send_if_modified(|progress|{if matches!(progress.phase,Phase::Cancelling|Phase::Ready|Phase::Cancelled|Phase::Failed){return false;}*progress=Progress {phase,completed_bytes:event.params.get("completedBytes").and_then(Value::as_u64).unwrap_or(0),total_bytes:event.params.get("totalBytes").and_then(Value::as_u64)};true});
                }
            }
        });
        *epoch = Some(Epoch {
            epoch: info.helper_epoch.clone(),
            cancel: stop,
        });
        Ok(client)
    }
    /// Cancellation waits for the original operation's final reply, rather than
    /// treating the separate cancel acknowledgement as completed cleanup.
    pub async fn resolve(
        &self,
        spec: &DriverSpec,
        progress: watch::Sender<Progress>,
        cancel: &CancellationToken,
    ) -> Result<Installed> {
        let value = serde_json::to_value(spec).map_err(|_| {
            BridgeError::new(
                Failure::Protocol,
                "Driver spec contains invalid persistent fields",
            )
        })?;
        let client = match self.client(cancel).await {
            Ok(client) => client,
            Err(error) => {
                progress.send_modify(|value| {
                    value.phase = if error.kind == Failure::Cancelled {
                        Phase::Cancelled
                    } else {
                        Phase::Failed
                    }
                });
                return Err(error);
            }
        };
        let number = self
            .inner
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| BridgeError::new(Failure::Protocol, "Driver resolution IDs exhausted"))?;
        let id = format!("install-{number}");
        {
            let mut pending = self.inner.pending.lock().expect("driver progress");
            if pending.len() >= 10 {
                return Err(BridgeError::new(
                    Failure::Busy,
                    "Driver resolution queue is full",
                ));
            }
            pending.insert(id.clone(), progress.clone());
        }
        let mut active = Active {
            owner: Arc::downgrade(&self.inner),
            client: client.clone(),
            id: id.clone(),
            finished: false,
            tasks: self.inner.tasks.clone(),
        };
        let never_cancel_wait = CancellationToken::new();
        let request = client.request_with_timeout(
            "driver.resolve",
            json!({"resolutionId":id,"driver":value}),
            true,
            Duration::from_secs(600),
            &never_cancel_wait,
        );
        tokio::pin!(request);
        let result = tokio::select! {biased;
            result=&mut request=>result,
            _=cancel.cancelled()=>{
                progress.send_modify(|value|value.phase=Phase::Cancelling);
                // The original final reply remains authoritative if publishing
                // won the cancellation race. Do not replay either request.
                let _=cancel_resolution(&client,&id).await;
                request.await
            },
        };
        active.finished = result.is_ok()
            || result
                .as_ref()
                .is_err_and(|error| error.kind == Failure::Remote);
        let result = result.map_err(driver_error).and_then(|value| {
            let installed: Installed = serde_json::from_value(value).map_err(|_| {
                BridgeError::new(Failure::Protocol, "Invalid driver install response")
            })?;
            if installed.classpath.is_empty()
                || installed.classpath.len() > 256
                || installed.resolved_version.len() > 256
                || !digest(&installed.classpath_digest)
            {
                return Err(BridgeError::new(
                    Failure::Protocol,
                    "Invalid driver install bounds or digest",
                ));
            }
            Ok(installed)
        });
        progress.send_modify(|value| {
            value.phase = match &result {
                Ok(_) => Phase::Ready,
                Err(error) if error.remote_code == Some(-32800) => Phase::Cancelled,
                _ => Phase::Failed,
            }
        });
        result
    }
    pub async fn inspect(
        &self,
        provider: &str,
        classpath: &[PathBuf],
        driver_class: &str,
        cancel: &CancellationToken,
    ) -> Result<Inspected> {
        self.inspect_params(
            json!({"providerId":provider,"classpath":classpath,"driverClass":driver_class}),
            cancel,
        )
        .await
    }
    /// Inspect an installed/configured spec without downloading or creating its
    /// install root. This is also the connection service's version-aware lookup.
    pub async fn inspect_spec(
        &self,
        spec: &DriverSpec,
        cancel: &CancellationToken,
    ) -> Result<Inspected> {
        let driver = serde_json::to_value(spec)
            .map_err(|_| BridgeError::new(Failure::Protocol, "Invalid driver specification"))?;
        self.inspect_params(json!({"driver":driver}), cancel).await
    }
    async fn inspect_params(&self, params: Value, cancel: &CancellationToken) -> Result<Inspected> {
        let client = self.client(cancel).await?;
        let value = client
            .request("driver.inspect", params, false, cancel)
            .await
            .map_err(driver_error)?;
        let mut info: Inspected = serde_json::from_value(value).map_err(|_| {
            BridgeError::new(Failure::Protocol, "Invalid driver inspection response")
        })?;
        if info.loader_id.is_empty()
            || info.loader_id.len() > 128
            || info.driver_classes.is_empty()
            || info.driver_classes.len() > 128
            || info.driver_classes.iter().any(|name| name.len() > 1024)
            || info.load_errors.len() > 128
            || info.load_errors.iter().any(|name| name.len() > 256)
            || info
                .resolved_version
                .as_ref()
                .is_some_and(|version| version.len() > 256)
            || info.selected_driver_class.len() > 1024
            || info.classpath.len() > 256
            || info.classpath_digest != "application-classpath" && !digest(&info.classpath_digest)
        {
            return Err(BridgeError::new(
                Failure::Protocol,
                "Invalid driver inspection bounds or digest",
            ));
        }
        info.helper_epoch = client
            .info()
            .ok_or_else(|| BridgeError::new(Failure::Protocol, "JDBC helper has no epoch"))?
            .helper_epoch
            .clone();
        Ok(info)
    }
    pub async fn stop_progress(&self) {
        self.inner.stop.cancel();
        if let Some(epoch) = self.inner.epoch.lock().await.take() {
            epoch.cancel.cancel();
        }
        self.inner.tasks.close();
        self.inner.tasks.wait().await;
    }
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn driver_error(mut error: BridgeError) -> BridgeError {
    error.message = match error.remote_code {
        Some(-32010) => {
            "Driver operation failed; check the source, complete classpath and available disk space"
        }
        Some(-32011) => "Restart the database helper before changing this native driver",
        Some(-32013) => {
            "This driver needs a vendor JAR; download it from the vendor page and select its local classpath"
        }
        Some(-32014) => "The selected driver version is not installed; install its JARs first",
        Some(-32003) => "Driver installation queue is full; retry after current work finishes",
        Some(-32602) => "Check the driver coordinates, class and complete JAR classpath",
        Some(-32800) => "Driver installation cancelled",
        _ => error.message,
    };
    error
}
async fn cancel_resolution(client: &Client, id: &str) -> Result<Value> {
    client
        .request_with_timeout(
            "driver.cancel",
            json!({"resolutionId":id}),
            false,
            Duration::from_secs(2),
            &CancellationToken::new(),
        )
        .await
}
struct Active {
    owner: Weak<Inner>,
    client: Client,
    id: String,
    finished: bool,
    tasks: TaskTracker,
}
impl Drop for Active {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner
                .pending
                .lock()
                .expect("driver progress")
                .remove(&self.id);
        }
        if !self.finished && self.client.state() == State::Running {
            let client = self.client.clone();
            let id = self.id.clone();
            self.tasks.spawn(async move {
                let _ = cancel_resolution(&client, &id).await;
            });
        }
    }
}
