use crate::{
    events::ConsensusEventBus, scope::ConsensusScope, service::ConsensusService,
    session::ConsensusState, storage::ConsensusStorage,
};

/// Statistics about consensus sessions within a scope.
#[derive(Debug, Clone)]
pub struct ConsensusStats {
    pub total_sessions: usize,
    pub active_sessions: usize,
    pub consensus_reached: usize,
}

impl<Scope, S, E> ConsensusService<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    pub async fn get_scope_stats(&self, scope: &Scope) -> ConsensusStats {
        self.list_scope_sessions(scope)
            .await
            .map(|scope_sessions| {
                let total_sessions = scope_sessions.len();
                let active_sessions = scope_sessions.iter().filter(|s| s.is_active()).count();
                let consensus_reached = scope_sessions
                    .iter()
                    .filter(|s| matches!(s.state, ConsensusState::ConsensusReached(_)))
                    .count();

                ConsensusStats {
                    total_sessions,
                    active_sessions,
                    consensus_reached,
                }
            })
            .unwrap_or(ConsensusStats {
                total_sessions: 0,
                active_sessions: 0,
                consensus_reached: 0,
            })
    }
}
