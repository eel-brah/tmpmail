//! tmpmail's core: providers, the stored inboxes, polling and mail
//! sanitizing. The `tmpmail` binary (feature `cli`, on by default) adds the
//! command line, the TUI and the MCP server; the desktop app uses the core
//! without them.

// Without the CLI some helpers have no caller; the full build checks them.
#![cfg_attr(not(feature = "cli"), allow(dead_code, unused_imports))]

pub mod app;
#[cfg(feature = "cli")]
pub mod cli;
pub mod config;
pub mod error;
mod extract;
mod http;
#[cfg(feature = "cli")]
mod mcp;
pub mod provider;
pub mod push;
pub mod render;
pub mod store;
mod system;
#[cfg(feature = "cli")]
mod tui;
