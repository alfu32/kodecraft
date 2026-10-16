//! Lazy workspace ownership and runtime discovery. Construction performs no I/O.
use super::client::{BridgeError, Client, Failure, Options, Result, State};
use kode_platform::{
    jobs::CancellationToken,
    paths::ResourcePaths,
    process::{ProcessFactory, ProcessSpec, run_capture},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::task::TaskTracker;

#[derive(Clone)]
pub struct Installation {
    pub java: PathBuf,
    pub helper_candidates: Vec<PathBuf>,
}
impl Installation {
    pub fn from_environment(configured_java: Option<PathBuf>, resources: &ResourcePaths) -> Self {
        let java = configured_java
            .or_else(|| {
                std::env::var_os("KODE_JAVA")
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
            })
            .or_else(|| {
                std::env::var_os("JAVA_HOME")
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        PathBuf::from(s).join("bin").join(if cfg!(windows) {
                            "java.exe"
                        } else {
                            "java"
                        })
                    })
            })
            .unwrap_or_else(|| PathBuf::from("java"));
        let helper_candidates =
            if let Some(path) = std::env::var_os("KODE_JDBC_HELPER").filter(|s| !s.is_empty()) {
                vec![PathBuf::from(path)]
            } else {
                // The legacy Kotlin installation contains no compatible bridge JAR.
                [
                    resources.explicit.as_ref(),
                    resources.rust_home.as_ref(),
                    Some(&resources.executable_dir),
                ]
                .into_iter()
                .flatten()
                .map(|root| root.join("jdbc/kode-jdbc-bridge.jar"))
                .collect()
            };
        Self {
            java,
            helper_candidates,
        }
    }
    async fn spec(
        &self,
        factory: Arc<dyn ProcessFactory>,
        cancel: &CancellationToken,
        timeout: Duration,
    ) -> Result<ProcessSpec> {
        if cancel.is_cancelled() {
            return Err(BridgeError::new(
                Failure::Cancelled,
                "JDBC startup cancelled",
            ));
        }
        let probe_cancel = CancellationToken::new();
        struct CancelOnDrop(CancellationToken);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let _guard = CancelOnDrop(probe_cancel.clone());
        let mut probe = ProcessSpec::new(self.java.clone());
        probe.args.push("-version".into());
        let mut probe_task = tokio::spawn(async move {
            run_capture(probe, vec![], probe_cancel, factory.as_ref()).await
        });
        let captured = tokio::select! {biased;
            _=cancel.cancelled()=>None,
            result=tokio::time::timeout(timeout,&mut probe_task)=>Some(result),
        };
        let output = match captured {
            None => {
                _guard.0.cancel();
                let _ = probe_task.await;
                return Err(BridgeError::new(
                    Failure::Cancelled,
                    "JDBC startup cancelled",
                ));
            }
            Some(Err(_)) => {
                _guard.0.cancel();
                let _ = probe_task.await;
                return Err(
                    BridgeError::new(Failure::Timeout, "Java version check timed out")
                        .requiring_java(),
                );
            }
            Some(Ok(Ok(Ok(output)))) => output,
            _ => {
                return Err(BridgeError::new(
                    Failure::Unavailable,
                    "Java is unavailable; configure KODE_JAVA, JAVA_HOME, or install Java 21 or newer on PATH",
                ).requiring_java());
            }
        };
        // Do not surface raw launcher diagnostics: JAVA_TOOL_OPTIONS or a
        // launcher wrapper may print credentials. The bounded probe is ephemeral.
        let version = java_major(&output.stderr)
            .or_else(|| java_major(&String::from_utf8_lossy(&output.stdout)));
        if !output.status.success()
            || output.cancelled
            || output.truncated
            || version.is_none_or(|v| v < 21)
        {
            return Err(BridgeError::new(
                Failure::Unavailable,
                "Java 21 or newer is required; update KODE_JAVA or JAVA_HOME",
            )
            .requiring_java());
        }
        let mut helper = None;
        for path in &self.helper_candidates {
            match tokio::fs::metadata(path).await {
                Ok(meta) if meta.is_file() => {
                    helper = Some(path);
                    break;
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(BridgeError::new(
                        Failure::Unavailable,
                        "Cannot inspect JDBC helper; check its file permissions",
                    ));
                }
            }
        }
        let helper=helper.ok_or_else(||BridgeError::new(Failure::Unavailable,"JDBC helper is missing; install jdbc/kode-jdbc-bridge.jar or configure KODE_JDBC_HELPER"))?;
        let mut spec = ProcessSpec::new(self.java.clone());
        spec.args = vec![
            "-jar".into(),
            helper
                .to_str()
                .ok_or_else(|| {
                    BridgeError::new(Failure::Unavailable, "JDBC helper path must be Unicode")
                })?
                .into(),
        ];
        Ok(spec)
    }
}
pub fn java_major(text: &str) -> Option<u32> {
    for line in text.lines() {
        let line = line.trim();
        if !(line.starts_with("java version \"") || line.starts_with("openjdk version \"")) {
            continue;
        }
        let version = line.split('"').nth(1)?;
        let mut parts = version.split(['.', '-', '+']);
        let first = parts.next()?.parse::<u32>().ok()?;
        return if first == 1 {
            parts.next()?.parse().ok()
        } else {
            Some(first)
        };
    }
    None
}
struct Inner {
    installation: Installation,
    managed_java: Option<PathBuf>,
    options: Options,
    factory: Arc<dyn ProcessFactory>,
    client: tokio::sync::Mutex<Option<Client>>,
    stop: CancellationToken,
    last_error: Mutex<Option<BridgeError>>,
    startup_tasks: TaskTracker,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Ok(slot) = self.client.try_lock()
            && let Some(client) = slot.as_ref()
        {
            client.cancel();
        }
        self.startup_tasks.close();
    }
}
#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}
impl Runtime {
    pub fn new(
        installation: Installation,
        options: Options,
        factory: Arc<dyn ProcessFactory>,
    ) -> Self {
        Self::configured(installation, options, factory, None)
    }
    pub fn new_with_managed_java(
        installation: Installation,
        options: Options,
        factory: Arc<dyn ProcessFactory>,
        root: PathBuf,
    ) -> Self {
        Self::configured(installation, options, factory, Some(root))
    }
    fn configured(
        installation: Installation,
        options: Options,
        factory: Arc<dyn ProcessFactory>,
        managed_java: Option<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                installation,
                managed_java,
                options,
                factory,
                client: tokio::sync::Mutex::new(None),
                stop: CancellationToken::new(),
                last_error: Mutex::new(None),
                startup_tasks: TaskTracker::new(),
            }),
        }
    }
    /// A completed explicit runtime install resolves only the missing-Java hint;
    /// it does not start a helper or retry any database operation.
    pub fn acknowledge_java_runtime_install(&self) {
        let mut error = self.inner.last_error.lock().expect("JDBC runtime error");
        if error
            .as_ref()
            .is_some_and(BridgeError::requires_java_runtime)
        {
            *error = None;
        }
    }
    pub fn last_error(&self) -> Option<BridgeError> {
        self.inner
            .last_error
            .lock()
            .expect("JDBC runtime error")
            .clone()
    }
    /// Start/reuse only on explicit database work. Failed epochs are retained;
    /// restart must be explicit, and no SQL/transaction operation is replayed.
    pub async fn get(&self, cancel: &CancellationToken) -> Result<Client> {
        let result = async {
            let mut slot = tokio::select! {biased;
                _=self.inner.stop.cancelled()=>return Err(BridgeError::new(Failure::Cancelled,"JDBC workspace closed")),
                _=cancel.cancelled()=>return Err(BridgeError::new(Failure::Cancelled,"JDBC startup cancelled")),
                slot=self.inner.client.lock()=>slot,
            };
            if self.inner.stop.is_cancelled(){return Err(BridgeError::new(Failure::Cancelled,"JDBC workspace closed"));}
            if let Some(client) = slot.as_ref() {
                return if client.state() == State::Running {
                    Ok(client.clone())
                } else {
                    Err(client.last_error().unwrap_or_else(|| {
                        BridgeError::new(
                            Failure::Transport,
                            "JDBC helper stopped; restart explicitly",
                        )
                    }))
                };
            };
            // Keep startup alive long enough to reap even when get() itself is
            // dropped. shutdown() joins these tasks as well as the live client.
            let token=self.inner.stop.child_token();
            struct Startup(CancellationToken);impl Drop for Startup {fn drop(&mut self){self.0.cancel();}}
            let _startup=Startup(token.clone());
            let installation=self.inner.installation.clone();let options=self.inner.options.clone();let factory=self.inner.factory.clone();
            let managed_java=self.inner.managed_java.clone();
            let lifetime=self.inner.startup_tasks.clone();
            let mut task=self.inner.startup_tasks.spawn(async move {
                let initial=installation.spec(factory.clone(),&token,options.startup_timeout).await;
                let mut spec=match initial {
                    Err(error) if error.requires_java_runtime() && managed_java.is_some()=>{
                        let root=managed_java.expect("configured managed root");
                        let target=crate::java_runtime::native_target();
                        let java=tokio::task::spawn_blocking(move||target.map(|target|crate::java_runtime::current_java(&root,target)).transpose()).await
                            .map_err(|_|BridgeError::new(Failure::Unavailable,"Managed Java discovery failed").requiring_java())?
                            .map_err(|_|BridgeError::new(Failure::Unavailable,"Installed Java runtime is incomplete; download it again or configure local Java").requiring_java())?.flatten();
                        if let Some(java)=java {Installation{java,helper_candidates:installation.helper_candidates.clone()}.spec(factory.clone(),&token,options.startup_timeout).await?}else{return Err(error)}
                    },
                    result=>result?,
                };
                spec.env.extend(options.helper_environment.clone());
                let client=Client::start(spec,options,factory,&token).await?;
                // Startup can finish just as its caller drops the result. Track
                // the child's actors independently of the returned Client so
                // workspace shutdown still joins that final handoff race.
                let actors=client.cleanup_tracker();
                lifetime.spawn(async move {actors.wait().await;});
                Ok(client)
            });
            let result=tokio::select! {biased;
                _=self.inner.stop.cancelled()=>{_startup.0.cancel();task.await},
                _=cancel.cancelled()=>{_startup.0.cancel();task.await},
                result=&mut task=>result,
            };
            let client=result.map_err(|_|BridgeError::new(Failure::Transport,"JDBC startup supervisor failed"))??;
            if self.inner.stop.is_cancelled()||cancel.is_cancelled(){let _=client.shutdown().await;return Err(BridgeError::new(Failure::Cancelled,"JDBC startup cancelled"));}
            *slot = Some(client.clone());
            Ok(client)
        }.await;
        *self.inner.last_error.lock().expect("JDBC runtime error") = result.as_ref().err().cloned();
        result
    }
    pub async fn restart(&self, cancel: &CancellationToken) -> Result<Client> {
        {
            let mut slot = self.inner.client.lock().await;
            if let Some(client) = slot.take() {
                let _ = client.shutdown().await;
            }
        }
        *self.inner.last_error.lock().expect("JDBC runtime error") = None;
        self.get(cancel).await
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.inner.stop.cancel();
        let mut slot = self.inner.client.lock().await;
        let result = match slot.take() {
            Some(client) => client.shutdown().await,
            None => Ok(()),
        };
        drop(slot);
        self.inner.startup_tasks.close();
        self.inner.startup_tasks.wait().await;
        result
    }
}
