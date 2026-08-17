//! Library surface for searchlight_cli: the .dta reader, data model, formatters, expression
//! evaluator, command parser, and command implementations. The `main.rs` binary is a thin CLI
//! wrapper over these modules; exposing them as a library also lets the integration tests in
//! `tests/` exercise the engine directly.

pub mod commands;
pub mod expr;
pub mod export;
pub mod format;
pub mod json;
pub mod model;
pub mod parser;
pub mod reader;
