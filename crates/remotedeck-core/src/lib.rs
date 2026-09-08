//! Renderer-independent RemoteDeck business functionality.
//!
//! This crate owns persistence, OpenSSH integration, jobs, terminals,
//! transfers, telemetry, tunnels and migration logic. HTTP, browser lifecycle
//! and native dialogs belong to `remotedeck-server`.

pub mod agent;
pub mod agent_service;
pub mod command_job;
pub mod diagnostics;
pub mod error;
pub mod events;
pub mod host_operation;
pub mod key_service;
pub mod keys;
pub mod migration;
pub mod model;
#[cfg(all(test, feature = "openssh-integration"))]
mod openssh_integration;
pub mod process;
pub mod risk;
pub mod session;
pub mod ssh;
pub mod store;
pub mod telemetry;
pub mod tunnel;

pub use error::{AppError, AppResult};
pub use events::EventSink;
