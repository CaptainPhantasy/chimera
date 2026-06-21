//! chimera-tui — A competitive terminal UI for the CHIMERA agentic coding harness.
//!
//! Features:
//! - Theme system with dark/light palettes and reusable styles
//! - Streaming chat view with markdown-style rendering
//! - Tool-call cards with expand/collapse
//! - Status bar (model, tokens, cost, iteration)
//! - Multi-line input bar with slash-command autocomplete
//! - Help overlay with keyboard shortcuts
//! - Full event loop with raw-mode terminal lifecycle

pub mod app;
pub mod chat;
pub mod help;
pub mod input;
pub mod message;
pub mod runner;
pub mod status;
pub mod theme;
pub mod views;
