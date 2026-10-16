//! SQL jobs survive editor focus changes, but are scoped to their workspace and
//! console request. No completion can submit or replay SQL.
use super::{AppRuntime, Context, Output};
use crate::{
    editor::host::{ExecutionAction, HostExecution},
    panels::database::{console::Running, results::ResultsPanel},
};
use kode_core::{DomainError, JobTag, Result};
use kode_database::{
    bridge::client::{BridgeError, Failure},
    execution::{Options, Phase, Snapshot},
    statement::{self, Dialect},
};
use kode_platform::jobs::{CancellationToken, Delivery};
use tokio::sync::watch;

pub struct QueryReport {
    pub sql: String,
    pub result: Snapshot,
}
pub struct QueryContext {
    pub console: String,
    pub started_at_ms: u64,
}
impl AppRuntime {
    pub(super) fn sql_control(
        &mut self,
        tag: JobTag,
        source: String,
        action: crate::panels::database::console::Control,
    ) -> Result<()> {
        use crate::panels::database::console::Control;
        let cancel = self
            .state
            .sql
            .controls
            .get(&source)
            .filter(|(pending, _)| *pending == tag)
            .map(|(_, cancel)| cancel.clone())
            .ok_or(DomainError::Cancelled)?;
        let Some(database) = self.database.clone() else {
            return self.sql_control_complete(
                tag,
                source,
                Err(DomainError::Unsupported(
                    "Database services are unavailable".into(),
                )),
            );
        };
        let target = source.clone();
        let submitted=self.jobs.submit(self.root_owner,tag,Delivery::Critical,move|scheduler_cancel|async move {
            let cancellation=cancel.clone();
            let future=async move {
                let result=match action {
                    Control::AutoCommit=>{let current=database.connections.session(&target).map_err(super::database::connection_error)?;database.connections.set_context(&target,kode_database::connections::Context{auto_commit:Some(!current.auto_commit),..Default::default()},&cancel).await},
                    Control::Commit=>database.connections.commit(&target,&cancel).await,
                    Control::Rollback=>database.connections.rollback(&target,&cancel).await,
                }.map_err(super::database::connection_error)?;
                database.metadata.refresh_all(&target);
                Ok(Output::SqlControl(result))
            };
            tokio::pin!(future);
            tokio::select!{biased;_=scheduler_cancel.cancelled()=>{cancellation.cancel();future.await},result=&mut future=>result}
        });
        match submitted {
            Ok(()) => {
                self.contexts.insert(tag, Context::SqlControl(source));
            }
            Err(error) => self.sql_control_complete(tag, source, Err(error))?,
        }
        Ok(())
    }
    pub(super) fn sql_control_complete(
        &mut self,
        tag: JobTag,
        source: String,
        result: Result<kode_database::connections::SessionInfo>,
    ) -> Result<()> {
        if self
            .state
            .sql
            .controls
            .get(&source)
            .is_none_or(|(pending, _)| *pending != tag)
        {
            return Ok(());
        }
        self.state.sql.controls.remove(&source);
        match result {
            Ok(session) => {
                self.state.sql.diagnostics.remove(&source);
                self.state
                    .sql
                    .manager
                    .set_auto_commit(&source, session.auto_commit);
                self.state.changed()?;
                self.state.notice =
                    "Transaction operation completed for all consoles using this datasource".into();
            }
            Err(error) => {
                let text = format!(
                    "Transaction operation / request {}\n{}",
                    tag.request.0, error
                );
                self.state.sql.diagnostics.insert(source, text);
                self.state.notice = format!(
                    "{} [F4 Details]",
                    crate::widgets::diagnostic::summary(&error.to_string())
                );
            }
        }
        Ok(())
    }
    pub(super) fn sql_save(
        &mut self,
        job: crate::editor::host::OwnerSaveJob,
        path: std::path::PathBuf,
        expected: Option<kode_core::services::DiskIdentity>,
    ) -> Result<()> {
        use kode_core::{DocumentOrigin, services::DocumentIo};
        let console = self
            .state
            .sql
            .documents
            .get(&job.snapshot.id)
            .cloned()
            .ok_or(DomainError::NoSaveTarget)?;
        let snapshot = job.snapshot.clone();
        let target = path.clone();
        self.blocking(
            job.tag,
            self.root_owner,
            "sql-save",
            true,
            Context::SqlSave { console, job, path },
            Box::new(move |cancel| {
                if cancel.is_cancelled() {
                    return Err(DomainError::Cancelled);
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|source| DomainError::Io {
                        operation: "create SQL session directory",
                        path: parent.into(),
                        source,
                    })?;
                }
                let mut snapshot = snapshot.to_service_snapshot();
                snapshot.origin = DocumentOrigin::File(target.clone());
                kode_platform::files::NativeDocumentIo
                    .save(&target, &snapshot, expected.as_ref())
                    .map(Output::Saved)
            }),
        )
    }
    pub(super) fn sql_save_complete(
        &mut self,
        console: String,
        job: crate::editor::host::OwnerSaveJob,
        path: std::path::PathBuf,
        result: Result<kode_core::services::DiskIdentity>,
    ) -> Result<()> {
        match result {
            Ok(identity) => {
                self.state.sql.saved.insert(console, (path, identity));
                if let Some(dialog) = self.state.sql.dialog.take() {
                    self.state.focus.focused = dialog.previous_focus;
                }
                if let Some(work) =
                    self.state
                        .documents
                        .complete_owner_save(job.snapshot.id, job.tag, Ok(()))?
                {
                    self.accept(super::super::state::AppWork::Editor {
                        owner: self.root_owner,
                        work,
                    })?;
                }
                self.state.notice = "SQL session saved".into();
                self.state.changed()?;
            }
            Err(error) => {
                self.state.notice = error.to_string();
                if let Some(dialog) = &mut self.state.sql.dialog {
                    dialog.busy = false;
                    dialog.dialog.finished = false;
                }
                if self.state.closing {
                    self.state.dismiss_sql_dialog()?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn sql_execute(&mut self, execution: HostExecution) -> Result<()> {
        let Some(console) = self
            .state
            .sql
            .documents
            .get(&execution.snapshot.id)
            .cloned()
        else {
            return Err(DomainError::Unsupported(
                "No SQL console owns this execution".into(),
            ));
        };
        if self.state.closing {
            return Err(DomainError::Cancelled);
        }
        let database = self
            .database
            .clone()
            .ok_or_else(|| DomainError::Unsupported("Database services are unavailable".into()))?;
        let session = self
            .state
            .sql
            .manager
            .get(&console)
            .ok_or_else(|| DomainError::MissingResource("SQL session".into()))?
            .state
            .clone();
        let definition = self
            .state
            .database
            .tree
            .sources
            .iter()
            .find(|source| source.id == session.data_source_id)
            .ok_or_else(|| {
                DomainError::MissingResource(
                    "SQL datasource was removed; the buffer is retained".into(),
                )
            })?;
        // This API never opens a connection as a side effect of execution.
        database
            .connections
            .session(&session.data_source_id)
            .map_err(super::database::connection_error)?;
        if execution.snapshot.len_bytes() > 4 * 1024 * 1024 {
            return Err(DomainError::Unsupported(
                "SQL console exceeds the 4 MiB execution limit".into(),
            ));
        }
        let options = Options {
            catalog: Some(
                session
                    .catalog
                    .clone()
                    .or_else(|| definition.default_catalog.clone()),
            ),
            schema: Some(
                session
                    .schema
                    .clone()
                    .or_else(|| definition.default_schema.clone()),
            ),
            ..Default::default()
        };
        let dialect = if matches!(definition.driver.id.as_str(), "mysql" | "mariadb") {
            Dialect::MySql
        } else {
            Dialect::Standard
        };
        let tag = self.state.tag()?;
        self.state.sql.manager.begin(&console, tag.request)?;
        let panel = self
            .state
            .sql
            .results
            .entry(console.clone())
            .or_insert(ResultsPanel::new()?);
        panel.reset();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (progress, receiver) = watch::channel(Snapshot::default());
        self.state.sql.running.insert(
            console.clone(),
            Running {
                tag,
                cancel,
                progress: receiver,
                cancel_retry: None,
            },
        );
        let source = session.data_source_id.clone();
        let submitted=self.jobs.submit(self.root_owner,tag,Delivery::Critical,move|scheduler_cancel|async move{
            let cancellation=worker_cancel.clone();
            let future=async move {
                let targeted=tokio::task::spawn_blocking(move|| {
                    let text=execution.snapshot.text();
                    let range=execution.selection.range();
                    let found=if execution.action==ExecutionAction::SqlScript { statement::script(&text,dialect) } else { statement::locate(&text,execution.selection.head.0,(!range.is_empty()).then_some(range.start.0..range.end.0),dialect)? };
                    found.map(|found|found.sql.to_string()).ok_or_else(||DomainError::Unsupported("No executable SQL statement at the cursor".into()))
                }).await.map_err(|_|DomainError::Unsupported("SQL targeting worker failed".into()))??;
                let result=database.executor.execute(&source,targeted.clone(),options,progress,&worker_cancel).await;
                // Refresh generations prevent metadata requested during SQL from
                // repopulating the tree after a schema-changing execution.
                database.metadata.refresh_all(&source);
                Ok(Output::Sql(QueryReport{sql:targeted,result:result.unwrap_or_else(failed)}))
            };
            tokio::pin!(future);
            tokio::select!{biased;_=scheduler_cancel.cancelled()=>{cancellation.cancel();future.await},result=&mut future=>result}
        });
        match submitted {
            Ok(()) => {
                self.contexts.insert(
                    tag,
                    Context::Sql(QueryContext {
                        console,
                        started_at_ms: super::super::workspace::epoch_millis().max(0) as u64,
                    }),
                );
            }
            Err(error) => self.sql_complete(
                tag,
                QueryContext {
                    console,
                    started_at_ms: 0,
                },
                Err(error),
            )?,
        }
        Ok(())
    }
    pub(super) fn sql_complete(
        &mut self,
        tag: JobTag,
        context: QueryContext,
        result: Result<QueryReport>,
    ) -> Result<()> {
        let Some(running) = self
            .state
            .sql
            .running
            .get(&context.console)
            .filter(|running| running.tag == tag)
        else {
            return Ok(());
        };
        let last = running.progress.borrow().clone();
        if let Err(error) = &result {
            self.state.notice = error.to_string();
        }
        let report = result.unwrap_or_else(|error| QueryReport {
            sql: String::new(),
            result: Snapshot {
                phase: Phase::Finished,
                error: Some(BridgeError::new(
                    Failure::Protocol,
                    match error {
                        DomainError::Cancelled => "SQL execution cancelled",
                        _ => "SQL target could not be executed",
                    },
                )),
                ..last
            },
        });
        self.state.sql.running.remove(&context.console);
        if report.result.unknown_outcome {
            self.state.notice="SQL outcome unknown. Check the database before retrying. Alt+4, R restarts the JDBC service; c reconnects.".into();
        }
        if self.state.sql.manager.finish(
            &context.console,
            tag.request,
            report.sql,
            context.started_at_ms,
            report.result.clone(),
        ) {
            if let Some(panel) = self.state.sql.results.get_mut(&context.console) {
                panel.publish(report.result);
            }
            self.state.changed()?;
        }
        Ok(())
    }
    pub(super) fn pump_sql(&mut self) {
        if self.state.closing {
            for (_, cancel) in self.state.sql.controls.values() {
                cancel.cancel();
            }
        }
        let updates: Vec<_> = self
            .state
            .sql
            .running
            .iter_mut()
            .filter_map(|(id, running)| {
                if self.state.closing {
                    running.cancel.cancel();
                }
                running.progress.has_changed().unwrap_or(false).then(|| {
                    (
                        id.clone(),
                        running.tag.request,
                        running.progress.borrow_and_update().clone(),
                    )
                })
            })
            .collect();
        for (console, request, snapshot) in updates {
            if self
                .state
                .sql
                .manager
                .progress(&console, request, snapshot.clone())
                && let Some(panel) = self.state.sql.results.get_mut(&console)
            {
                panel.publish(snapshot);
                self.notifications.repaint();
            }
        }
    }
}
fn failed(error: BridgeError) -> Snapshot {
    Snapshot {
        phase: Phase::Finished,
        error: Some(error),
        ..Default::default()
    }
}
