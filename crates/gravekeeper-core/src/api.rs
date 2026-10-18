//! Messages exchanged between the engine and its front-ends.
//!
//! Everything here is serializable so the same types can become the IPC protocol once the
//! engine runs as its own process (Phase 4).

use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// A request from a front-end to the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Stop all managed work and terminate the engine thread.
    Shutdown,
}

/// A read-only view of the engine state, cheap enough to copy every frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub started_at: SystemTime,
    pub uptime: Duration,
}
