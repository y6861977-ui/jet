//! The jet library — the core of the package manager in Rust (no external crates).
//!
//! Modules are declared here so they can be reused by both the `jet` binary (CLI)
//! and the separate `jet-server` crate (a private npm registry). Exposed outward
//! are mainly the building blocks useful to the server: `semver`, `targz`, `json`,
//! `hash`, `registry`, `mirrors`, `spec`, `types`.

// Some internal APIs are covered by tests but not called from every consumer.
#![allow(dead_code)]

pub mod audit;
pub mod audit_cli;
pub mod cache;
pub mod cli;
pub mod complete;
pub mod config;
pub mod dashboard;
pub mod db;
#[cfg(feature = "pro")]
pub mod explain;
pub mod global;
pub mod hash;
pub mod history;
pub mod installer;
pub mod json;
#[cfg(feature = "pro")]
pub mod license;
pub mod lockfile;
pub mod lockresolve;
pub mod mirrors;
pub mod platform;
pub mod project;
pub mod registry;
pub mod resolver;
pub mod runner;
pub mod self_uninstall;
pub mod semver;
pub mod spec;
pub mod storage;
pub mod store;
pub mod targz;
pub mod templates;
pub mod types;
pub mod ui;
