//! Upstream forwarding seam.
//!
//! Keeping the transport trait in one public module makes it possible for a
//! replay harness to inject a deterministic mock without reaching into the
//! platform facades or the concrete reqwest client.

pub use crate::outbound::{
    OutboundRequest, ReqwestTransport, ResolvedTarget, TransportError, UpstreamResponse,
    UpstreamTransport, join_paths, resolve_target,
};
