//! Actual provider text in the shared editor, with no execution or save target.
use super::state::{AppState, AppWork};
use crate::editor::host::{EditorView, HostDescriptor, HostRole, QueuedEditorHost};
use kode_core::{DocumentOrigin, RequestId, Result};
use kode_database::model::MetadataSourceDocument;

impl AppState {
    pub fn open_database_source(
        &mut self,
        title: String,
        definitions: Vec<MetadataSourceDocument>,
    ) -> Result<()> {
        if self.closing
            || self.tabs.selected != super::panel::DATABASE
            || self.focus.focused != Some(self.database.tree.id)
        {
            self.notice =
                "Package source focus changed; reopen it from the database explorer".into();
            return Ok(());
        }
        let text = definitions
            .into_iter()
            .map(|definition| definition.text)
            .collect::<Vec<_>>()
            .join("\n\n");
        if text.is_empty() || text.len() > 3 * 1024 * 1024 {
            return Err(kode_core::DomainError::Unsupported(
                "Package source is empty or exceeds display bounds".into(),
            ));
        }
        self.cache_main_view()?;
        self.clear_diff()?;
        self.detach_image();
        self.active_hex = None;
        self.deactivate_rest_form();
        self.pending_open = None;
        let mut descriptor = HostDescriptor::new(
            HostRole::Historical,
            "database-package-source".into(),
            RequestId::fresh()?.0.to_string(),
            format!("Package source — {title}"),
            self.project.root.clone(),
        );
        descriptor.language = Some("sql".into());
        descriptor.mime = Some("application/sql".into());
        descriptor.grammar = "sql".into();
        let id = self
            .documents
            .create_virtual(&descriptor, self.generation, &text)?;
        let mut view = EditorView::new(
            id,
            &self.documents,
            QueuedEditorHost::new(descriptor),
            self.generation,
            self.clipboard.clone(),
            self.clock.clone(),
        )?;
        if let Some(mut old) = self.editor.take() {
            old.detach(&mut self.focus);
            let owner = old.owner();
            self.work.extend(
                old.take_work()
                    .into_iter()
                    .map(|work| AppWork::Editor { owner, work }),
            );
        }
        // Keep at most one ephemeral package source, including when a file or
        // SQL console has been selected between successive source requests.
        let previous: Vec<_> = self
            .documents
            .ids()
            .filter(|candidate| *candidate != id)
            .filter(|candidate| {
                self.documents.document(*candidate).is_ok_and(|doc| {
                    matches!(doc.origin(), DocumentOrigin::Virtual { owner, .. }
                        if owner == "database-package-source")
                })
            })
            .collect();
        for previous in previous {
            self.documents.remove(previous)?;
            self.views.remove(&previous);
        }
        self.focus.focused = Some(view.input.id);
        self.splash = false;
        let owner = view.owner();
        self.work.extend(
            view.take_work()
                .into_iter()
                .map(|work| AppWork::Editor { owner, work }),
        );
        self.editor = Some(view);
        self.changed()
    }
}
