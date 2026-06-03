use std::sync::{
    Arc,
    mpsc::{self, Receiver, SyncSender, TrySendError},
};

use parking_lot::Mutex;

use crate::{scope::ConsensusScope, types::ConsensusEvent};

/// Trait for broadcasting consensus events to subscribers.
///
/// Implement this to use your own event system (message queue, webhooks, etc.).
/// The default `BroadcastEventBus` fans out over standard-library channels, which
/// works well for in-process event distribution.
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

type Subscribers<Scope> = Arc<Mutex<Vec<SyncSender<(Scope, ConsensusEvent)>>>>;

/// Sends every event to all current subscribers in-process.
///
/// Each subscriber gets its own channel. If a subscriber joins late, it misses earlier events.
/// If a subscriber's buffer is full, it simply misses new events without blocking.
#[derive(Clone)]
pub struct BroadcastEventBus<Scope>
where
    Scope: ConsensusScope,
{
    capacity: usize,
    subscribers: Subscribers<Scope>,
}

impl<Scope> BroadcastEventBus<Scope>
where
    Scope: ConsensusScope,
{
    /// Create a new broadcast event bus with a custom max_queued_events size.
    ///
    /// The max_queued_events size determines how many events can be buffered per
    /// subscriber before that subscriber starts missing events. Default is 1000.
    pub fn new(max_queued_events: usize) -> Self {
        Self {
            capacity: max_queued_events,
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
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
    type Receiver = Receiver<(Scope, ConsensusEvent)>;

    fn subscribe(&self) -> Self::Receiver {
        let (sender, receiver) = mpsc::sync_channel(self.capacity);
        self.subscribers.lock().push(sender);
        receiver
    }

    fn publish(&self, scope: Scope, event: ConsensusEvent) {
        let mut subscribers = self.subscribers.lock();
        // Deliver to every live subscriber; drop senders whose receiver is gone,
        // and skip (without blocking) any subscriber whose buffer is full.
        subscribers.retain(
            |sender| match sender.try_send((scope.clone(), event.clone())) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => false,
            },
        );
    }
}
