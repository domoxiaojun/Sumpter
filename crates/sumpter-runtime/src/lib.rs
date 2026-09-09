//! Shared SQLite runtime projection and query layer.
//!
//! This crate is intentionally platform-neutral.  The public types and wire
//! serialization are shared by the Linux daemon and the macOS sidecar.

pub mod runtime_query;
pub mod runtime_store;

mod database;
mod entities;

pub use runtime_query::*;
pub use runtime_store::*;
