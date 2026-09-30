//! `xhl-core` — libxhl.
//!
//! Domain, driver, store, dan transport untuk mengendalikan akun X dari AI agent.
//! Crate ini **tidak tahu** apa itu CLI atau MCP: adapter (`xhl-cli`, `xhl-mcp`)
//! menerjemahkan tipe domain ke protokol masing-masing.

pub mod antibot;
pub mod config;
pub mod domain;
pub mod driver;
pub mod error;
pub mod http;
pub mod limiter;
pub mod llm;
pub mod native;
pub mod query;
pub mod service;
pub mod session;
pub mod store;

pub use config::Config;
pub use error::XhlError;
