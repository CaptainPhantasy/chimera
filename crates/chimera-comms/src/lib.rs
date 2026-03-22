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

// ---------------------------------------------------------------------------
// Mock comms bridge
// ---------------------------------------------------------------------------

/// A mock CommsBridge that records messages in memory.
pub struct MockCommsBridge {
    messages: tokio::sync::RwLock<Vec<(MessageId, Notification)>>,
    approvals: tokio::sync::RwLock<std::collections::HashMap<PendingApprovalId, Option<bool>>>,
}

impl MockCommsBridge {
    pub fn new() -> Self {
        Self {
            messages: tokio::sync::RwLock::new(Vec::new()),
            approvals: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Simulate answering a pending approval.
    pub async fn answer_approval(&self, id: PendingApprovalId, approved: bool) {
        self.approvals.write().await.insert(id, Some(approved));
    }
}

impl Default for MockCommsBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl CommsBridge for MockCommsBridge {
    async fn notify(&self, msg: Notification) -> anyhow::Result<MessageId> {
        let id = Uuid::new_v4();
        self.messages.write().await.push((id, msg));
        Ok(id)
    }

    async fn request_approval(&self, _prompt: CommsApprovalPrompt) -> anyhow::Result<PendingApprovalId> {
        let id = Uuid::new_v4();
        self.approvals.write().await.insert(id, None);
        Ok(id)
    }

    async fn check_approval(&self, id: PendingApprovalId) -> anyhow::Result<Option<bool>> {
        Ok(self.approvals.read().await.get(&id).copied().flatten())
    }

    async fn receipt(&self, id: MessageId) -> anyhow::Result<Option<DeliveryReceipt>> {
        let msgs = self.messages.read().await;
        Ok(msgs.iter().find(|(mid, _)| *mid == id).map(|(mid, msg)| {
            DeliveryReceipt {
                message_id: *mid,
                channel: msg.channel.clone(),
                delivered_at: Utc::now(),
                acknowledged: true,
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_notification(subject: &str) -> Notification {
        Notification {
            channel: Channel::Terminal,
            priority: Priority::Normal,
            subject: subject.into(),
            body: format!("body of {}", subject),
            metadata: None,
        }
    }

    #[tokio::test]
    async fn notify_stores_message() {
        let bridge = MockCommsBridge::new();
        let id = bridge.notify(make_notification("test alert")).await.unwrap();

        let receipt = bridge.receipt(id).await.unwrap();
        assert!(receipt.is_some());
        assert!(receipt.unwrap().acknowledged);
    }

    #[tokio::test]
    async fn request_approval_starts_pending() {
        let bridge = MockCommsBridge::new();
        let prompt = CommsApprovalPrompt {
            channel: Channel::Slack { channel: "#ops".into() },
            actor: "MANTIS-1".into(),
            class: "yellow".into(),
            action: "modify 6 files".into(),
            reason: "extract auth kernel".into(),
            touched_resources: vec!["src/auth.rs".into()],
        };

        let id = bridge.request_approval(prompt).await.unwrap();
        let status = bridge.check_approval(id).await.unwrap();
        assert_eq!(status, None); // still pending
    }

    #[tokio::test]
    async fn approval_can_be_answered() {
        let bridge = MockCommsBridge::new();
        let prompt = CommsApprovalPrompt {
            channel: Channel::Terminal,
            actor: "OWL-1".into(),
            class: "green".into(),
            action: "analyze".into(),
            reason: "evaluation".into(),
            touched_resources: vec![],
        };

        let id = bridge.request_approval(prompt).await.unwrap();
        bridge.answer_approval(id, true).await;

        let status = bridge.check_approval(id).await.unwrap();
        assert_eq!(status, Some(true));
    }

    #[tokio::test]
    async fn receipt_returns_none_for_unknown() {
        let bridge = MockCommsBridge::new();
        let receipt = bridge.receipt(Uuid::new_v4()).await.unwrap();
        assert!(receipt.is_none());
    }
}
