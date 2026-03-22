use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

/// Unique browser session identifier.
pub type BrowserSessionId = Uuid;

// ---------------------------------------------------------------------------
// Browser target
// ---------------------------------------------------------------------------

/// Where to open a browser session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserTarget {
    /// Open a specific URL.
    Url(String),
    /// Open a named environment (resolved from config, e.g. "staging").
    Environment(String),
    /// Open a blank page.
    Blank,
}

// ---------------------------------------------------------------------------
// Browser actions
// ---------------------------------------------------------------------------

/// An action to perform in a browser session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserAction {
    /// Navigate to a URL.
    Navigate { url: String },
    /// Click an element by selector.
    Click { selector: String },
    /// Type text into an element.
    Type { selector: String, text: String },
    /// Wait for a selector to appear.
    WaitFor { selector: String, timeout_ms: u64 },
    /// Execute arbitrary JavaScript.
    EvalJs { script: String },
    /// Scroll the page.
    Scroll { x: i32, y: i32 },
    /// Take a screenshot.
    Screenshot,
    /// Read the page text content.
    ReadText { selector: Option<String> },
    /// Read console messages.
    ReadConsole,
}

// ---------------------------------------------------------------------------
// Browser artifacts
// ---------------------------------------------------------------------------

/// An artifact produced by a browser action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserArtifact {
    /// A screenshot image.
    Screenshot {
        /// Path or URI to the screenshot file.
        uri: String,
        /// Timestamp of capture.
        captured_at: DateTime<Utc>,
    },
    /// Extracted text content.
    Text {
        content: String,
    },
    /// Console log output.
    Console {
        messages: Vec<ConsoleMessage>,
    },
    /// JavaScript evaluation result.
    JsResult {
        value: serde_json::Value,
    },
    /// Action completed with no specific artifact.
    Ack,
}

/// A browser console message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleMessage {
    /// Log level (log, warn, error, info, debug).
    pub level: String,
    /// Message text.
    pub text: String,
    /// Timestamp.
    pub at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Browser session state
// ---------------------------------------------------------------------------

/// State of a browser session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserState {
    /// Session ID.
    pub id: BrowserSessionId,
    /// Current URL.
    pub current_url: String,
    /// Page title.
    pub title: String,
    /// Whether the session is still active.
    pub active: bool,
    /// When the session was opened.
    pub opened_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Evidence reference (browser-specific)
// ---------------------------------------------------------------------------

/// A reference to evidence captured by the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserEvidence {
    /// Kind label (e.g. "screenshot", "console", "dom_snapshot").
    pub kind: String,
    /// URI or path to the artifact.
    pub uri: String,
    /// Human-readable label.
    pub label: String,
    /// When captured.
    pub captured_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// BrowserController trait
// ---------------------------------------------------------------------------

/// Controls browser sessions for automation, screenshots, and evidence capture.
#[async_trait::async_trait]
pub trait BrowserController: Send + Sync {
    /// Open a new browser session targeting a URL or environment.
    async fn open(&self, target: BrowserTarget) -> anyhow::Result<BrowserSessionId>;

    /// Perform an action in a browser session.
    async fn act(
        &self,
        session: BrowserSessionId,
        action: BrowserAction,
    ) -> anyhow::Result<BrowserArtifact>;

    /// Take a screenshot and return an evidence reference.
    async fn screenshot(&self, session: BrowserSessionId) -> anyhow::Result<BrowserEvidence>;

    /// Get the current state of a browser session.
    async fn state(&self, session: BrowserSessionId) -> anyhow::Result<BrowserState>;

    /// Close a browser session.
    async fn close(&self, session: BrowserSessionId) -> anyhow::Result<()>;

    /// List all active browser session IDs.
    async fn list_sessions(&self) -> anyhow::Result<Vec<BrowserSessionId>>;
}
