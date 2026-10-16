use kode_core::{ByteOffset, RequestId, Result, TextRange, document::TextSnapshot};
use kode_platform::jobs::CancellationToken;

#[derive(Debug, Clone)]
pub struct WordJob {
    pub request: RequestId,
    pub snapshot: TextSnapshot,
    pub position: ByteOffset,
    pub cancel: CancellationToken,
}
impl WordJob {
    pub fn run(&self, scheduler: &CancellationToken) -> Result<TextRange> {
        kode_core::editor::words::range_at(&self.snapshot, self.position, || {
            self.cancel.is_cancelled() || scheduler.is_cancelled()
        })
    }
}
