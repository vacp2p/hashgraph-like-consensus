use crate::{
    events::ConsensusEventBus, scope::ConsensusScope, service::ConsensusService,
    session::ConsensusState, storage::ConsensusStorage,
};

#[derive(Debug, Clone)]
pub struct ConsensusStats {
    /// Total number of proposals in this scope.
    pub total_sessions: usize,
    /// How many proposals are still accepting votes.
    pub active_sessions: usize,
    /// How many proposals failed to reach consensus (timeout with insufficient votes).
    pub failed_sessions: usize,
    /// How many proposals successfully reached consensus.
    pub consensus_reached: usize,
}

impl<Scope, S, E> ConsensusService<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    /// Get statistics about proposals in a scope.
    ///
    /// Returns counts of total, active, failed, and finalized proposals.
    /// Useful for monitoring and dashboards.
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
                let failed_sessions = scope_sessions
                    .iter()
                    .filter(|s| matches!(s.state, ConsensusState::Failed))
                    .count();

                ConsensusStats {
                    total_sessions,
                    active_sessions,
                    consensus_reached,
                    failed_sessions,
                }
            })
            .unwrap_or(ConsensusStats {
                total_sessions: 0,
                active_sessions: 0,
                consensus_reached: 0,
                failed_sessions: 0,
            })
    }
}
