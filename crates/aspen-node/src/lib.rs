//! aspen-node — the node layer: bus store, session manager, delivery engine,
//! and the bus tools served to agents over the in-process MCP path.
//!
//! Bus semantics are plumb's, upgraded by pipe ownership (see
//! `docs/DESIGN.md` §4.2/§6): three delivery classes, send-order delivery,
//! no acks (the trail memorializes passively; the runtime's replay ack gives
//! proof of ingestion), and delivery notes at send time instead of silent
//! downgrades.

pub mod delivery;
pub mod node;
pub mod federation;
pub mod mesh;
pub mod permit;
pub mod skills;
pub mod store;
pub mod tools;

pub use node::{Node, SpawnOpts, TurnState};
pub use store::{BusStore, StoredMessage};
