//! Supervisor engine of Gravekeeper.
//!
//! This crate must not depend on any GUI library. Front-ends talk to it only through
//! [`CoreHandle`] using the message types in [`api`], so the engine can later move into a
//! separate process behind local IPC without changing the front-end.

pub mod api;
mod engine;
pub mod process;

pub use api::{Command, Snapshot};
pub use engine::{CoreHandle, Notifier};
