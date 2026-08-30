use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Delivery class. Urgency rations *derailment* — it is delivery timing,
/// nothing else, and it never reorders (delivery always carries everything
/// pending, in send order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    /// Interrupt a turn in progress.
    Gating,
    /// Wait a turn out; wakes an idle recipient immediately.
    Normal,
    /// Ambient fact. Rides along with other deliveries or the next turn
    /// boundary; never wakes anyone, never interrupts anything.
    Notice,
}

/// One message on the bus. The store adds delivery bookkeeping
/// (delivered_at, delivered_via, repo commit at delivery); there are no
/// acks, receipts, or read-markers by design — the trail memorializes
/// passively, and a sender who needs confirmation asks in the message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusMessage {
    pub id: Uuid,
    pub from: String,
    /// `@agent`, `@operator`, or `#channel`.
    pub to: String,
    pub urgency: Urgency,
    pub body: String,
    /// Optional durable-record reference (issue id, doc path). The bus is
    /// the notification; the record is where a ruling lives.
    pub record: Option<String>,
    pub thread: Option<String>,
    pub created_at_epoch_ms: u64,
}
