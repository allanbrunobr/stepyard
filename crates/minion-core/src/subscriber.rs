//! The [`EventSubscriber`] trait — the universal sink for engine events.
//!
//! Implementors live wherever they need to (file, webhook, dashboard, etc).
//! The contract is intentionally synchronous: the engine fans out via this
//! trait inside its event bus, and IO-bound subscribers spawn their own
//! tokio tasks (see how `WebhookSubscriber` and `DashboardSubscriber` do
//! fire-and-forget HTTP).

use crate::event::Event;

/// Anything that wants to observe engine lifecycle events implements this.
///
/// The `on_event` call must NOT block the engine. If the subscriber needs to
/// do IO, it should spawn a task and return immediately.
pub trait EventSubscriber: Send + Sync {
    /// Called for every event emitted by the engine.
    fn on_event(&self, event: &Event);
}
