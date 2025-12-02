use tokio::sync::broadcast;

use crate::{scope::ConsensusScope, session::ConsensusEvent};

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
    pub fn new(buffer: usize) -> Self {
        let (sender, _) = broadcast::channel(buffer);
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
