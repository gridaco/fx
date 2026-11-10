//! Display-only causes of control-flow decisions. They never participate in identity or replay.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

/// A stable causal category; presentation supplies the human wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipCode {
    ConditionFalse,
    EarlierTakeAccepted,
    EarlierTakeSettled,
    UpstreamFailed,
    UpstreamAbsent,
    Rejected,
    AssertionSkip,
    NoCandidate,
}

/// A recorded engine decision and the exact instances involved, without evaluated values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkipReason {
    pub code: SkipCode,
    pub sources: Vec<String>,
}

impl SkipReason {
    /// Canonical, deduplicated source identities make repeated snapshots stable.
    pub fn new(code: SkipCode, sources: impl IntoIterator<Item = String>) -> Self {
        Self {
            code,
            sources: sources
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }

    /// The additive event/viewer descriptor; no workflow values or authored text are copied.
    pub fn to_value(&self) -> Value {
        serde_json::json!(self)
    }
}

/// A conditional position omitted before a node type or its takes need to be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPosition {
    pub path: String,
    /// The surrounding scopes' takes, before a node or group adds its own take.
    pub takes: Vec<u32>,
    pub reason: SkipReason,
}

impl SkippedPosition {
    /// A known member at this exact position, or below a skipped group/import position.
    pub fn contains(&self, path: &str, takes: &[u32]) -> bool {
        takes.starts_with(&self.takes)
            && (path == self.path
                || path
                    .strip_prefix(&self.path)
                    .is_some_and(|rest| rest.starts_with('.')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_positions_match_only_their_own_scope_and_take_prefix() {
        let skipped = SkippedPosition {
            path: "item['a'].branch".into(),
            takes: vec![2],
            reason: SkipReason::new(SkipCode::ConditionFalse, []),
        };
        assert!(skipped.contains("item['a'].branch", &[2, 1]));
        assert!(skipped.contains("item['a'].branch.draw", &[2, 3, 1]));
        assert!(!skipped.contains("item['a'].branching", &[2, 1]));
        assert!(!skipped.contains("item['b'].branch.draw", &[2, 1]));
        assert!(!skipped.contains("item['a'].branch.draw", &[1, 3, 1]));
    }
}
