use super::{CompletionCandidate, CompletionContext};

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CompletionScore {
    pub prefix: f32,
    pub scope: f32,
    pub r#type: f32,
    pub recency: f32,
    pub frequency: f32,
    pub optional: f32,
}
impl CompletionScore {
    pub fn total(self) -> f32 {
        self.prefix * 0.30
            + self.scope * 0.15
            + self.r#type * 0.25
            + self.recency * 0.10
            + self.frequency * 0.10
            + self.optional * 0.10
    }
    pub(super) fn valid(self) -> bool {
        [
            self.prefix,
            self.scope,
            self.r#type,
            self.recency,
            self.frequency,
            self.optional,
        ]
        .into_iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
    }
}
/// Optional rankers score existing candidates; they cannot introduce bindings.
/// No learned model, model runtime or ONNX dependency is included.
pub trait CompletionRanker: Send + Sync {
    fn score(
        &self,
        context: &CompletionContext,
        candidate: &CompletionCandidate,
    ) -> CompletionScore;
}
pub struct HeuristicCompletionRanker;
impl CompletionRanker for HeuristicCompletionRanker {
    fn score(
        &self,
        context: &CompletionContext,
        candidate: &CompletionCandidate,
    ) -> CompletionScore {
        let prefix = context.prefix.to_lowercase();
        let label = candidate.label.to_lowercase();
        CompletionScore {
            prefix: if prefix.is_empty() {
                0.5
            } else if label == prefix {
                1.0
            } else if label.starts_with(&prefix) {
                0.9
            } else {
                0.0
            },
            scope: candidate
                .scope_distance
                .map_or(0.0, |distance| 1.0 / (1.0 + distance as f32)),
            r#type: if context.member_access && candidate.member {
                1.0
            } else if candidate.member {
                0.6
            } else {
                0.3
            },
            recency: if candidate.same_file_count > 0 {
                0.75
            } else {
                0.0
            },
            frequency: (((candidate.workspace_frequency as f64).ln_1p() / 16.0f64.ln())
                .clamp(0.0, 1.0)) as f32,
            optional: 0.0,
        }
    }
}
