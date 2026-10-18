//! Supervisor engine of Gravekeeper.
//!
//! This crate must not depend on any GUI library. Front-ends talk to it only through
//! [`CoreHandle`] using the message types in [`api`], so the engine can later move into a
//! separate process behind local IPC without changing the front-end.

pub mod api;
pub mod config;
mod engine;
pub mod logfile;
pub mod monitor;
pub mod paths;
pub mod process;
pub mod schedule;
mod store;
pub mod supervisor;
pub mod webhook;

pub use api::{Command, Snapshot};
pub use engine::{CoreHandle, Notifier, StartError};
