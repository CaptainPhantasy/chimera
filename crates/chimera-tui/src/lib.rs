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
mod connect;
pub mod dialog;
mod external;
pub mod help;
pub mod input;
mod jobs;
mod memory;
pub mod message;
mod models;
mod permissions;
mod rewind;
pub mod runner;
mod sessions;
mod skills;
pub mod status;
pub mod theme;
pub mod views;
