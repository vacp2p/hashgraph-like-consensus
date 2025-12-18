use tokio::sync::broadcast;

use crate::{scope::ConsensusScope, types::ConsensusEvent};

/// Trait for broadcasting consensus events to subscribers.
///
/// Implement this to use your own event system (message queue, webhooks, etc.).
/// The default `BroadcastEventBus` uses Tokio's broadcast channel, which works
/// well for in-process event distribution.
pub trait ConsensusEventBus<Scope>: Clone + Send + Sync + 'static
where
    Scope: ConsensusScope,
{
    /// The type returned when subscribing to events.
    type Receiver;

    /// Subscribe to receive consensus events from all scopes.
    fn subscribe(&self) -> Self::Receiver;
    /// Publish an event for a specific scope.
    fn publish(&self, scope: Scope, event: ConsensusEvent);
}

/// Default event bus implementation using Tokio's broadcast channel.
///
/// This broadcasts events to all subscribers within the same process. Events are sent
/// to all active subscribers, and late subscribers miss events that occurred before
/// they subscribed. Perfect for in-process event distribution.
#[derive(Clone)]
pub struct BroadcastEventBus<Scope>
where
    Scope: ConsensusScope,
{
    sender: broadcast::Sender<(Scope, ConsensusEvent)>,
}

impl<Scope> BroadcastEventBus<Scope>
where
    Scope: ConsensusScope,
{
    /// Create a new broadcast event bus with a custom max_queued_events size.
    ///
    /// The max_queued_events size determines how many events can be queued before subscribers
    /// start missing events. Default is 1000.
    pub fn new(max_queued_events: usize) -> Self {
        let (sender, _) = broadcast::channel(max_queued_events);
        Self { sender }
    }
}

impl<Scope> Default for BroadcastEventBus<Scope>
where
    Scope: ConsensusScope,
{
    fn default() -> Self {
        Self::new(1000)
    }
}

impl<Scope> ConsensusEventBus<Scope> for BroadcastEventBus<Scope>
where
    Scope: ConsensusScope,
{
    type Receiver = broadcast::Receiver<(Scope, ConsensusEvent)>;

    fn subscribe(&self) -> Self::Receiver {
        self.sender.subscribe()
    }

    fn publish(&self, scope: Scope, event: ConsensusEvent) {
        let _ = self.sender.send((scope, event));
    }
}
