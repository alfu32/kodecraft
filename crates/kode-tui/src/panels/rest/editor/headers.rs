use crate::editor::{
    host::QueuedEditorHost,
    popups::completion::{HostCompletionProvider, HostSuggestion},
};
use kode_core::{LineIndex, Result, intelligence::IntelligenceRequest};
use std::sync::Arc;

pub struct HeaderNames;
impl HostCompletionProvider for HeaderNames {
    fn suggestions(
        &self,
        request: &IntelligenceRequest,
        fragment: &str,
    ) -> Result<Vec<HostSuggestion>> {
        let Some(completion) =
            kode_rest::headers::complete(&request.snapshot.text, request.position.0)
        else {
            return Ok(Vec::new());
        };
        let prefix = &request.snapshot.text[completion.prefix.clone()];
        let Some(fixed) = prefix.strip_suffix(fragment) else {
            return Ok(Vec::new());
        };
        // The general editor's word prefix ends at '-'. Preserve that existing
        // part and replace only its current fragment, while displaying the full
        // HTTP field name in the shared completion popup.
        Ok(completion
            .names
            .into_iter()
            .map(|name| HostSuggestion {
                label: name.into(),
                insert_text: Some(name[fixed.len()..].into()),
                detail: Some("HTTP header".into()),
            })
            .collect())
    }
}
pub fn configure(host: &mut QueuedEditorHost, text: &str) {
    host.external = Some(Arc::new(HeaderNames));
    host.descriptor.auto_show_external = false;
    host.warnings = kode_rest::headers::duplicate_lines(text)
        .into_iter()
        .map(LineIndex)
        .collect();
}
