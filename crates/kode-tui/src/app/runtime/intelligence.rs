use super::*;
use kode_intelligence::{
    facade::IntelligenceResources,
    ids::SemanticIds,
    service::{CodeIntelService, IndexPhase},
};

pub(super) struct AppPreviewSourceReader {
    pub native: Option<Arc<kode_intelligence::facade::NativeFacade>>,
    pub disk: NativePreviewSourceReader,
}
pub(super) struct AppPreviewHighlighter {
    pub provider: Arc<dyn kode_core::intelligence::IntelligenceProvider>,
}
impl definition::PreviewHighlighter for AppPreviewHighlighter {
    fn highlights(
        &self,
        snapshot: &kode_core::document::TextSnapshot,
    ) -> Result<Vec<kode_core::intelligence::HighlightSpan>> {
        self.provider.highlights(&snapshot.to_service_snapshot())
    }
}
impl definition::PreviewSourceReader for AppPreviewSourceReader {
    fn read(
        &self,
        target: &kode_core::intelligence::NavigationTarget,
        tag: JobTag,
    ) -> Result<kode_core::document::TextSnapshot> {
        if let Some(native) = &self.native
            && let Some(path) = target.path.to_str()
            && let Some(snapshot) = native.captured_source(path)?
        {
            return Ok(snapshot);
        }
        definition::PreviewSourceReader::read(&self.disk, target, tag)
    }
}

impl AppRuntime {
    pub async fn start_intelligence(
        &mut self,
        resources: Arc<IntelligenceResources>,
    ) -> Result<()> {
        if self.intelligence.is_some() {
            return Ok(());
        }
        let mut service = CodeIntelService::open(
            &self.state.project,
            resources.clone(),
            self.detector.clone(),
        )
        .await?;
        service.rescan(self.state.project.clone(), false)?;
        self.overlay_worker = Some(Arc::new(crate::editor::overlays::OverlayWorker::new(
            resources.clone(),
        )));
        self.intelligence_resources = Some(resources);
        self.native_watch_revision = self
            .state
            .watcher
            .as_ref()
            .map_or(0, |watcher| watcher.source_revision());
        self.native_watch_due = None;
        self.native_last_poll = self.state.clock.now_millis();
        self.native_auto_scan = true;
        self.native_scan_cancel = None;
        self.intelligence = Some(service);
        Ok(())
    }
    pub fn intelligence(&self) -> Option<&CodeIntelService> {
        self.intelligence.as_ref()
    }
    pub fn rebuild_index(&mut self) -> Result<()> {
        let service = self
            .intelligence
            .as_mut()
            .ok_or_else(|| DomainError::Unsupported("native index is not running".into()))?;
        service.rescan(self.state.project.clone(), true)?;
        self.native_auto_scan = true;
        Ok(())
    }
    pub fn clear_index(&mut self) -> Result<()> {
        let service = self
            .intelligence
            .as_mut()
            .ok_or_else(|| DomainError::Unsupported("native index is not running".into()))?;
        service.clear(self.state.project.clone())?;
        self.native_auto_scan = false;
        Ok(())
    }
    pub(super) fn stamp_query(&mut self, tag: JobTag) -> Result<()> {
        if let Some(service) = &self.intelligence {
            self.query_versions.insert(tag, service.query_revision()?);
        }
        Ok(())
    }
    pub(super) fn pump_intelligence(&mut self) -> Result<()> {
        let Some(service) = &mut self.intelligence else {
            return Ok(());
        };
        let now = self.state.clock.now_millis();
        let provider = service.facade();
        let mut captures = vec![];
        if let Some(view) = &mut self.state.editor {
            view.host.intelligence = Some(provider.clone());
            view.overlays.enable();
            captures.push(
                self.state
                    .documents
                    .document(view.document_id())?
                    .snapshot(),
            );
        }
        if let Some(dialog) = &mut self.state.search
            && let Some(view) = &mut dialog.editor
        {
            view.host.intelligence = Some(provider);
            view.overlays.enable();
            captures.push(
                self.state
                    .documents
                    .document(view.document_id())?
                    .snapshot(),
            );
        }
        if !self.state.closing {
            if self
                .native_scan_cancel
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                self.native_scan_cancel = None;
                if let Some(tag) = service.cancel_rescan()? {
                    self.jobs.cancel_request(tag);
                    let queued = self.pending.iter().any(|work| work.tag == tag);
                    self.pending.retain(|work| work.tag != tag);
                    if queued {
                        service.failed(tag, DomainError::Cancelled, now)?;
                    }
                }
            }
            for capture in captures {
                service.observe(capture, now)?;
            }
            let revision = self
                .state
                .watcher
                .as_ref()
                .map_or(self.native_watch_revision, |watcher| {
                    watcher.source_revision()
                });
            if revision != self.native_watch_revision {
                self.native_watch_revision = revision;
                self.native_watch_due = Some(now.saturating_add(150));
            }
            let changed = self.native_watch_due.is_some_and(|due| now >= due);
            let fallback = self.native_auto_scan
                && now.saturating_sub(self.native_last_poll) >= 5_000
                && matches!(
                    service.status()?.phase,
                    IndexPhase::Ready | IndexPhase::Error
                );
            if self.native_auto_scan && changed || fallback {
                service.rescan(self.state.project.clone(), false)?;
                self.native_watch_due = None;
                self.native_last_poll = now;
            }
        }
        for change in service.take_changes() {
            self.state.intelligence_revision = change.revision;
            for view in self.state.editor.iter_mut().chain(
                self.state
                    .search
                    .iter_mut()
                    .filter_map(|dialog| dialog.editor.as_mut()),
            ) {
                let snapshot = self
                    .state
                    .documents
                    .document(view.document_id())?
                    .snapshot();
                let path = kode_intelligence::facade::source_path(&snapshot.origin)?;
                if change.affected_files.contains(&SemanticIds::file(&path)) {
                    view.intelligence_changed();
                }
            }
        }
        let status = service.status()?;
        if let Some(dialog) = &mut self.state.rescan
            && !dialog.done
        {
            if self.state.scan_token.is_none()
                && (!service.has_pending_work() || status.phase == IndexPhase::Error)
            {
                let result = if status.phase == IndexPhase::Cancelled {
                    Err(DomainError::Cancelled)
                } else if status.phase == IndexPhase::Error {
                    Err(DomainError::Unsupported(
                        status
                            .notes
                            .last()
                            .cloned()
                            .unwrap_or_else(|| "native indexing failed".into()),
                    ))
                } else if let Some(error) = &self.state.scan.error {
                    Err(DomainError::Unsupported(error.to_string()))
                } else {
                    Ok(())
                };
                dialog.complete(dialog.tag, result);
            } else if self.state.scan_token.is_none() {
                dialog.status = format!("Index {:?}: {} processed", status.phase, status.completed);
            }
        }
        let detail = status
            .notes
            .first()
            .map_or(String::new(), |note| format!(" | {note}"));
        self.state.intelligence_status = format!(
            "index {:?} r{}: {} files / {} symbols | {} / {} processed{detail}",
            status.phase,
            status.revision,
            status.files,
            status.symbols,
            status.completed,
            status
                .total
                .map_or_else(|| "?".into(), |total| total.to_string())
        );
        let job = if self.state.closing {
            None
        } else {
            service.next_job(now)?
        };
        let current: std::collections::HashSet<_> = self
            .contexts
            .keys()
            .copied()
            .chain(self.pending.iter().map(|pending| pending.tag))
            .collect();
        self.query_versions.retain(|tag, _| current.contains(tag));
        if let Some(job) = job {
            self.blocking(
                job.tag,
                self.root_owner,
                "native-index",
                false,
                Context::IndexPrepared,
                Box::new(move |cancel| {
                    job.run(cancel)
                        .map(|prepared| Output::IndexPrepared(Box::new(prepared)))
                }),
            )?;
        }
        Ok(())
    }
    pub(super) fn index_prepared(&mut self, tag: JobTag, result: Result<Output>) -> Result<()> {
        let Some(service) = &mut self.intelligence else {
            return Ok(());
        };
        let result = result.and_then(|output| match output {
            Output::IndexPrepared(prepared) => Ok(*prepared),
            _ => Err(DomainError::Unsupported(
                "wrong native preparation result".into(),
            )),
        });
        let Some(commit) = service.prepared(tag, result, self.state.clock.now_millis())? else {
            return Ok(());
        };
        self.contexts.insert(tag, Context::IndexCommitted);
        if let Err(error) = self.jobs.submit(
            self.root_owner,
            tag,
            Delivery::Critical,
            move |_| async move {
                commit
                    .run()
                    .await
                    .map(|result| Output::IndexCommitted(Box::new(result)))
            },
        ) {
            self.complete(Completion {
                tag,
                owner: self.root_owner,
                result: Err(error),
            })?;
        }
        Ok(())
    }
    pub(super) fn index_committed(&mut self, tag: JobTag, result: Result<Output>) -> Result<()> {
        if let Some(service) = &mut self.intelligence {
            let result = result.and_then(|output| match output {
                Output::IndexCommitted(result) => Ok(*result),
                _ => Err(DomainError::Unsupported(
                    "wrong native persistence result".into(),
                )),
            });
            service.committed(tag, result, self.state.clock.now_millis())?;
        }
        Ok(())
    }
    pub(super) fn index_file_change(&mut self, change: &FileChange) -> Result<()> {
        if let Some(service) = &mut self.intelligence {
            match change {
                FileChange::Renamed { from, .. } | FileChange::Deleted(from) => {
                    if let Some(path) = from.to_str() {
                        service.remove_path(path)?;
                    }
                }
                _ => {}
            }
            service.rescan(self.state.project.clone(), false)?;
            self.native_auto_scan = true;
        }
        Ok(())
    }
}
