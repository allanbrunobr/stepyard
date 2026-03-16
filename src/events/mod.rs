pub mod subscribers;
pub mod types;

use tokio::sync::broadcast;

pub use types::Event;

/// Capacity of the broadcast channel (number of events buffered before slow
/// receivers start lagging)
const CHANNEL_CAPACITY: usize = 256;

/// Trait for synchronous event subscribers that receive a callback on each event.
pub trait EventSubscriber: Send + Sync {
    fn on_event(&self, event: &Event);
}

/// Central event bus used by the engine to publish lifecycle events.
///
/// Internally uses a `tokio::sync::broadcast` channel so that multiple
/// independent async receivers can each consume every event.
pub struct EventBus {
    sender: broadcast::Sender<Event>,
    subscribers: Vec<Box<dyn EventSubscriber + Send + Sync>>,
}

impl EventBus {
    /// Create a new EventBus with an empty subscriber list.
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            sender,
            subscribers: Vec::new(),
        }
    }

    /// Subscribe to the broadcast channel and receive a `Receiver` handle.
    /// Multiple handles can be created; each will receive every future event.
    #[allow(dead_code)]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    /// Emit an event to all broadcast receivers and all registered subscribers.
    /// If there are no broadcast receivers, the send is silently dropped.
    pub async fn emit(&self, event: Event) {
        // Notify synchronous subscribers
        for sub in &self.subscribers {
            sub.on_event(&event);
        }

        // Broadcast to async receivers; ignore errors (no receivers = ok)
        let _ = self.sender.send(event);
    }

    /// Register a synchronous subscriber that will be called for every event.
    pub fn add_subscriber(&mut self, subscriber: Box<dyn EventSubscriber + Send + Sync>) {
        self.subscribers.push(subscriber);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
