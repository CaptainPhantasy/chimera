//! Explicit destinations and durable, single-dispatch notifications.
//! A successful webhook response proves HTTP acceptance, not human acknowledgement.
use crate::{
    Channel, CommsApprovalPrompt, CommsBridge, DeliveryReceipt, MessageId, Notification,
    PendingApprovalId, Priority,
};
use anyhow::{ensure, Context};
use chimera_memory::storage::ProjectRecords;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Destination {
    pub name: String,
    pub channel: Channel,
    pub enabled: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Dispatching,
    Delivered,
    Failed,
    Ambiguous,
    Expired,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub id: MessageId,
    pub destination: String,
    pub message: Notification,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub attempted_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: DeliveryStatus,
    pub http_status: Option<u16>,
    pub response_preview: String,
    pub response_truncated: bool,
    pub error: Option<String>,
    pub receipt: Option<DeliveryReceipt>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPrompt {
    pub id: PendingApprovalId,
    pub prompt: CommsApprovalPrompt,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub answer: Option<bool>,
    pub consumed: bool,
}
#[derive(Default, Serialize, Deserialize)]
struct Records {
    destinations: BTreeMap<String, Destination>,
    destination_history: Vec<Destination>,
    deliveries: BTreeMap<MessageId, DeliveryRecord>,
    approvals: BTreeMap<PendingApprovalId, PendingPrompt>,
}

pub struct FileCommsBridge {
    records: ProjectRecords,
    client: reqwest::Client,
    test_http: bool,
}
impl FileCommsBridge {
    pub fn new(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        Self::build(root.into(), Duration::from_secs(10), false)
    }
    fn build(root: PathBuf, timeout: Duration, test_http: bool) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent(concat!("chimera/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            records: ProjectRecords::new(root, "comms"),
            client,
            test_http,
        })
    }
    /// Configuration is owner intent, not a grant for a model to send arbitrary content.
    pub fn configure_destination(
        &self,
        name: &str,
        channel: Channel,
    ) -> anyhow::Result<Destination> {
        ensure!(
            !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
            "invalid destination name"
        );
        validate_channel(&channel, self.test_http)?;
        let destination = Destination {
            name: name.into(),
            channel,
            enabled: true,
        };
        self.records.update("records", |records: &mut Records| {
            ensure!(
                records.destinations.contains_key(name) || records.destinations.len() < 100,
                "destination limit reached"
            );
            if let Some(previous) = records
                .destinations
                .insert(name.into(), destination.clone())
            {
                records.destination_history.push(previous);
            }
            Ok(destination.clone())
        })
    }
    pub fn destinations(&self) -> anyhow::Result<Vec<Destination>> {
        Ok(self
            .records
            .read::<Records>("records")?
            .destinations
            .into_values()
            .collect())
    }
    pub fn disable_destination(&self, name: &str) -> anyhow::Result<()> {
        self.records.update("records", |records: &mut Records| {
            let destination = records
                .destinations
                .get_mut(name)
                .context("destination not found")?;
            destination.enabled = false;
            Ok(())
        })
    }
    pub fn prepare(
        &self,
        destination: &str,
        message: Notification,
        lifetime: Duration,
    ) -> anyhow::Result<DeliveryRecord> {
        ensure!(
            !lifetime.is_zero() && lifetime <= Duration::from_secs(86400),
            "notification lifetime must be between zero and 24 hours, excluding zero"
        );
        validate_message(&message)?;
        let now = Utc::now();
        let row = pending(
            Uuid::new_v4(),
            destination,
            message,
            now,
            now + chrono::Duration::from_std(lifetime)?,
        );
        self.records.update("records", |records: &mut Records| {
            check_destination(records, &row)?;
            insert_delivery(records, row.clone())?;
            Ok(row)
        })
    }
    pub fn list(&self) -> anyhow::Result<Vec<DeliveryRecord>> {
        Ok(self
            .records
            .read::<Records>("records")?
            .deliveries
            .into_values()
            .map(expire_view)
            .collect())
    }
    pub fn get(&self, id: MessageId) -> anyhow::Result<DeliveryRecord> {
        self.records
            .read::<Records>("records")?
            .deliveries
            .remove(&id)
            .map(expire_view)
            .context("notification not found in this project")
    }
    pub fn inbox(&self) -> anyhow::Result<Vec<DeliveryRecord>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|r| {
                matches!(r.message.channel, Channel::Terminal)
                    && r.status == DeliveryStatus::Delivered
            })
            .collect())
    }
    pub fn acknowledge(&self, id: MessageId) -> anyhow::Result<DeliveryReceipt> {
        self.records.update("records", |records: &mut Records| {
            let row = records
                .deliveries
                .get_mut(&id)
                .context("notification not found")?;
            let receipt = row
                .receipt
                .as_mut()
                .context("notification has not been delivered")?;
            receipt.acknowledged = true;
            Ok(receipt.clone())
        })
    }
    pub fn cancel(&self, id: MessageId) -> anyhow::Result<()> {
        self.records.update("records", |records: &mut Records| {
            let row = records
                .deliveries
                .get_mut(&id)
                .context("notification not found")?;
            ensure!(
                row.status == DeliveryStatus::Pending,
                "only a pending notification can be cancelled"
            );
            row.status = DeliveryStatus::Cancelled;
            row.finished_at = Some(Utc::now());
            Ok(())
        })
    }
    /// Caller must first approve this stored destination and payload through its policy.
    /// The UUID is the transport idempotency key. Existing dispatches are never repeated.
    pub async fn deliver(&self, id: MessageId) -> anyhow::Result<DeliveryRecord> {
        let row = self.records.update("records", |records: &mut Records| {
            let row = records.deliveries.get(&id).cloned().context("notification not found")?;
            if row.status == DeliveryStatus::Delivered { return Ok(row); }
            ensure!(row.status == DeliveryStatus::Pending, "notification is not pending ({:?}); previous dispatch may have reached its destination and will not be repeated", row.status);
            if row.expires_at <= Utc::now() {
                let row = records.deliveries.get_mut(&id).unwrap(); row.status = DeliveryStatus::Expired; row.finished_at = Some(Utc::now()); return Ok(row.clone());
            }
            check_destination(records, &row)?;
            validate_channel(&row.message.channel, self.test_http)?;
            let row = records.deliveries.get_mut(&id).unwrap(); row.status = DeliveryStatus::Dispatching; row.attempted_at = Some(Utc::now()); Ok(row.clone())
        })?;
        if matches!(
            row.status,
            DeliveryStatus::Delivered | DeliveryStatus::Expired
        ) {
            return Ok(row);
        }
        let attempt = match &row.message.channel {
            Channel::Terminal => Attempt {
                status: DeliveryStatus::Delivered,
                http_status: None,
                response: "Saved to this project's terminal inbox".into(),
                truncated: false,
                error: None,
            },
            Channel::Webhook { url } => self.webhook(url, &row).await,
            _ => anyhow::bail!("unsupported notification channel"),
        };
        self.records.update("records", |records: &mut Records| {
            let row = records
                .deliveries
                .get_mut(&id)
                .context("notification disappeared after dispatch")?;
            ensure!(
                row.status == DeliveryStatus::Dispatching,
                "notification dispatch state changed unexpectedly"
            );
            row.status = attempt.status;
            row.http_status = attempt.http_status;
            row.response_preview = attempt.response;
            row.response_truncated = attempt.truncated;
            row.error = attempt.error;
            row.finished_at = Some(Utc::now());
            if row.status == DeliveryStatus::Delivered {
                row.receipt = Some(DeliveryReceipt {
                    message_id: id,
                    channel: row.message.channel.clone(),
                    delivered_at: row.finished_at.unwrap(),
                    acknowledged: false,
                });
            }
            Ok(row.clone())
        })
    }
    async fn webhook(&self, url: &str, row: &DeliveryRecord) -> Attempt {
        let response = self.client.post(url).header("Idempotency-Key", row.id.to_string())
            .json(&serde_json::json!({"id":row.id,"created_at":row.created_at,"expires_at":row.expires_at,"notification":row.message})).send().await;
        match response {
            Ok(mut response) => {
                let http_status = response.status();
                let mut bytes = Vec::new(); let mut truncated = false;
                loop {
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            let keep = chunk.len().min(8192usize.saturating_sub(bytes.len())); bytes.extend_from_slice(&chunk[..keep]);
                            if keep < chunk.len() { truncated = true; break; }
                        }
                        Ok(None) => break,
                        Err(error) => return Attempt { status: DeliveryStatus::Ambiguous, http_status: Some(http_status.as_u16()), response: bounded(&String::from_utf8_lossy(&bytes), 8192), truncated, error: Some(format!("response was incomplete; delivery may have occurred: {}", error.without_url())) },
                    }
                }
                truncated |= String::from_utf8_lossy(&bytes).len() > 8192;
                let status = if http_status.is_success() { DeliveryStatus::Delivered } else { DeliveryStatus::Failed };
                Attempt { status, http_status: Some(http_status.as_u16()), response: bounded(&String::from_utf8_lossy(&bytes), 8192), truncated,
                    error: if status == DeliveryStatus::Failed { Some(format!("webhook returned HTTP {}; no automatic retry", http_status.as_u16())) } else { None } }
            }
            Err(error) => Attempt { status: DeliveryStatus::Ambiguous, http_status: None, response: String::new(), truncated: false,
                error: Some(format!("transport did not confirm delivery; destination may have received the message: {}", error.without_url())) },
        }
    }
    fn destination_for(&self, channel: &Channel) -> anyhow::Result<String> {
        validate_channel(channel, self.test_http)?;
        self.destinations()?
            .into_iter()
            .find(|d| d.enabled && same_channel(&d.channel, channel))
            .map(|d| d.name)
            .context("channel has no enabled, owner-configured destination")
    }
    /// This records a communication prompt only. Execution still requires the core's
    /// exact tool-call approval. A remote HTTP response never sets `answer`.
    pub fn prepare_approval(
        &self,
        id: PendingApprovalId,
        prompt: CommsApprovalPrompt,
        expires_at: DateTime<Utc>,
    ) -> anyhow::Result<PendingPrompt> {
        ensure!(
            expires_at > Utc::now() && expires_at <= Utc::now() + chrono::Duration::minutes(5),
            "approval prompt must expire within five minutes"
        );
        ensure!(
            serde_json::to_vec(&prompt)?.len() <= 32_768,
            "approval prompt exceeds size limit"
        );
        ensure!(
            !prompt.actor.trim().is_empty() && !prompt.action.trim().is_empty(),
            "approval actor and action are required"
        );
        let destination = self.destination_for(&prompt.channel)?;
        let now = Utc::now();
        let approval = PendingPrompt {
            id,
            prompt: prompt.clone(),
            created_at: now,
            expires_at,
            answer: None,
            consumed: false,
        };
        let message = Notification {
            channel: prompt.channel.clone(),
            priority: Priority::High,
            subject: "Approval requested".into(),
            body: format!("{}: {}\n{}", prompt.actor, prompt.action, prompt.reason),
            metadata: Some(
                serde_json::json!({"pending_id":id,"expires_at":expires_at,"prompt":prompt}),
            ),
        };
        validate_message(&message)?;
        let delivery = pending(id, &destination, message, now, expires_at);
        self.records.update("records", |records: &mut Records| {
            if let Some(existing) = records.approvals.get(&id) {
                ensure!(
                    serde_json::to_value(&existing.prompt)?
                        == serde_json::to_value(&approval.prompt)?
                        && existing.expires_at == approval.expires_at,
                    "pending approval identifier belongs to another prompt"
                );
                return Ok(existing.clone());
            }
            check_destination(records, &delivery)?;
            insert_delivery(records, delivery)?;
            records.approvals.insert(id, approval.clone());
            Ok(approval)
        })
    }
    pub fn pending_approvals(&self) -> anyhow::Result<Vec<PendingPrompt>> {
        Ok(self
            .records
            .read::<Records>("records")?
            .approvals
            .into_values()
            .collect())
    }
    /// Local operator action only. The caller must bind this decision to the same
    /// core pending action before it can authorize execution.
    pub fn answer_approval(
        &self,
        id: PendingApprovalId,
        approved: bool,
    ) -> anyhow::Result<PendingPrompt> {
        self.records.update("records", |records: &mut Records| {
            let prompt = records
                .approvals
                .get_mut(&id)
                .context("approval prompt not found")?;
            ensure!(prompt.expires_at > Utc::now(), "approval prompt expired");
            ensure!(
                prompt.answer.is_none() && !prompt.consumed,
                "approval prompt was already answered or consumed"
            );
            prompt.answer = Some(approved);
            Ok(prompt.clone())
        })
    }
    pub fn consume_approval(
        &self,
        id: PendingApprovalId,
        expected: &CommsApprovalPrompt,
    ) -> anyhow::Result<bool> {
        self.records.update("records", |records: &mut Records| {
            let prompt = records
                .approvals
                .get_mut(&id)
                .context("approval prompt not found")?;
            ensure!(
                prompt.expires_at > Utc::now() && !prompt.consumed,
                "approval prompt expired or was consumed"
            );
            ensure!(
                serde_json::to_value(&prompt.prompt)? == serde_json::to_value(expected)?,
                "approval prompt does not match the proposed action"
            );
            let answer = prompt
                .answer
                .context("approval prompt has not been answered")?;
            prompt.consumed = true;
            Ok(answer)
        })
    }
}
#[async_trait::async_trait]
impl CommsBridge for FileCommsBridge {
    async fn notify(&self, message: Notification) -> anyhow::Result<MessageId> {
        let destination = self.destination_for(&message.channel)?;
        let row = self.prepare(&destination, message, Duration::from_secs(300))?;
        let row = self.deliver(row.id).await?;
        ensure!(
            row.status == DeliveryStatus::Delivered,
            "notification {} was {:?}: {}",
            row.id,
            row.status,
            row.error.as_deref().unwrap_or("not delivered")
        );
        Ok(row.id)
    }
    async fn request_approval(
        &self,
        prompt: CommsApprovalPrompt,
    ) -> anyhow::Result<PendingApprovalId> {
        let id = Uuid::new_v4();
        self.prepare_approval(id, prompt, Utc::now() + chrono::Duration::minutes(5))?;
        let row = self.deliver(id).await?;
        ensure!(
            row.status == DeliveryStatus::Delivered,
            "approval prompt {id} was not delivered: {:?}",
            row.status
        );
        Ok(id)
    }
    async fn check_approval(&self, id: PendingApprovalId) -> anyhow::Result<Option<bool>> {
        let records: Records = self.records.read("records")?;
        let prompt = records
            .approvals
            .get(&id)
            .context("approval prompt not found")?;
        ensure!(prompt.expires_at > Utc::now(), "approval prompt expired");
        ensure!(!prompt.consumed, "approval prompt was already consumed");
        Ok(prompt.answer)
    }
    async fn receipt(&self, id: MessageId) -> anyhow::Result<Option<DeliveryReceipt>> {
        Ok(self
            .records
            .read::<Records>("records")?
            .deliveries
            .get(&id)
            .and_then(|r| r.receipt.clone()))
    }
}
struct Attempt {
    status: DeliveryStatus,
    http_status: Option<u16>,
    response: String,
    truncated: bool,
    error: Option<String>,
}
fn pending(
    id: Uuid,
    destination: &str,
    message: Notification,
    now: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> DeliveryRecord {
    DeliveryRecord {
        id,
        destination: destination.into(),
        message,
        created_at: now,
        expires_at,
        attempted_at: None,
        finished_at: None,
        status: DeliveryStatus::Pending,
        http_status: None,
        response_preview: String::new(),
        response_truncated: false,
        error: None,
        receipt: None,
    }
}
fn expire_view(mut row: DeliveryRecord) -> DeliveryRecord {
    if row.status == DeliveryStatus::Pending && row.expires_at <= Utc::now() {
        row.status = DeliveryStatus::Expired;
    }
    row
}
fn same_channel(a: &Channel, b: &Channel) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}
fn check_destination(records: &Records, row: &DeliveryRecord) -> anyhow::Result<()> {
    let destination = records
        .destinations
        .get(&row.destination)
        .context("destination is not configured")?;
    ensure!(destination.enabled, "destination is disabled");
    ensure!(
        same_channel(&destination.channel, &row.message.channel),
        "saved message destination does not match the current configuration; prepare a new message"
    );
    Ok(())
}
fn insert_delivery(records: &mut Records, row: DeliveryRecord) -> anyhow::Result<()> {
    ensure!(
        records.deliveries.len() < 10_000,
        "notification history limit reached; archive the project state before sending more"
    );
    ensure!(
        !records.deliveries.contains_key(&row.id),
        "notification identifier already exists"
    );
    records.deliveries.insert(row.id, row);
    Ok(())
}
fn validate_message(message: &Notification) -> anyhow::Result<()> {
    ensure!(
        !message.subject.trim().is_empty() && message.subject.len() <= 256,
        "notification subject must have 1 to 256 bytes"
    );
    ensure!(
        serde_json::to_vec(message)?.len() <= 65_536,
        "notification exceeds 64 KiB"
    );
    Ok(())
}
fn validate_channel(channel: &Channel, test_http: bool) -> anyhow::Result<()> {
    match channel {
        Channel::Terminal => Ok(()),
        Channel::Webhook { url } => {
            ensure!(url.len() <= 4096, "webhook URL is too long");
            let parsed = reqwest::Url::parse(url).context("invalid webhook URL")?;
            let test_loopback = test_http && parsed.scheme() == "http" && matches!(parsed.host_str(), Some("127.0.0.1") | Some("[::1]") | Some("::1"));
            ensure!(parsed.scheme() == "https" || test_loopback, "webhook requires HTTPS");
            ensure!(parsed.host_str().is_some() && parsed.username().is_empty() && parsed.password().is_none() && parsed.fragment().is_none(), "webhook URL must have a host and no embedded username, password or fragment"); Ok(())
        }
        Channel::Slack { .. } => anyhow::bail!("Slack channel delivery is not configured; configure an explicit HTTPS webhook destination"),
        Channel::Discord { .. } => anyhow::bail!("Discord channel delivery is not configured; configure an explicit HTTPS webhook destination"),
        Channel::Email { .. } => anyhow::bail!("email delivery requires an implemented provider; no email was sent"),
        Channel::Voice { .. } => anyhow::bail!("voice delivery requires an implemented provider; no call was placed"),
    }
}
fn bounded(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("chimera-comms-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        root
    }
    fn message(channel: Channel) -> Notification {
        Notification {
            channel,
            priority: Priority::Normal,
            subject: "Exact subject".into(),
            body: "Exact authorized text".into(),
            metadata: None,
        }
    }
    fn test_bridge(root: PathBuf, timeout: Duration) -> FileCommsBridge {
        FileCommsBridge::build(root, timeout, true).unwrap()
    }
    async fn receiver(
        status: u16,
        body: Vec<u8>,
        delay: Duration,
    ) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                assert!(data.len() <= 200_000);
                if let Some(end) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&data[..end]);
                    let length: usize = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|n| n.trim().parse().unwrap())
                        })
                        .unwrap();
                    if data.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            tokio::time::sleep(delay).await;
            let header = format!(
                "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if stream.write_all(header.as_bytes()).await.is_ok() {
                let _ = stream.write_all(&body).await;
            }
            drop(stream);
            // A duplicate deliver call must not open another connection.
            assert!(
                tokio::time::timeout(Duration::from_millis(150), listener.accept())
                    .await
                    .is_err()
            );
            data
        });
        (format!("http://{address}/owner-configured"), task)
    }
    #[tokio::test]
    async fn terminal_inbox_survives_restart_and_does_not_auto_acknowledge() {
        let root = root();
        let bridge = FileCommsBridge::new(&root).unwrap();
        assert!(bridge.notify(message(Channel::Terminal)).await.is_err());
        bridge
            .configure_destination("inbox", Channel::Terminal)
            .unwrap();
        let id = bridge.notify(message(Channel::Terminal)).await.unwrap();
        let receipt = bridge.receipt(id).await.unwrap().unwrap();
        assert!(!receipt.acknowledged);
        let other = FileCommsBridge::new(&root).unwrap();
        assert_eq!(
            other.inbox().unwrap()[0].message.body,
            "Exact authorized text"
        );
        assert_eq!(
            receipt.delivered_at,
            other.receipt(id).await.unwrap().unwrap().delivered_at
        );
        assert!(other.acknowledge(id).unwrap().acknowledged);
        assert!(bridge.receipt(id).await.unwrap().unwrap().acknowledged);
    }
    #[tokio::test]
    async fn actual_http_delivery_has_stable_id_payload_receipt_and_no_duplicate_send() {
        let (url, server) = receiver(202, b"accepted".to_vec(), Duration::ZERO).await;
        let root = root();
        let bridge = test_bridge(root.clone(), Duration::from_secs(2));
        let channel = Channel::Webhook { url: url.clone() };
        bridge
            .configure_destination("hook", channel.clone())
            .unwrap();
        let row = bridge
            .prepare("hook", message(channel), Duration::from_secs(60))
            .unwrap();
        assert!(bridge.receipt(row.id).await.unwrap().is_none());
        let delivered = bridge.deliver(row.id).await.unwrap();
        assert_eq!(delivered.status, DeliveryStatus::Delivered);
        assert_eq!(delivered.http_status, Some(202));
        assert!(!delivered.receipt.as_ref().unwrap().acknowledged);
        assert_eq!(delivered.response_preview, "accepted");
        let duplicate = test_bridge(root, Duration::from_secs(2))
            .deliver(row.id)
            .await
            .unwrap();
        assert_eq!(
            duplicate.receipt.unwrap().delivered_at,
            delivered.receipt.unwrap().delivered_at
        );
        let request = server.await.unwrap();
        let request = String::from_utf8(request).unwrap();
        assert!(request
            .to_ascii_lowercase()
            .contains(&format!("idempotency-key: {}", row.id)));
        let body: serde_json::Value =
            serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["id"], row.id.to_string());
        assert_eq!(body["notification"]["body"], "Exact authorized text");
    }
    #[tokio::test]
    async fn actual_http_failure_timeout_and_bounded_response_are_saved() {
        for (status, delay, expected) in [
            (503, Duration::ZERO, DeliveryStatus::Failed),
            (200, Duration::from_millis(200), DeliveryStatus::Ambiguous),
        ] {
            let (url, server) = receiver(status, b"actual server message".to_vec(), delay).await;
            let bridge = test_bridge(root(), Duration::from_millis(100));
            let channel = Channel::Webhook { url };
            bridge
                .configure_destination("hook", channel.clone())
                .unwrap();
            let row = bridge
                .prepare("hook", message(channel), Duration::from_secs(60))
                .unwrap();
            let result = bridge.deliver(row.id).await.unwrap();
            assert_eq!(result.status, expected);
            assert!(result.error.is_some());
            assert!(bridge.receipt(row.id).await.unwrap().is_none());
            assert!(bridge.deliver(row.id).await.is_err());
            server.await.unwrap();
        }
        let (url, server) = receiver(200, vec![255; 5000], Duration::ZERO).await;
        let bridge = test_bridge(root(), Duration::from_secs(2));
        let channel = Channel::Webhook { url };
        bridge
            .configure_destination("hook", channel.clone())
            .unwrap();
        let row = bridge
            .prepare("hook", message(channel), Duration::from_secs(60))
            .unwrap();
        let row = bridge.deliver(row.id).await.unwrap();
        assert!(row.response_truncated);
        assert!(row.response_preview.len() <= 8192);
        server.await.unwrap();
    }
    #[tokio::test]
    async fn expired_cancelled_changed_destination_and_in_flight_rows_never_send() {
        let bridge = FileCommsBridge::new(root()).unwrap();
        bridge
            .configure_destination("inbox", Channel::Terminal)
            .unwrap();
        let expired = bridge
            .prepare(
                "inbox",
                message(Channel::Terminal),
                Duration::from_millis(1),
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            bridge.deliver(expired.id).await.unwrap().status,
            DeliveryStatus::Expired
        );
        assert!(bridge.receipt(expired.id).await.unwrap().is_none());
        let cancelled = bridge
            .prepare("inbox", message(Channel::Terminal), Duration::from_secs(60))
            .unwrap();
        bridge.cancel(cancelled.id).unwrap();
        assert!(bridge.deliver(cancelled.id).await.is_err());
        let in_flight = bridge
            .prepare("inbox", message(Channel::Terminal), Duration::from_secs(60))
            .unwrap();
        bridge
            .records
            .update("records", |r: &mut Records| {
                r.deliveries.get_mut(&in_flight.id).unwrap().status = DeliveryStatus::Dispatching;
                Ok(())
            })
            .unwrap();
        assert!(bridge
            .deliver(in_flight.id)
            .await
            .unwrap_err()
            .to_string()
            .contains("will not be repeated"));
        let changed = bridge
            .prepare("inbox", message(Channel::Terminal), Duration::from_secs(60))
            .unwrap();
        bridge
            .configure_destination(
                "inbox",
                Channel::Webhook {
                    url: "https://example.invalid/notify".into(),
                },
            )
            .unwrap();
        assert!(bridge
            .deliver(changed.id)
            .await
            .unwrap_err()
            .to_string()
            .contains("does not match"));
    }
    #[tokio::test]
    async fn approvals_are_pending_exact_expiring_and_single_use() {
        let bridge = FileCommsBridge::new(root()).unwrap();
        bridge
            .configure_destination("inbox", Channel::Terminal)
            .unwrap();
        let prompt = CommsApprovalPrompt {
            channel: Channel::Terminal,
            actor: "worker".into(),
            class: "yellow".into(),
            action: "write report.txt".into(),
            reason: "requested result".into(),
            touched_resources: vec!["report.txt".into()],
        };
        let id = bridge.request_approval(prompt.clone()).await.unwrap();
        assert_eq!(bridge.check_approval(id).await.unwrap(), None);
        assert!(!bridge.receipt(id).await.unwrap().unwrap().acknowledged);
        bridge.answer_approval(id, true).unwrap();
        assert!(bridge.answer_approval(id, false).is_err());
        let mut changed = prompt.clone();
        changed.action = "write other file".into();
        assert!(bridge.consume_approval(id, &changed).is_err());
        assert!(bridge.consume_approval(id, &prompt).unwrap());
        assert!(bridge.consume_approval(id, &prompt).is_err());
        let expired = Uuid::new_v4();
        bridge
            .prepare_approval(
                expired,
                prompt,
                Utc::now() + chrono::Duration::milliseconds(1),
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(bridge.answer_approval(expired, true).is_err());
        assert!(bridge.check_approval(expired).await.is_err());
    }
    #[test]
    fn production_rejects_http_unsupported_channels_and_copied_project() {
        let root = root();
        let bridge = FileCommsBridge::new(&root).unwrap();
        for channel in [
            Channel::Webhook {
                url: "http://127.0.0.1:9999".into(),
            },
            Channel::Webhook {
                url: "https://name:password@example.invalid".into(),
            },
            Channel::Email {
                address: "owner@example.invalid".into(),
            },
            Channel::Voice {
                number: "123".into(),
            },
        ] {
            assert!(bridge.configure_destination("bad", channel).is_err());
        }
        bridge
            .configure_destination("inbox", Channel::Terminal)
            .unwrap();
        let other = root.with_file_name(format!("chimera-comms-copy-{}", Uuid::new_v4()));
        std::fs::create_dir(&other).unwrap();
        std::fs::create_dir(other.join(".chimera")).unwrap();
        std::fs::create_dir(other.join(".chimera/comms")).unwrap();
        std::fs::copy(
            root.join(".chimera/comms/records.json"),
            other.join(".chimera/comms/records.json"),
        )
        .unwrap();
        assert!(FileCommsBridge::new(other)
            .unwrap()
            .destinations()
            .unwrap_err()
            .to_string()
            .contains("identity mismatch"));
        std::fs::write(root.join(".chimera/comms/records.json"), "broken").unwrap();
        assert!(bridge.list().unwrap_err().to_string().contains("corrupt"));
    }
}
