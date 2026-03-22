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

// ---------------------------------------------------------------------------
// Mock evaluator for testing
// ---------------------------------------------------------------------------

/// A mock evaluator that runs graders as simple pass/fail checks.
pub struct MockEvaluator;

#[async_trait::async_trait]
impl Evaluator for MockEvaluator {
    async fn grade(&self, req: EvalRequest) -> anyhow::Result<EvalReport> {
        let grades: Vec<GradeResult> = req
            .graders
            .iter()
            .map(|g| {
                let passed = g.config.get("pass").and_then(|v| v.as_bool()).unwrap_or(true);
                let score = if passed { 1.0 } else { 0.0 };
                GradeResult {
                    grader: g.name.clone(),
                    passed,
                    score,
                    details: if passed {
                        "all checks passed".into()
                    } else {
                        "check failed".into()
                    },
                    evidence: serde_json::json!({}),
                }
            })
            .collect();

        let all_passed = grades.iter().all(|g| g.passed);
        let avg_score = if grades.is_empty() {
            1.0
        } else {
            grades.iter().map(|g| g.score).sum::<f64>() / grades.len() as f64
        };

        Ok(EvalReport {
            name: req.name,
            passed: all_passed,
            score: avg_score,
            grades,
            completed_at: Utc::now(),
        })
    }

    async fn compare(&self, a: CheckpointId, b: CheckpointId) -> anyhow::Result<DiffReport> {
        Ok(DiffReport {
            checkpoint_a: a,
            checkpoint_b: b,
            files_changed: vec![FileChange {
                path: "src/mock.rs".into(),
                change_type: ChangeType::Modified,
                lines_added: 10,
                lines_removed: 3,
            }],
            summary: "mock diff comparison".into(),
            regression_detected: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn grade_all_pass() {
        let eval = MockEvaluator;
        let req = EvalRequest {
            name: "test-eval".into(),
            target: EvalTarget::Artifact { path: "src/lib.rs".into() },
            graders: vec![
                GraderSpec { name: "lint".into(), config: serde_json::json!({"pass": true}) },
                GraderSpec { name: "tests".into(), config: serde_json::json!({"pass": true}) },
            ],
        };

        let report = eval.grade(req).await.unwrap();
        assert!(report.passed);
        assert_eq!(report.score, 1.0);
        assert_eq!(report.grades.len(), 2);
        assert!(report.grades.iter().all(|g| g.passed));
    }

    #[tokio::test]
    async fn grade_with_failure() {
        let eval = MockEvaluator;
        let req = EvalRequest {
            name: "failing-eval".into(),
            target: EvalTarget::Artifact { path: "src/bad.rs".into() },
            graders: vec![
                GraderSpec { name: "lint".into(), config: serde_json::json!({"pass": true}) },
                GraderSpec { name: "security".into(), config: serde_json::json!({"pass": false}) },
            ],
        };

        let report = eval.grade(req).await.unwrap();
        assert!(!report.passed);
        assert_eq!(report.score, 0.5);
        assert!(!report.grades[1].passed);
    }

    #[tokio::test]
    async fn grade_empty_graders() {
        let eval = MockEvaluator;
        let req = EvalRequest {
            name: "empty".into(),
            target: EvalTarget::Session { session_id: Uuid::new_v4() },
            graders: vec![],
        };

        let report = eval.grade(req).await.unwrap();
        assert!(report.passed);
        assert_eq!(report.score, 1.0);
    }

    #[tokio::test]
    async fn compare_checkpoints() {
        let eval = MockEvaluator;
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        let diff = eval.compare(a, b).await.unwrap();
        assert_eq!(diff.checkpoint_a, a);
        assert_eq!(diff.checkpoint_b, b);
        assert_eq!(diff.files_changed.len(), 1);
        assert_eq!(diff.files_changed[0].path, "src/mock.rs");
        assert!(!diff.regression_detected);
    }
}
