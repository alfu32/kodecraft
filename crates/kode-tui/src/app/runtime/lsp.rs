use super::*;
use crate::panels::settings::lsp::{
    LifecycleAction, ServerLifecycle, SettingsAction, SettingsReport,
};
use kode_lsp::{
    catalog::Catalog,
    discovery::DiscoveryOptions,
    install::{HttpFetcher, InstallJob, uninstall},
};
use kode_platform::paths::ResourcePaths;
use std::path::PathBuf;
#[derive(Clone)]
pub struct LspConfiguration {
    pub paths: ResourcePaths,
    pub catalog: Option<PathBuf>,
    pub discovery: DiscoveryOptions,
}
impl LspConfiguration {
    pub fn from_environment(paths: &ResourcePaths) -> Self {
        Self {
            paths: paths.clone(),
            catalog: std::env::var_os("KODE_LSP_CATALOG").map(PathBuf::from),
            discovery: DiscoveryOptions::from_environment(paths, std::env::var_os("KODE_LSP_HOME")),
        }
    }
}
pub(super) struct LspRuntime {
    config: LspConfiguration,
    fetcher: Arc<HttpFetcher>,
    pub lifecycle: Option<Arc<dyn ServerLifecycle>>,
    pub service: Option<Arc<kode_lsp::service::LspService>>,
    editor: Option<Arc<kode_lsp::provider::EditorLsp>>,
    pub documents_busy: bool,
    documents_dirty: bool,
    attachments: std::collections::HashSet<(OwnerId, DocumentId, String)>,
    automatic: Vec<String>,
    reported_errors: HashMap<String, String>,
    revision: u64,
}
struct NativeLspLifecycle {
    service: Arc<kode_lsp::service::LspService>,
    config: LspConfiguration,
    runtime: tokio::runtime::Handle,
}
impl ServerLifecycle for NativeLspLifecycle {
    fn running(&self, id: &str) -> bool {
        self.service.running(id)
    }
    fn apply(
        &self,
        id: &str,
        action: LifecycleAction,
        cancel: &kode_platform::jobs::CancellationToken,
    ) -> Result<String> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if action == LifecycleAction::Stop {
            self.runtime.block_on(self.service.stop(id))?;
            return Ok(format!("Stopped {id}"));
        }
        let (catalog, _) = Catalog::load(&self.config.paths, self.config.catalog.as_deref())?;
        let entry = catalog
            .entry(id)
            .cloned()
            .ok_or_else(|| DomainError::Unsupported(format!("unknown language server: {id}")))?;
        let manifest = self.config.discovery.read_manifest(id)?;
        let command = self
            .config
            .discovery
            .resolve_command(&entry, manifest.as_ref())?;
        if action == LifecycleAction::Restart {
            self.runtime
                .block_on(self.service.restart(entry, command, cancel))?;
        } else {
            self.runtime
                .block_on(self.service.start(entry, command, cancel))?;
        }
        Ok(format!(
            "{} {id}",
            if action == LifecycleAction::Restart {
                "Restarted"
            } else {
                "Started"
            }
        ))
    }
    fn stop_all(&self) -> Result<()> {
        self.runtime.block_on(self.service.shutdown())
    }
}
impl AppRuntime {
    pub fn configure_lsp(&mut self, config: LspConfiguration) -> Result<()> {
        if self.lsp.is_some() {
            return Err(DomainError::Unsupported(
                "LSP settings are already configured; refresh the catalog to reload it".into(),
            ));
        }
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|e| DomainError::Unsupported(format!("LSP runtime unavailable: {e}")))?;
        self.lsp = Some(LspRuntime {
            config,
            fetcher: Arc::new(HttpFetcher::new(handle)?),
            lifecycle: None,
            service: None,
            editor: None,
            documents_busy: false,
            documents_dirty: false,
            attachments: Default::default(),
            automatic: vec![],
            reported_errors: HashMap::new(),
            revision: 0,
        });
        self.bind_builtin_lsp()
    }
    pub fn set_lsp_lifecycle(&mut self, lifecycle: Arc<dyn ServerLifecycle>) -> Result<()> {
        if self.state.lsp_settings.busy.is_some()
            || self
                .lsp_service()
                .is_some_and(|service| service.has_running_servers())
        {
            return Err(DomainError::Unsupported("stop active servers and wait for the current action before replacing their process service".into()));
        }
        let runtime = self
            .lsp
            .as_mut()
            .ok_or_else(|| DomainError::Unsupported("LSP settings are not configured".into()))?;
        runtime.lifecycle = Some(lifecycle);
        runtime.service = None;
        if let Some(editor) = runtime.editor.take() {
            editor.stop();
        }
        self.state.lsp_settings.request_refresh();
        Ok(())
    }
    pub fn lsp_service(&self) -> Option<Arc<kode_lsp::service::LspService>> {
        self.lsp
            .as_ref()
            .and_then(|runtime| runtime.service.clone())
    }
    pub(super) fn editor_lsp(&self) -> Option<Arc<kode_lsp::provider::EditorLsp>> {
        self.lsp.as_ref().and_then(|runtime| runtime.editor.clone())
    }
    pub(super) fn bind_builtin_lsp(&mut self) -> Result<()> {
        let Some(runtime) = &mut self.lsp else {
            return Ok(());
        };
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|e| DomainError::Unsupported(e.to_string()))?;
        let service = Arc::new(kode_lsp::service::LspService::new(
            self.state.project.root.clone(),
            Some(self.notifications.clone()),
        ));
        runtime.editor = Some(Arc::new(kode_lsp::provider::EditorLsp::new(
            service.clone(),
            self.state.generation,
            handle.clone(),
        )));
        runtime.documents_busy = false;
        runtime.documents_dirty = true;
        runtime.attachments.clear();
        runtime.automatic.clear();
        runtime.reported_errors.clear();
        runtime.lifecycle = Some(Arc::new(NativeLspLifecycle {
            service: service.clone(),
            config: runtime.config.clone(),
            runtime: handle,
        }));
        runtime.service = Some(service);
        runtime.revision = 0;
        self.state.lsp_settings.request_refresh();
        Ok(())
    }
    pub(super) fn capture_lsp(&mut self) {
        if self.state.closing {
            return;
        }
        let Some(runtime) = &mut self.lsp else {
            return;
        };
        let Some(editor) = &runtime.editor else {
            return;
        };
        let mut snapshots = vec![];
        let mut attachments = std::collections::HashSet::new();
        for view in self.state.editor.iter().chain(
            self.state
                .search
                .iter()
                .filter_map(|dialog| dialog.editor.as_ref()),
        ) {
            if let Ok(document) = self.state.documents.document(view.document_id()) {
                let snapshot = document.snapshot();
                if let (DocumentOrigin::File(_), Some(language)) =
                    (&snapshot.origin, &snapshot.language)
                {
                    let attachment = (view.owner(), snapshot.id, language.clone());
                    if !runtime.attachments.contains(&attachment)
                        && !runtime.automatic.contains(language)
                    {
                        runtime.automatic.push(language.clone());
                        runtime.documents_dirty = true;
                    }
                    attachments.insert(attachment);
                    snapshots.push(snapshot);
                }
            }
        }
        runtime.attachments = attachments;
        let mut ids: Vec<_> = self.state.documents.ids().collect();
        ids.sort_unstable();
        snapshots.extend(ids.into_iter().filter_map(|id| {
            self.state
                .documents
                .document(id)
                .ok()
                .map(|document| document.snapshot())
        }));
        if editor.replace_sources(snapshots) {
            runtime.documents_dirty = true;
        }
    }
    pub(super) fn pump_lsp(&mut self) -> Result<()> {
        if self.state.closing {
            return Ok(());
        }
        self.capture_lsp();
        if let Some(runtime) = &mut self.lsp
            && let Some(service) = &runtime.service
            && runtime.revision != service.revision()
        {
            runtime.revision = service.revision();
            self.notifications.repaint();
            runtime.documents_dirty = true;
            for status in &mut self.state.lsp_settings.statuses {
                status.running = service.running(&status.entry.id);
                if let Some(error) = service.last_error(&status.entry.id) {
                    status.message = Some(error.clone());
                    runtime
                        .reported_errors
                        .insert(status.entry.id.clone(), error);
                } else if let Some(previous) = runtime.reported_errors.remove(&status.entry.id)
                    && status.message.as_ref() == Some(&previous)
                {
                    status.message = None;
                }
            }
        }
        let Some(runtime) = &mut self.lsp else {
            return Ok(());
        };
        if runtime.documents_busy
            || !runtime.documents_dirty
            || self.state.lsp_settings.busy.is_some()
        {
            return Ok(());
        }
        let (Some(editor), Some(service), Some(lifecycle)) = (
            runtime.editor.clone(),
            runtime.service.clone(),
            runtime.lifecycle.clone(),
        ) else {
            return Ok(());
        };
        runtime.documents_busy = true;
        runtime.documents_dirty = false;
        let automatic = std::mem::take(&mut runtime.automatic);
        let config = runtime.config.clone();
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|error| DomainError::Unsupported(error.to_string()))?;
        let tag = self.state.tag()?;
        self.blocking(
            tag,
            self.root_owner,
            "lsp-documents",
            false,
            Context::LspDocuments,
            Box::new(move |cancel| {
                if !automatic.is_empty() {
                    let (catalog, _) = Catalog::load(&config.paths, config.catalog.as_deref())?;
                    for language in automatic {
                        if cancel.is_cancelled() {
                            return Err(DomainError::Cancelled);
                        }
                        if service.for_language(&language).is_some() {
                            continue;
                        }
                        if let Some(entry) = catalog.servers.iter().find(|entry| {
                            entry.languages.iter().any(|candidate| {
                                kode_lsp::documents::language_id(candidate)
                                    == kode_lsp::documents::language_id(&language)
                            })
                        }) && let Err(error) =
                            lifecycle.apply(&entry.id, LifecycleAction::Start, &cancel)
                        {
                            service.report_start_error(&entry.id, &error);
                        }
                    }
                }
                handle.block_on(editor.synchronize(&cancel))?;
                Ok(Output::LspDocuments)
            }),
        )
    }
    pub(super) fn lsp_work(&mut self, action: SettingsAction) -> Result<()> {
        let Some(job) = self
            .state
            .lsp_settings
            .begin(self.state.generation, action)?
        else {
            return Ok(());
        };
        let Some(runtime) = &self.lsp else {
            self.state.lsp_settings.complete(
                job.tag,
                Err(DomainError::Unsupported(
                    "LSP settings are not configured".into(),
                )),
            );
            return Ok(());
        };
        let config = runtime.config.clone();
        let fetcher = runtime.fetcher.clone();
        let lifecycle = runtime.lifecycle.clone();
        let service = runtime.service.clone();
        let tag = job.tag;
        let owner = OwnerId(self.state.lsp_settings.id.0);
        let notifications = self.notifications.clone();
        let critical = !matches!(job.action, SettingsAction::Refresh);
        self.blocking(
            tag,
            owner,
            "lsp-settings",
            critical,
            Context::LspSettings,
            Box::new(move |scheduler_cancel| {
                if scheduler_cancel.is_cancelled() || job.cancel.is_cancelled() {
                    return Err(DomainError::Cancelled);
                }
                let (catalog, catalog_path) =
                    Catalog::load(&config.paths, config.catalog.as_deref())?;
                let selected = |id: &str| {
                    catalog.entry(id).cloned().ok_or_else(|| {
                        DomainError::Unsupported(format!("unknown language server: {id}"))
                    })
                };
                let lifecycle_action = |id: &str, action: LifecycleAction| {
                    lifecycle
                        .as_ref()
                        .ok_or_else(|| {
                            DomainError::Unsupported(
                                "Language-server process service is unavailable".into(),
                            )
                        })?
                        .apply(id, action, &job.cancel)
                };
                let outcome: Result<String> = (|| {
                    Ok(match job.action {
                        SettingsAction::Refresh => "Catalog refreshed".into(),
                        SettingsAction::Install(ref id) => {
                            if lifecycle
                                .as_ref()
                                .is_some_and(|lifecycle| lifecycle.running(id))
                            {
                                lifecycle_action(id, LifecycleAction::Stop)?;
                            }
                            let receipt = InstallJob {
                                entry: selected(id)?,
                                options: config.discovery.clone(),
                            }
                            .run(
                                fetcher.as_ref(),
                                &job.cancel,
                                |progress| {
                                    notifications.progress(
                                        owner,
                                        tag,
                                        format!("{}: {} bytes", progress.phase, progress.completed),
                                        progress.completed,
                                        progress.total,
                                    )
                                },
                            )?;
                            format!(
                                "Installed {} {}{}",
                                receipt.manifest.id,
                                receipt.manifest.version,
                                receipt.previous.map_or(String::new(), |path| format!(
                                    "; previous files: {}",
                                    path.display()
                                ))
                            )
                        }
                        SettingsAction::Uninstall(ref id) => {
                            selected(id)?;
                            if lifecycle
                                .as_ref()
                                .is_some_and(|lifecycle| lifecycle.running(id))
                            {
                                lifecycle_action(id, LifecycleAction::Stop)?;
                            }
                            if uninstall(&config.discovery, id, &job.cancel)? {
                                format!("Removed {id}")
                            } else {
                                format!("{id}: nothing installed in this root")
                            }
                        }
                        SettingsAction::Lifecycle(ref id, action) => {
                            selected(id)?;
                            lifecycle_action(id, action)?
                        }
                    })
                })();
                let message = outcome.unwrap_or_else(|error| error.to_string());
                let mut statuses = config.discovery.statuses(&catalog, |id| {
                    lifecycle
                        .as_ref()
                        .is_some_and(|lifecycle| lifecycle.running(id))
                });
                if let Some(service) = &service {
                    for status in &mut statuses {
                        if let Some(error) = service.last_error(&status.entry.id) {
                            status.message = Some(error);
                        }
                    }
                }
                Ok(Output::LspSettings(Box::new(SettingsReport {
                    statuses,
                    catalog_path,
                    install_root: config.discovery.install_root,
                    message,
                })))
            }),
        )
    }
}
