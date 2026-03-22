//! chimera-api: Local daemon API, IPC / Unix socket / optional HTTP bridge.
//!
//! This crate will provide the control socket and optional HTTP interface
//! for remote observers and local clients to interact with the Chimera harness.

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
