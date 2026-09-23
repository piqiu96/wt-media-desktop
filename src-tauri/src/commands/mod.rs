//! Tauri command modules, grouped by the surface they drive.
//!
//! `register_commands` was a placeholder with no caller; the real registration
//! is `invoke_handler!` in `main.rs`, which still owns the command order.

pub mod account;
pub mod agent;
pub mod bind;
pub mod logging;
pub mod profile;
pub mod public_config;
