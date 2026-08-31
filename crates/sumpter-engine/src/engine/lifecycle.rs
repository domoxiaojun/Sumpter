//! Lifecycle seam for embedders.

/// Minimal lifecycle state used by adapters and status surfaces.  Starting or
/// stopping a concrete listener remains the responsibility of the Linux/macOS
/// facade; the shared engine only exposes data-plane state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Stopped,
    Running,
}

pub trait Lifecycle: Send + Sync {
    fn lifecycle_state(&self) -> LifecycleState;
}
