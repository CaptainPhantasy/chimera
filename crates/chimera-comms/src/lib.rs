use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

pub type MessageId = Uuid;
pub type PendingApprovalId = Uuid;

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

/// A communication channel for notifications and approvals.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Channel {
    /// Terminal inbox (default).
    Terminal,
    /// Slack channel or DM.
    Slack { channel: String },
    /// Discord channel.
    Discord { channel: String },
    /// Email address.
    Email { address: String },
    /// Voice call.
    Voice { number: String },
    /// Webhook URL.
    Webhook { url: String },
}

// ---------------------------------------------------------------------------
// Notification
// ---------------------------------------------------------------------------

/// Priority level for a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Normal,
    High,
    Urgent,
}

/// A notification to send to the operator or a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// Target channel.
    pub channel: Channel,
    /// Priority level.
    pub priority: Priority,
    /// Subject line.
    pub subject: String,
    /// Message body.
    pub body: String,
    /// Optional structured metadata.
    pub metadata: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Approval prompt (comms-layer)
// ---------------------------------------------------------------------------

/// An approval prompt to send via a communication channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommsApprovalPrompt {
    /// Target channel for the prompt.
    pub channel: Channel,
    /// Actor requesting approval.
    pub actor: String,
    /// Action class (green/yellow/orange/red).
    pub class: String,
    /// Description of the proposed action.
    pub action: String,
    /// Reason for the action.
    pub reason: String,
    /// Resources that will be touched.
    pub touched_resources: Vec<String>,
}

// ---------------------------------------------------------------------------
// Delivery receipt
// ---------------------------------------------------------------------------

/// Receipt for a delivered notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub message_id: MessageId,
    pub channel: Channel,
    pub delivered_at: DateTime<Utc>,
    pub acknowledged: bool,
}

// ---------------------------------------------------------------------------
// CommsBridge trait
// ---------------------------------------------------------------------------

/// Bridges notifications and approval prompts to external channels.
#[async_trait::async_trait]
pub trait CommsBridge: Send + Sync {
    /// Send a notification to a channel.
    async fn notify(&self, msg: Notification) -> anyhow::Result<MessageId>;

    /// Send an approval request via a channel and return a pending ID.
    async fn request_approval(
        &self,
        prompt: CommsApprovalPrompt,
    ) -> anyhow::Result<PendingApprovalId>;

    /// Check if a pending approval has been answered.
    async fn check_approval(&self, id: PendingApprovalId) -> anyhow::Result<Option<bool>>;

    /// Get the delivery receipt for a sent message.
    async fn receipt(&self, id: MessageId) -> anyhow::Result<Option<DeliveryReceipt>>;
}
