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
        });
        self.state.lsp_settings.request_refresh();
        Ok(())
    }
    pub fn set_lsp_lifecycle(&mut self, lifecycle: Arc<dyn ServerLifecycle>) -> Result<()> {
        self.lsp
            .as_mut()
            .ok_or_else(|| DomainError::Unsupported("LSP settings are not configured".into()))?
            .lifecycle = Some(lifecycle);
        self.state.lsp_settings.request_refresh();
        Ok(())
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
                let statuses = config.discovery.statuses(&catalog, |id| {
                    lifecycle
                        .as_ref()
                        .is_some_and(|lifecycle| lifecycle.running(id))
                });
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
