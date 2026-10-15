use super::{OverlayJob, OverlayReport, PaintSpan, compose, remap::remap_ranges};
use crate::{editor::render::OverlaySpan, theme::Theme};
use kode_core::{
    DomainError, JobTag, Result, TextRange, WorkspaceGeneration,
    document::{EditDelta, TextSnapshot},
};
use kode_syntax::{model::GrammarStatus, service::tag_for};

#[derive(Default)]
pub struct OverlayCache {
    enabled: bool,
    dirty: bool,
    pending: Option<OverlayJob>,
    snapshot: Option<TextSnapshot>,
    coverage: Option<TextRange>,
    lexical: Vec<PaintSpan>,
    semantic: Vec<PaintSpan>,
    pub status: Option<GrammarStatus>,
    pub notes: Vec<String>,
}
impl OverlayCache {
    pub fn enable(&mut self) {
        if !self.enabled {
            self.enabled = true;
            self.dirty = true;
        }
    }
    pub fn invalidate(&mut self) {
        self.dirty = true;
        self.pending = None;
    }
    pub fn changed(&mut self, snapshot: &TextSnapshot, deltas: Option<&[EditDelta]>) {
        if let Some(old) = &self.snapshot {
            let ranges: Vec<_> = self
                .lexical
                .iter()
                .chain(&self.semantic)
                .map(|s| s.range)
                .collect();
            let mut remapped = remap_ranges(old, snapshot, &ranges, deltas).into_iter();
            for spans in [&mut self.lexical, &mut self.semantic] {
                spans.retain_mut(|span| {
                    if let Some(Some(range)) = remapped.next() {
                        span.range = range;
                        true
                    } else {
                        false
                    }
                });
            }
        }
        self.snapshot = Some(snapshot.clone());
        self.coverage = None;
        self.invalidate();
    }
    pub fn request(
        &mut self,
        snapshot: &TextSnapshot,
        visible: TextRange,
        coverage: TextRange,
        workspace: WorkspaceGeneration,
        language: String,
    ) -> Result<Option<OverlayJob>> {
        if !self.enabled {
            return Ok(None);
        }
        if self
            .snapshot
            .as_ref()
            .is_none_or(|s| (s.id, s.revision) != (snapshot.id, snapshot.revision))
        {
            self.changed(snapshot, None);
        }
        let contains = |range: TextRange| range.start <= visible.start && visible.end <= range.end;
        if !self.dirty
            && (self.coverage.is_some_and(contains)
                || self.pending.as_ref().is_some_and(|job| contains(job.range)))
        {
            return Ok(None);
        }
        self.dirty = false;
        let job = OverlayJob {
            tag: tag_for(snapshot, workspace)?,
            snapshot: snapshot.clone(),
            range: coverage,
            language,
        };
        self.pending = Some(job.clone());
        Ok(Some(job))
    }
    pub fn complete(
        &mut self,
        tag: JobTag,
        result: Result<OverlayReport>,
        snapshot: &TextSnapshot,
    ) -> Result<bool> {
        if self.pending.as_ref().is_none_or(|job| job.tag != tag)
            || tag.document != Some((snapshot.id, snapshot.revision))
        {
            return Ok(false);
        }
        let pending = self.pending.take().expect("matching pending job");
        match result {
            Ok(report) => {
                if report.tag != tag || report.range != pending.range {
                    self.dirty = true;
                    return Err(DomainError::Unsupported(
                        "mismatched highlight report".into(),
                    ));
                }
                // Validate the entire report before publishing any of it.
                for span in report
                    .lexical
                    .iter()
                    .chain(report.semantic.iter().flatten())
                {
                    snapshot.slice(span.range)?;
                }
                self.lexical = report.lexical;
                if let Some(semantic) = report.semantic {
                    self.semantic = semantic;
                }
                self.coverage = Some(report.range);
                self.status = Some(report.status);
                self.notes = report.notes;
                self.snapshot = Some(snapshot.clone());
                Ok(true)
            }
            Err(DomainError::Cancelled) => {
                self.dirty = true;
                Ok(false)
            }
            Err(error) => {
                self.coverage = Some(pending.range);
                self.notes = vec![error.to_string()];
                Ok(true)
            }
        }
    }
    pub fn spans(&self) -> impl Iterator<Item = &PaintSpan> {
        self.lexical.iter().chain(&self.semantic)
    }
    pub fn declarations(&self) -> Vec<TextRange> {
        self.semantic
            .iter()
            .filter(|s| s.class == "codeintel.declaration")
            .map(|s| s.range)
            .collect()
    }
    pub fn label(&self, fallback: &str) -> String {
        let mut label = self.status.as_ref().map_or_else(
            || fallback.to_owned(),
            |status| {
                format!(
                    "{:?}{}",
                    status.mode,
                    status
                        .message
                        .as_ref()
                        .map_or(String::new(), |message| format!(" ({message})"))
                )
            },
        );
        if let Some(note) = self.notes.first() {
            label.push_str(&format!(" | {note}"));
        }
        label
    }
    pub fn overlays(&self, theme: &Theme) -> Vec<OverlaySpan> {
        compose(&self.spans().cloned().collect::<Vec<_>>(), theme)
    }
}
