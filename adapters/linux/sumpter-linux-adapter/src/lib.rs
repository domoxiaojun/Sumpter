//! Linux platform adapter for the shared Sumpter engine.
//!
//! Only Linux control-plane behavior and the Admin facade live here. The
//! request pipeline, protocol bridge, runtime store, and health model are
//! re-exported from the shared crates so there is one implementation.

pub mod admin;
mod admin_auth;
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
