use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Autonomy level
// ---------------------------------------------------------------------------

/// How much freedom a creature has to act without approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    /// Every action requires explicit approval.
    None,
    /// Read-only / search actions auto-approved; writes need approval.
    Low,
    /// Writes within policy scope auto-approved; commits/externals need approval.
    Medium,
    /// Most actions auto-approved; only red-class actions need approval.
    High,
    /// Full autonomy — only hard policy blocks stop the creature.
    Full,
}

// ---------------------------------------------------------------------------
// Tool capability
// ---------------------------------------------------------------------------

/// A capability a creature is allowed to invoke.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolCapability {
    /// Tool name (e.g. "bash", "git", "browser", "mcp:search").
    pub name: String,
    /// Whether the tool is read-only for this creature.
    pub read_only: bool,
}

// ---------------------------------------------------------------------------
// World kind
// ---------------------------------------------------------------------------

/// The kind of execution environment a creature prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldKind {
    LocalShell,
    WorktreeReadOnly,
    WorktreeReadWrite,
    Container,
    MicroVm,
    BrowserSandbox,
    MobileEmulator,
    RemoteRunner,
}

// ---------------------------------------------------------------------------
// Stop conditions
// ---------------------------------------------------------------------------

/// Conditions under which a creature should halt execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopCondition {
    /// Confidence drops below threshold.
    ConfidenceBelow(f64),
    /// Token budget exhausted.
    BudgetExhausted,
    /// Wall-clock deadline exceeded.
    DeadlineExceeded,
    /// Explicit operator stop signal.
    OperatorStop,
    /// Maximum number of tool calls reached.
    MaxToolCalls(u32),
}

// ---------------------------------------------------------------------------
// Escalation rules
// ---------------------------------------------------------------------------

/// When and how a creature should escalate to the operator or another creature.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationRule {
    /// Escalate when conflicting evidence is found.
    ConflictingEvidence,
    /// Escalate when secrets or credentials are detected.
    SecretsDetected,
    /// Escalate when confidence drops below threshold.
    LowConfidence(f64),
    /// Escalate when a tool call fails repeatedly.
    RepeatedToolFailure { tool: String, max_retries: u32 },
    /// Escalate when proposed changes exceed a diff size threshold.
    LargeDiff { max_lines: u32 },
}

// ---------------------------------------------------------------------------
// Temperament
// ---------------------------------------------------------------------------

/// Behavioral profile influencing how a creature approaches work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Temperament {
    /// Read-heavy, cautious, low-write.
    Cautious,
    /// Balanced read/write, moderate risk tolerance.
    Balanced,
    /// Write-heavy, action-oriented.
    Aggressive,
    /// Long-running, patient, retry-tolerant.
    Patient,
    /// Fast, parallel, throughput-oriented.
    Throughput,
}

// ---------------------------------------------------------------------------
// Creature spec
// ---------------------------------------------------------------------------

/// The complete operational profile for a creature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatureSpec {
    /// Creature type name (e.g. "raven", "mantis", "hound").
    pub name: String,
    /// Human-readable role description.
    pub role: String,
    /// Behavioral temperament.
    pub temperament: Temperament,
    /// Tools this creature is allowed to invoke.
    pub toolbelt: Vec<ToolCapability>,
    /// Preferred execution environments, in priority order.
    pub preferred_worlds: Vec<WorldKind>,
    /// How much freedom the creature has.
    pub autonomy: AutonomyLevel,
    /// Conditions that halt the creature.
    pub stop_conditions: Vec<StopCondition>,
    /// Rules for when to escalate.
    pub escalation_rules: Vec<EscalationRule>,
    /// Optional token budget ceiling.
    pub token_budget: Option<u64>,
    /// Optional wall-clock deadline in seconds.
    pub deadline_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// Creature registry trait
// ---------------------------------------------------------------------------

/// Registry for resolving creature specs by name.
pub trait CreatureRegistry: Send + Sync {
    /// Return all built-in creature specs.
    fn builtin(&self) -> Vec<CreatureSpec>;
    /// Resolve a creature spec by name.
    fn resolve(&self, name: &str) -> anyhow::Result<CreatureSpec>;
}
