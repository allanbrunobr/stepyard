// `#[async_trait]` marks generated trait methods `#[must_use]` while they
// already return a must-use future; newer clippy flags that as double_must_use.
#![allow(clippy::double_must_use)]

pub mod chat_client;
pub mod claude;
pub mod cli;
pub mod config;
pub mod control_flow;
pub mod display;
pub mod engine;
pub mod error;
pub mod events;
pub mod github_repo;
pub mod plugins;
pub mod prompts;
pub mod sandbox;
pub mod signal;
#[cfg(feature = "slack")]
pub mod slack;
pub mod startup;
pub mod steps;
pub mod workflow;
