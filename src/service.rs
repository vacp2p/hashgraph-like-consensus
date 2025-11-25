use std::{
    marker::PhantomData,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::time::{Duration, sleep};
use tracing::info;

use crate::{
    error::ConsensusError,
    events::{BroadcastEventBus, ConsensusEventBus},
    protos::consensus::v1::Proposal,
    scope::{ConsensusScope, GroupId},
    session::{ConsensusEvent, ConsensusSession, ConsensusState, ConsensusTransition},
    storage::{ConsensusStorage, InMemoryConsensusStorage},
    utils::{calculate_consensus_result, check_sufficient_votes},
};
/// Consensus service that manages multiple consensus sessions, optionally namespaced by scope.
pub struct ConsensusService<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    storage: Arc<S>,
    max_sessions_per_scope: usize,
    event_bus: E,
    _scope: PhantomData<Scope>,
}

impl<Scope, S, E> Clone for ConsensusService<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    fn clone(&self) -> Self {
        Self {
            storage: Arc::clone(&self.storage),
            max_sessions_per_scope: self.max_sessions_per_scope,
            event_bus: self.event_bus.clone(),
            _scope: PhantomData,
        }
    }
}

/// Default consensus service type using in-memory storage and tokio broadcast events.
pub type DefaultConsensusService =
    ConsensusService<GroupId, InMemoryConsensusStorage<GroupId>, BroadcastEventBus<GroupId>>;

impl DefaultConsensusService {
    fn new() -> Self {
        Self::new_with_max_sessions(10)
    }

    pub fn new_with_max_sessions(max_sessions_per_scope: usize) -> Self {
        Self::new_with_components(
            Arc::new(InMemoryConsensusStorage::new()),
            BroadcastEventBus::default(),
            max_sessions_per_scope,
        )
    }
}

impl Default for DefaultConsensusService {
    fn default() -> Self {
        Self::new()
    }
}

impl<Scope, S, E> ConsensusService<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    pub fn new_with_components(
        storage: Arc<S>,
        event_bus: E,
        max_sessions_per_scope: usize,
    ) -> Self {
        Self {
            storage,
            max_sessions_per_scope,
            event_bus,
            _scope: PhantomData,
        }
    }

    pub fn subscribe_to_events(&self) -> E::Receiver {
        self.event_bus.subscribe()
    }

    fn emit_event(&self, scope: &Scope, event: ConsensusEvent) {
        self.event_bus.publish(scope.clone(), event);
    }

    pub(crate) fn handle_transition(
        &self,
        scope: &Scope,
        proposal_id: u32,
        transition: ConsensusTransition,
    ) {
        if let ConsensusTransition::ConsensusReached(result) = transition {
            self.emit_event(
                scope,
                ConsensusEvent::ConsensusReached {
                    proposal_id,
                    result,
                },
            );
        }
    }

    pub(crate) async fn update_session<R, F>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        mutator: F,
    ) -> Result<R, ConsensusError>
    where
        F: FnOnce(&mut ConsensusSession) -> Result<R, ConsensusError>,
    {
        let mut session = self
            .storage
            .get_session(scope, proposal_id)
            .await?
            .ok_or(ConsensusError::SessionNotFound)?;
        let result = mutator(&mut session)?;
        self.storage.save_session(scope, session).await?;
        Ok(result)
    }

    pub(crate) async fn save_session(
        &self,
        scope: &Scope,
        session: ConsensusSession,
    ) -> Result<(), ConsensusError> {
        self.storage.save_session(scope, session).await
    }

    pub(crate) async fn get_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<ConsensusSession, ConsensusError> {
        self.storage
            .get_session(scope, proposal_id)
            .await?
            .ok_or(ConsensusError::SessionNotFound)
    }

    pub(crate) async fn enforce_scope_limit(&self, scope: &Scope) -> Result<(), ConsensusError> {
        let mut sessions = self.storage.list_scope_sessions(scope).await?;
        if sessions.len() <= self.max_sessions_per_scope {
            return Ok(());
        }

        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        sessions.truncate(self.max_sessions_per_scope);
        self.storage.replace_scope_sessions(scope, sessions).await?;
        Ok(())
    }

    pub(crate) async fn list_scope_sessions(
        &self,
        scope: &Scope,
    ) -> Result<Vec<ConsensusSession>, ConsensusError> {
        self.storage.list_scope_sessions(scope).await
    }

    pub(crate) fn spawn_timeout_task(&self, scope: Scope, proposal_id: u32, timeout_seconds: u64) {
        let service = self.clone();
        Self::spawn_timeout_task_owned(service, scope, proposal_id, timeout_seconds);
    }

    fn spawn_timeout_task_owned(
        service: ConsensusService<Scope, S, E>,
        scope: Scope,
        proposal_id: u32,
        timeout_seconds: u64,
    ) {
        tokio::spawn(async move {
            sleep(Duration::from_secs(timeout_seconds)).await;

            if service
                .get_consensus_result(&scope, proposal_id)
                .await
                .is_some()
            {
                return;
            }

            if let Ok(result) = service.handle_consensus_timeout(&scope, proposal_id).await {
                info!(
                    "Automatic timeout applied for proposal {proposal_id} in scope {scope:?} after {timeout_seconds}s => {result}"
                );
            }
        });
    }

    pub async fn set_consensus_threshold_for_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
        consensus_threshold: f64,
    ) -> Result<(), ConsensusError> {
        self.update_session(scope, proposal_id, |session| {
            session.set_consensus_threshold(consensus_threshold);
            Ok(())
        })
        .await
    }

    pub async fn get_proposal_liveness_criteria(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Option<bool> {
        self.storage
            .get_session(scope, proposal_id)
            .await
            .ok()
            .flatten()
            .map(|session| session.proposal.liveness_criteria_yes)
    }

    pub async fn get_consensus_result(&self, scope: &Scope, proposal_id: u32) -> Option<bool> {
        self.storage
            .get_session(scope, proposal_id)
            .await
            .ok()
            .flatten()
            .and_then(|session| match session.state {
                ConsensusState::ConsensusReached(result) => Some(result),
                _ => None,
            })
    }

    pub async fn get_active_proposals(&self, scope: &Scope) -> Vec<Proposal> {
        self.storage
            .list_scope_sessions(scope)
            .await
            .map(|sessions| {
                sessions
                    .into_iter()
                    .filter(|session| session.is_active())
                    .map(|session| session.proposal)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn cleanup_expired_sessions(&self) {
        if let Ok(scopes) = self.storage.list_scopes().await {
            for scope in scopes {
                if let Ok(mut sessions) = self.storage.list_scope_sessions(&scope).await {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("Failed to get current time")
                        .as_secs();
                    sessions.retain(|session| {
                        now <= session.proposal.expiration_time && session.is_active()
                    });
                    if let Err(err) = self.storage.replace_scope_sessions(&scope, sessions).await {
                        tracing::error!("Failed to cleanup sessions for scope {scope:?}: {err:?}");
                    }
                }
            }
        }
    }

    pub async fn handle_consensus_timeout(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<bool, ConsensusError> {
        let mut session = self
            .storage
            .get_session(scope, proposal_id)
            .await?
            .ok_or(ConsensusError::SessionNotFound)?;

        if let ConsensusState::ConsensusReached(result) = session.state {
            return Ok(result);
        }

        let total_votes = session.votes.len() as u32;
        let expected_voters = session.proposal.expected_voters_count;
        let result = if check_sufficient_votes(
            total_votes,
            expected_voters,
            session.config.consensus_threshold,
        ) {
            calculate_consensus_result(&session.votes, session.proposal.liveness_criteria_yes)
        } else {
            session.proposal.liveness_criteria_yes
        };

        session.state = ConsensusState::ConsensusReached(result);
        self.storage.save_session(scope, session).await?;

        self.emit_event(
            scope,
            ConsensusEvent::ConsensusReached {
                proposal_id,
                result,
            },
        );

        Ok(result)
    }
}
