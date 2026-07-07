//! Library surface for skillscope, split out of `main.rs` so both the
//! `skillscope` binary and the `tests/` integration suite can share the same
//! parsing/aggregation/fidelity code without duplicating module wiring.

pub mod aggregate;
pub mod cli;
pub mod fidelity;
pub mod fzf;
pub mod inventory;
pub mod models;
pub mod parser;
pub mod report;
pub mod resolve;
pub mod sessions;
pub mod sessionscan;
pub mod tui;
