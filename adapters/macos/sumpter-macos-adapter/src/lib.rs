//! macOS platform adapter for the shared Sumpter engine.
//!
//! Platform control and sidecar integration stay here; all data-plane and
//! runtime logic is re-exported from the shared crates.

pub mod admin;
pub mod engine;
pub mod platform;
pub mod server;

pub mod health {
    pub use sumpter_engine::health::*;
}
pub mod outbound {
    pub use sumpter_engine::outbound::*;
}
pub mod request_build {
    pub use sumpter_engine::request_build::*;
}
pub mod runtime_query {
    pub use sumpter_engine::runtime_query::*;
}
pub mod runtime_store {
    pub use sumpter_engine::runtime_store::*;
}

pub use engine::{Engine, EngineNotice, MAX_BODY_BYTES};
pub use platform::Platform;
