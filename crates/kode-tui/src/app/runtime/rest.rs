use super::*;
use crate::panels::rest::state::{self, Reply, Work};
use kode_platform::jobs::CancellationToken;
impl AppRuntime {
    pub(super) fn rest_work(&mut self, work: Work) -> Result<()> {
        let tag = work.tag();
        let context = work.context();
        let (owner, operation, critical) = match &work {
            Work::Editor(job) => match job.as_ref() {
                crate::panels::rest::editor::jobs::Job::Format { snapshot, .. } => (
                    self.state.documents.save_owner(snapshot.id)?,
                    "rest-format",
                    false,
                ),
                crate::panels::rest::editor::jobs::Job::Change { snapshot, .. } => (
                    self.state.documents.save_owner(snapshot.id)?,
                    "rest-buffer-change",
                    false,
                ),
                crate::panels::rest::editor::jobs::Job::Response { proof, .. } => {
                    let Some(form) = self
                        .state
                        .rest
                        .form
                        .as_ref()
                        .filter(|form| form.key == proof.key)
                    else {
                        return Ok(());
                    };
                    (OwnerId(form.id.0), "rest-response", false)
                }
                crate::panels::rest::editor::jobs::Job::Derived { key, .. } => {
                    let Some(form) = self
                        .state
                        .rest
                        .form
                        .as_ref()
                        .filter(|form| form.key == *key)
                    else {
                        return Ok(());
                    };
                    (OwnerId(form.id.0), "rest-derived", false)
                }
                _ => (self.root_owner, "rest-open", false),
            },
            Work::Run(_) => (self.root_owner, "rest-prepare-run", false),
            Work::Open { .. } => (self.root_owner, "rest-open", false),
            Work::Environment { snapshot, .. } => (
                self.state.documents.save_owner(snapshot.id)?,
                "rest-environment",
                false,
            ),
            Work::Import { .. } => (self.root_owner, "rest-import", false),
            Work::Export { .. } => (self.root_owner, "rest-export", true),
        };
        self.blocking(
            tag,
            owner,
            operation,
            critical,
            Context::Rest(Box::new(context)),
            Box::new(move |cancel| {
                let accepted = CancellationToken::new();
                let token = if critical { &accepted } else { &cancel };
                work.prepare(token)
                    .map(|(_, reply)| Output::Rest(Box::new(reply)))
            }),
        )
    }
    pub(super) fn rest_complete(
        &mut self,
        tag: JobTag,
        context: state::Context,
        result: Result<Output>,
    ) -> Result<()> {
        let result = result.and_then(|output| {
            if let Output::Rest(reply) = output {
                Ok(*reply)
            } else {
                Err(DomainError::Unsupported("wrong REST worker result".into()))
            }
        });
        match context {
            state::Context::Editor(context) => {
                let reply = result.and_then(|reply| {
                    if let Reply::Editor(reply) = reply {
                        Ok(*reply)
                    } else {
                        Err(DomainError::Unsupported(
                            "wrong request-editor result".into(),
                        ))
                    }
                });
                self.state.complete_rest_editor(tag, context, reply)?;
            }
            state::Context::Run { structure } => {
                if self.state.rest.run.pending != Some(tag) {
                    return Ok(());
                }
                self.state.rest.run.pending = None;
                if self.state.closing {
                    return Ok(());
                }
                if structure != self.state.rest.run.structure {
                    self.state.rest.sidebar.message =
                        "REST collection changed while preparing; run the selection again".into();
                    return Ok(());
                }
                match result {
                    Ok(Reply::Run(prepared)) => self.start_rest_run(tag, *prepared)?,
                    Err(error) => {
                        self.state.rest.sidebar.message = error.to_string();
                        self.state.notice = error.to_string();
                        if let Some(form) = &mut self.state.rest.form {
                            form.message = error.to_string();
                        }
                    }
                    _ => {}
                }
            }
            state::Context::Open {
                descriptor,
                kind,
                expected,
            } => {
                if self.state.rest.pending_open != Some(tag) {
                    return Ok(());
                }
                match result {
                    Ok(Reply::Opened(document)) => self
                        .state
                        .install_rest_prepared(tag, descriptor, kind, expected, document)?,
                    Err(error) => {
                        self.state.rest.pending_open = None;
                        if !matches!(error, DomainError::Cancelled) {
                            self.state.notice = error.to_string();
                        }
                    }
                    _ => {}
                }
            }
            state::Context::Environment {
                document,
                environment,
            } => {
                if self.state.rest.pending_environments.get(&document) != Some(&tag) {
                    return Ok(());
                }
                self.state.rest.pending_environments.remove(&document);
                if self.state.closing
                    || !tag.document.is_some_and(|(id, revision)| {
                        self.state
                            .documents
                            .document(id)
                            .is_ok_and(|document| document.revision() == revision)
                    })
                {
                    return Ok(());
                }
                match result {
                    Ok(Reply::Environment(values)) => {
                        if self
                            .state
                            .rest
                            .model
                            .set_environment_values(&environment, values)
                        {
                            self.state.changed()?;
                        }
                    }
                    Err(error) if !matches!(error, DomainError::Cancelled) => {
                        self.state.notice = error.to_string()
                    }
                    _ => {}
                }
            }
            state::Context::Import {
                nonce,
                parent,
                expected,
            } => {
                if self.state.closing {
                    return Ok(());
                }
                match result {
                    Ok(Reply::Imported(imported)) => {
                        match self.state.rest.sidebar.import_finished(
                            nonce,
                            &expected,
                            imported,
                            &parent,
                            &mut self.state.rest.model,
                        ) {
                            Ok(true) => self.state.changed()?,
                            Ok(false) => {}
                            Err(error) => self.state.notice = error.to_string(),
                        }
                    }
                    Err(error) => self
                        .state
                        .rest
                        .sidebar
                        .file_finished(nonce, error.to_string()),
                    _ => {}
                }
            }
            state::Context::Export { nonce, path } => match result {
                Ok(Reply::Exported(identity)) => {
                    self.state.rest.exports.insert(path.clone(), identity);
                    self.state
                        .rest
                        .sidebar
                        .file_finished(nonce, format!("Exported collection to {}", path.display()));
                }
                Err(error) => self
                    .state
                    .rest
                    .sidebar
                    .file_finished(nonce, error.to_string()),
                _ => {}
            },
        }
        Ok(())
    }
    fn start_rest_run(
        &mut self,
        tag: JobTag,
        mut prepared: crate::panels::rest::run::Prepared,
    ) -> Result<()> {
        use crate::panels::rest::run::Running;
        // A view may have acquired its item key while preparation was running.
        // Reconcile by unchanged structural paths before handing off the frozen run.
        let mut keys = (*prepared.keys).clone();
        for (path, key) in self.state.rest.run.keys.iter() {
            keys.insert(path.clone(), key.clone());
        }
        for item in &mut prepared.definition.items {
            if let Some(key) = keys.get(&item.path) {
                item.id = key.clone();
            }
        }
        prepared.keys = Arc::new(keys);
        let total = prepared.definition.items.len();
        if total == 0 {
            self.state.rest.sidebar.message = "No requests in this selection".into();
            return Ok(());
        }
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let (events, receiver) = kode_rest::stream::event_channel();
        let (responses, response_receiver) = mpsc::channel(1);
        let client = self.state.rest.run.client.clone();
        let submitted = self.jobs.submit(self.root_owner,tag,Delivery::Cancellable,move |scheduler_cancel|async move {
            let _cancel_on_drop = token.clone().drop_guard();
            let future = kode_rest::runner::execute(prepared.definition,client,token.clone(),Some(events),responses);
            tokio::pin!(future);
            tokio::select! {biased;
                _=scheduler_cancel.cancelled()=>{token.cancel();future.await.map(Output::RestRun)},
                result=&mut future=>result.map(Output::RestRun),
            }
        });
        if let Err(error) = submitted {
            self.state.rest.sidebar.message = error.to_string();
            return Ok(());
        }
        self.contexts.insert(tag, Context::RestRun);
        self.state.rest.run.keys = prepared.keys;
        self.state.rest.run.summary = None;
        self.state.rest.run.running = Some(Running {
            tag,
            cancel,
            events: receiver,
            responses: response_receiver,
            completed: 0,
            total,
            current: None,
            received_bytes: 0,
        });
        self.state.rest.sidebar.message = format!("Running 0/{total} — Run again to stop");
        Ok(())
    }
    pub(super) fn pump_rest(&mut self) {
        if self.state.closing {
            self.state.rest.run.pending = None;
            self.state.rest.run.cancel();
        }
        if self.state.rest.run.pump() {
            if let Some(running) = &self.state.rest.run.running {
                self.state.rest.sidebar.message = format!(
                    "Running {}/{} · {} bytes — Run again to stop",
                    running.completed, running.total, running.received_bytes
                );
            }
            self.notifications.repaint();
        }
    }
    pub(super) fn rest_run_complete(&mut self, tag: JobTag, result: Result<Output>) -> Result<()> {
        if self
            .state
            .rest
            .run
            .running
            .as_ref()
            .is_none_or(|run| run.tag != tag)
        {
            return Ok(());
        }
        self.pump_rest();
        self.state.rest.run.running = None;
        match result {
            Ok(Output::RestRun(summary)) => {
                let failed = summary
                    .outcomes
                    .iter()
                    .filter(|outcome| outcome.error.is_some() || outcome.validation_error.is_some())
                    .count();
                self.state.rest.sidebar.message = format!(
                    "{}: {} completed, {failed} failed",
                    if summary.cancelled {
                        "REST run stopped"
                    } else {
                        "REST run finished"
                    },
                    summary.outcomes.len()
                );
                if let Some(form) = &mut self.state.rest.form
                    && let Some(outcome) = summary
                        .outcomes
                        .iter()
                        .find(|outcome| outcome.item.id == form.key)
                    && let Some(error) = outcome
                        .validation_error
                        .clone()
                        .or_else(|| outcome.error.map(|error| error.message().into()))
                {
                    form.message = error;
                }
                self.state.rest.run.summary = Some(summary);
            }
            Err(error) => self.state.rest.sidebar.message = error.to_string(),
            _ => {}
        }
        self.notifications.repaint();
        Ok(())
    }
}
