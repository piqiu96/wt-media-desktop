//! Tauri command modules, grouped by the surface they drive.
//!
//! `register_commands` was a placeholder with no caller; the real registration
//! is `invoke_handler!` in `main.rs`, which still owns the command order.

pub mod account;
pub mod agent;
pub mod bind;
pub mod cleanup;
pub mod diagnostic;
pub mod downloads;
pub mod profile;
pub mod public_config;
pub mod reveal;
pub mod saved_files;
pub mod settings;
pub mod storage;
pub mod webview;
