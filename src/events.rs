use tokio::sync::broadcast;

use crate::{scope::ConsensusScope, session::ConsensusEvent};

pub trait ConsensusEventBus<Scope>: Clone + Send + Sync + 'static
where
    Scope: ConsensusScope,
{
    /// Type returned to consumers that subscribe to consensus events.
    type Receiver;

    fn subscribe(&self) -> Self::Receiver;
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
