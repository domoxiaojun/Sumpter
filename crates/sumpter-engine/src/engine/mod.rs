//! Shared engine facade. Platform services enter through `EngineServices`.
//!
//! HTTP dispatch owns a single `CompletionGuard` until response relay takes it.
//! Dropping the response body cancels that relay and records a cancelled request.

pub mod capture;
mod catalog;
mod completion;
mod context;
mod dispatch;
pub mod events;
mod failure;
pub mod forward;
mod http_relay;
mod http_response;
pub mod inbound;
pub mod lifecycle;
mod payload;
mod protocol;
mod runtime_api;
mod sessions;
mod state;
#[cfg(test)]
mod tests;
mod websocket;
mod websocket_relay;

use std::sync::Arc;

pub use capture::DEFAULT_CAPTURE_MAX_BYTES;
pub use events::EngineNotice;
pub use http_response::{error_response, json_bytes_response, json_response};
pub use inbound::MAX_BODY_BYTES;
pub use state::{EngineInner, config_generation};
pub use websocket::{PreparedWebSocket, WebSocketPrepareError, websocket_prepare_error_response};

#[cfg(test)]
#[allow(unused_imports)]
use capture::*;
#[cfg(test)]
#[allow(unused_imports)]
use catalog::*;
#[cfg(test)]
#[allow(unused_imports)]
use completion::*;
#[cfg(test)]
#[allow(unused_imports)]
use context::*;
#[cfg(test)]
#[allow(unused_imports)]
use dispatch::*;
#[cfg(test)]
#[allow(unused_imports)]
use events::*;
#[cfg(test)]
#[allow(unused_imports)]
use failure::*;
#[cfg(test)]
#[allow(unused_imports)]
use http_relay::*;
#[cfg(test)]
#[allow(unused_imports)]
use http_response::*;
#[cfg(test)]
#[allow(unused_imports)]
use inbound::*;
#[cfg(test)]
#[allow(unused_imports)]
use lifecycle::*;
#[cfg(test)]
#[allow(unused_imports)]
use payload::*;
#[cfg(test)]
#[allow(unused_imports)]
use protocol::*;
#[cfg(test)]
#[allow(unused_imports)]
use runtime_api::*;
#[cfg(test)]
#[allow(unused_imports)]
use sessions::*;
#[cfg(test)]
#[allow(unused_imports)]
use state::*;
#[cfg(test)]
#[allow(unused_imports)]
use websocket::*;
#[cfg(test)]
#[allow(unused_imports)]
use websocket_relay::*;

pub struct Engine {
    inner: Arc<EngineInner>,
}

impl Clone for Engine {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
