//! The permission broker seam lives in aspen-core (harness-neutral); this
//! module re-exports it so the crate's callers keep their paths.

pub use aspen_core::permission::{
    BrokerDecision, DecidedBy, PermissionBroker, PermissionRequest, PolicyBroker,
};
