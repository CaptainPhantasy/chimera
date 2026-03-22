use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

pub type CheckpointId = Uuid;

// ---------------------------------------------------------------------------
// Eval request
// ---------------------------------------------------------------------------

/// A request to evaluate an artifact or run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRequest {
    /// Human-readable name for this evaluation.
    pub name: String,
    /// What to evaluate (session ID, artifact path, etc.).
    pub target: EvalTarget,
    /// Which graders to run.
    pub graders: Vec<GraderSpec>,
}

/// What the evaluation targets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EvalTarget {
    /// Evaluate a session's output.
    Session { session_id: Uuid },
    /// Evaluate a specific file or artifact.
    Artifact { path: String },
    /// Evaluate a diff between two states.
    Diff { before: String, after: String },
}

/// Specification for a grader to run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraderSpec {
    /// Grader name (e.g. "test_pass", "lint_clean", "regression_check").
    pub name: String,
    /// Grader-specific configuration.
    pub config: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Eval report
// ---------------------------------------------------------------------------

/// The result of an evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    /// Evaluation name.
    pub name: String,
    /// Overall pass/fail.
    pub passed: bool,
    /// Overall score (0.0 to 1.0).
    pub score: f64,
    /// Per-grader results.
    pub grades: Vec<GradeResult>,
    /// When the evaluation completed.
    pub completed_at: DateTime<Utc>,
}

/// Result from a single grader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradeResult {
    /// Grader name.
    pub grader: String,
    /// Pass/fail for this grader.
    pub passed: bool,
    /// Score from this grader (0.0 to 1.0).
    pub score: f64,
    /// Human-readable details.
    pub details: String,
    /// Structured evidence.
    pub evidence: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Diff report
// ---------------------------------------------------------------------------

/// A comparison between two checkpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffReport {
    /// First checkpoint.
    pub checkpoint_a: CheckpointId,
    /// Second checkpoint.
    pub checkpoint_b: CheckpointId,
    /// Files changed.
    pub files_changed: Vec<FileChange>,
    /// Summary of differences.
    pub summary: String,
    /// Regression detected.
    pub regression_detected: bool,
}

/// A file change in a diff report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub change_type: ChangeType,
    pub lines_added: u32,
    pub lines_removed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
}

// ---------------------------------------------------------------------------
// Evaluator trait
// ---------------------------------------------------------------------------

/// Runs evaluations, grades artifacts, and compares checkpoints.
#[async_trait::async_trait]
pub trait Evaluator: Send + Sync {
    /// Run an evaluation and return a report.
    async fn grade(&self, req: EvalRequest) -> anyhow::Result<EvalReport>;

    /// Compare two checkpoints and return a diff report.
    async fn compare(&self, a: CheckpointId, b: CheckpointId) -> anyhow::Result<DiffReport>;
}
