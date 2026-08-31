//! Diagnostic-capture seam.
//!
//! Full capture storage stays private to the engine.  Only bounded constants
//! and the public snapshot type are exposed so a facade cannot accidentally
//! widen the raw-diagnostic surface.

pub use sumpter_core::events::{DiagnosticCaptureSnapshot, DiagnosticRequestCapture};

pub const DEFAULT_MAX_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_INDEX_RECORDS: usize = 200;
