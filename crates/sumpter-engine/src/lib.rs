//! Sumpter shared proxy engine.
//!
//! This crate owns the complete platform-neutral data plane.  Operating-system
//! behavior is supplied by an adapter through [`PlatformBoundary`]; the engine
//! itself has no Linux/macOS feature selection and can therefore be tested with
//! a fake adapter.

pub mod boundary;
pub mod health;
pub mod outbound;
pub mod replay;
pub mod request_build;

// Facade users can name the exact shared config/runtime types accepted by
// `Engine` without adding another direct dependency.
pub use sumpter_core as core;
pub use sumpter_runtime as runtime;

// Runtime remains a separate platform-neutral crate. Re-exporting its modules
// keeps the public engine facade compact while retaining one source of truth.
pub use sumpter_runtime::{runtime_query, runtime_store};

pub mod engine;

pub use boundary::{
    AccessDecision, ConfigReplacement, ControlRequestMeta, EngineCapabilities, EngineServices,
    InboundRequest, NoopPlatform, PlatformAction, PlatformBoundary, PlatformNotice,
    PlatformRequest,
};
pub use engine::{Engine, EngineNotice, MAX_BODY_BYTES};
pub use replay::{
    ReplayDifference, ReplayObservation, ReplayReply, ReplayRequest, ReplayTransport,
};
