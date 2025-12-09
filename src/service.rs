use std::{collections::HashMap, marker::PhantomData};
use tokio::time::{Duration, sleep};
use tracing::info;

use crate::{
    error::ConsensusError,
    events::{BroadcastEventBus, ConsensusEventBus},
    protos::consensus::v1::Proposal,
    scope::{ConsensusScope, ScopeID},
    scope_config::{NetworkType, ScopeConfig, ScopeConfigBuilder},
    session::{ConsensusConfig, ConsensusSession, ConsensusState},
    storage::{ConsensusStorage, InMemoryConsensusStorage},
    types::{ConsensusEvent, SessionTransition},
    utils::{calculate_consensus_result, has_sufficient_votes},
};
/// The main service that handles proposals, votes, and consensus.
///
/// This is the main entry point for using the consensus service.
/// It handles creating proposals, processing votes, and managing timeouts.
pub struct ConsensusService<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    storage: S,
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
            storage: self.storage.clone(),
            max_sessions_per_scope: self.max_sessions_per_scope,
            event_bus: self.event_bus.clone(),
            _scope: PhantomData,
        }
    }
}

/// A ready-to-use service with in-memory storage and broadcast events.
///
/// This is the easiest way to get started. It stores everything in memory (great for
/// testing or single-node setups) and uses a simple broadcast channel for events.
/// If you need persistence or custom event handling, use `ConsensusService` directly.
pub type DefaultConsensusService =
    ConsensusService<ScopeID, InMemoryConsensusStorage<ScopeID>, BroadcastEventBus<ScopeID>>;

impl DefaultConsensusService {
    /// Create a service with default settings (10 max sessions per scope).
    fn new() -> Self {
        Self::new_with_max_sessions(10)
    }

    /// Create a service with a custom limit on how many sessions can exist per scope.
    ///
    /// When the limit is reached, older sessions are automatically removed to make room.
    pub fn new_with_max_sessions(max_sessions_per_scope: usize) -> Self {
        Self::new_with_components(
            InMemoryConsensusStorage::new(),
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
    /// Build a service with your own storage and event bus implementations.
    ///
    /// Use this when you need custom persistence (like a database) or event handling.
    /// The `max_sessions_per_scope` parameter controls how many sessions can exist per scope.
    /// When the limit is reached, older sessions are automatically removed.
    pub fn new_with_components(storage: S, event_bus: E, max_sessions_per_scope: usize) -> Self {
        Self {
            storage,
            max_sessions_per_scope,
            event_bus,
            _scope: PhantomData,
        }
    }

    /// Subscribe to events like consensus reached or consensus failed.
    ///
    /// Returns a receiver that you can use to listen for events across all scopes.
    /// Events are broadcast to all subscribers, so multiple parts of your application
    /// can react to consensus outcomes.
    pub fn subscribe_to_events(&self) -> E::Receiver {
        self.event_bus.subscribe()
    }

    /// Get the final consensus result for a proposal, if it's been reached.
    ///
    /// Returns `Ok(true)` if consensus was YES, `Ok(false)` if NO, or `Err` if
    /// consensus hasn't been reached yet (or the proposal doesn't exist or is still active).
    pub async fn get_consensus_result(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<bool, ConsensusError> {
        let session = self
            .storage
            .get_session(scope, proposal_id)
            .await?
            .ok_or(ConsensusError::SessionNotFound)?;

        match session.state {
            ConsensusState::ConsensusReached(result) => Ok(result),
            ConsensusState::Failed => Err(ConsensusError::ConsensusFailed),
            ConsensusState::Active => Err(ConsensusError::ConsensusNotReached),
        }
    }

    /// Get all proposals that are still accepting votes.
    pub async fn get_active_proposals(
        &self,
        scope: &Scope,
    ) -> Result<Option<Vec<Proposal>>, ConsensusError> {
        let sessions = self
            .storage
            .list_scope_sessions(scope)
            .await?
            .ok_or(ConsensusError::ScopeNotFound)?;
        let result = sessions
            .into_iter()
            .filter_map(|session| session.is_active().then_some(session.proposal))
            .collect::<Vec<Proposal>>();
        if result.is_empty() {
            return Ok(None);
        }
        Ok(Some(result))
    }

    /// Get all proposals that have reached consensus, along with their results.
    ///
    /// Returns a map from proposal ID to result (`true` for YES, `false` for NO).
    /// Only includes proposals that have finalized - active proposals are not included.
    /// Returns `None` if no proposals have reached consensus.
    pub async fn get_reached_proposals(
        &self,
        scope: &Scope,
    ) -> Result<Option<HashMap<u32, bool>>, ConsensusError> {
        let sessions = self
            .storage
            .list_scope_sessions(scope)
            .await?
            .ok_or(ConsensusError::ScopeNotFound)?;

        let result = sessions
            .into_iter()
            .filter_map(|session| {
                session
                    .get_consensus_result()
                    .ok()
                    .map(|result| (session.proposal.proposal_id, result))
            })
            .collect::<HashMap<u32, bool>>();
        if result.is_empty() {
            return Ok(None);
        }
        Ok(Some(result))
    }

    /// Check if a proposal has collected enough votes to reach consensus.
    pub async fn has_sufficient_votes_for_proposal(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<bool, ConsensusError> {
        let session = self.get_session(scope, proposal_id).await?;
        let total_votes = session.votes.len() as u32;
        let expected_voters = session.proposal.expected_voters_count;
        Ok(has_sufficient_votes(
            total_votes,
            expected_voters,
            session.config.consensus_threshold(),
        ))
    }

    // Scope management methods

    /// Get a builder for a scope configuration.
    ///
    /// # Example
    /// ```rust,no_run
    /// use hashgraph_like_consensus::{scope_config::NetworkType, scope::ScopeID, service::DefaultConsensusService};
    /// use std::time::Duration;
    ///
    /// async fn example() -> Result<(), Box<dyn std::error::Error>> {
    ///   let service = DefaultConsensusService::default();
    ///   let scope = ScopeID::from("my_scope");
    ///
    ///   // Initialize new scope
    ///   service
    ///     .scope(&scope)
    ///     .await?
    ///     .with_network_type(NetworkType::P2P)
    ///     .with_threshold(0.75)
    ///     .with_timeout(Duration::from_secs(120))
    ///     .initialize()
    ///     .await?;
    ///
    ///   // Update existing scope (single field)
    ///   service
    ///     .scope(&scope)
    ///     .await?
    ///     .with_threshold(0.8)
    ///     .update()
    ///     .await?;
    ///   Ok(())
    /// }
    /// ```
    pub async fn scope(
        &self,
        scope: &Scope,
    ) -> Result<ScopeConfigBuilderWrapper<Scope, S, E>, ConsensusError> {
        let existing_config = self.storage.get_scope_config(scope).await?;
        let builder = if let Some(config) = existing_config {
            ScopeConfigBuilder::from_existing(config)
        } else {
            ScopeConfigBuilder::new()
        };
        Ok(ScopeConfigBuilderWrapper::new(
            self.clone(),
            scope.clone(),
            builder,
        ))
    }

    async fn initialize_scope(
        &self,
        scope: &Scope,
        config: ScopeConfig,
    ) -> Result<(), ConsensusError> {
        config.validate()?;
        self.storage.set_scope_config(scope, config).await
    }

    async fn update_scope_config<F>(&self, scope: &Scope, updater: F) -> Result<(), ConsensusError>
    where
        F: FnOnce(&mut ScopeConfig) -> Result<(), ConsensusError> + Send,
    {
        self.storage.update_scope_config(scope, updater).await
    }

    /// Resolve configuration for a proposal.
    ///
    /// Priority: proposal override > proposal fields (expiration_timestamp, liveness_criteria_yes)
    ///   > scope config > global default
    pub(crate) async fn resolve_config(
        &self,
        scope: &Scope,
        proposal_override: Option<ConsensusConfig>,
        proposal: Option<&Proposal>,
    ) -> Result<ConsensusConfig, ConsensusError> {
        // 1. If explicit config override exists, use it as base
        let base_config = if let Some(override_config) = proposal_override {
            override_config
        } else if let Some(scope_config) = self.storage.get_scope_config(scope).await? {
            ConsensusConfig::from(scope_config)
        } else {
            ConsensusConfig::gossipsub()
        };

        // 2. Apply proposal field overrides if proposal is provided
        if let Some(prop) = proposal {
            // Calculate timeout from expiration_timestamp (absolute timestamp) - timestamp (creation time)
            let timeout_seconds = if prop.expiration_timestamp > prop.timestamp {
                Duration::from_secs(prop.expiration_timestamp - prop.timestamp)
            } else {
                base_config.consensus_timeout()
            };

            Ok(ConsensusConfig::new(
                base_config.consensus_threshold(),
                timeout_seconds,
                base_config.max_rounds(),
                base_config.use_gossipsub_rounds(),
                prop.liveness_criteria_yes,
            ))
        } else {
            Ok(base_config)
        }
    }

    /// Handle the timeout for a proposal.
    ///
    /// First checks if consensus has already been reached and returns the result if so.
    /// Otherwise, calculates consensus from current votes. If consensus is reached, marks
    /// the session as ConsensusReached and returns the result. If no consensus, marks the
    /// session as Failed and returns an error.
    pub async fn handle_consensus_timeout(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<bool, ConsensusError> {
        let timeout_result: Result<Option<bool>, ConsensusError> = self
            .update_session(scope, proposal_id, |session| {
                if let ConsensusState::ConsensusReached(result) = session.state {
                    return Ok(Some(result));
                }

                // Try to calculate consensus result first - if we have enough votes, return the result
                // even if the proposal has technically expired
                let result = calculate_consensus_result(
                    &session.votes,
                    session.proposal.expected_voters_count,
                    session.config.consensus_threshold(),
                    session.proposal.liveness_criteria_yes,
                );

                if let Some(result) = result {
                    session.state = ConsensusState::ConsensusReached(result);
                    Ok(Some(result))
                } else {
                    session.state = ConsensusState::Failed;
                    Ok(None)
                }
            })
            .await;

        match timeout_result? {
            Some(consensus_result) => {
                self.emit_event(
                    scope,
                    ConsensusEvent::ConsensusReached {
                        proposal_id,
                        result: consensus_result,
                    },
                );
                Ok(consensus_result)
            }
            None => {
                self.emit_event(scope, ConsensusEvent::ConsensusFailed { proposal_id });
                Err(ConsensusError::InsufficientVotesAtTimeout)
            }
        }
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

    pub(crate) async fn update_session<R, F>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        mutator: F,
    ) -> Result<R, ConsensusError>
    where
        R: Send,
        F: FnOnce(&mut ConsensusSession) -> Result<R, ConsensusError> + Send,
    {
        self.storage
            .update_session(scope, proposal_id, mutator)
            .await
    }

    pub(crate) async fn save_session(
        &self,
        scope: &Scope,
        session: ConsensusSession,
    ) -> Result<(), ConsensusError> {
        self.storage.save_session(scope, session).await
    }

    pub(crate) async fn trim_scope_sessions(&self, scope: &Scope) -> Result<(), ConsensusError> {
        self.storage
            .update_scope_sessions(scope, |sessions| {
                if sessions.len() <= self.max_sessions_per_scope {
                    return Ok(());
                }

                sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                sessions.truncate(self.max_sessions_per_scope);
                Ok(())
            })
            .await
    }

    pub(crate) async fn list_scope_sessions(
        &self,
        scope: &Scope,
    ) -> Result<Vec<ConsensusSession>, ConsensusError> {
        self.storage
            .list_scope_sessions(scope)
            .await?
            .ok_or(ConsensusError::ScopeNotFound)
    }

    pub(crate) fn handle_transition(
        &self,
        scope: &Scope,
        proposal_id: u32,
        transition: SessionTransition,
    ) {
        if let SessionTransition::ConsensusReached(result) = transition {
            self.emit_event(
                scope,
                ConsensusEvent::ConsensusReached {
                    proposal_id,
                    result,
                },
            );
        }
    }

    pub(crate) fn spawn_timeout_task(
        &self,
        scope: Scope,
        proposal_id: u32,
        timeout_seconds: Duration,
    ) {
        let service = self.clone();
        Self::spawn_timeout_task_owned(service, scope, proposal_id, timeout_seconds);
    }

    fn spawn_timeout_task_owned(
        service: ConsensusService<Scope, S, E>,
        scope: Scope,
        proposal_id: u32,
        timeout_seconds: Duration,
    ) {
        tokio::spawn(async move {
            sleep(timeout_seconds).await;

            if service
                .get_consensus_result(&scope, proposal_id)
                .await
                .is_ok()
            {
                return;
            }

            if let Ok(result) = service.handle_consensus_timeout(&scope, proposal_id).await {
                info!(
                    "Automatic timeout applied for proposal {proposal_id} in scope {scope:?} after {timeout_seconds:?} => {result}"
                );
            }
        });
    }

    fn emit_event(&self, scope: &Scope, event: ConsensusEvent) {
        self.event_bus.publish(scope.clone(), event);
    }
}

/// Wrapper around ScopeConfigBuilder that stores service and scope for convenience methods.
pub struct ScopeConfigBuilderWrapper<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    service: ConsensusService<Scope, S, E>,
    scope: Scope,
    builder: ScopeConfigBuilder,
}

impl<Scope, S, E> ScopeConfigBuilderWrapper<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    fn new(
        service: ConsensusService<Scope, S, E>,
        scope: Scope,
        builder: ScopeConfigBuilder,
    ) -> Self {
        Self {
            service,
            scope,
            builder,
        }
    }

    /// Set network type (P2P or Gossipsub)
    pub fn with_network_type(mut self, network_type: NetworkType) -> Self {
        self.builder = self.builder.with_network_type(network_type);
        self
    }

    /// Set consensus threshold (0.0 to 1.0)
    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.builder = self.builder.with_threshold(threshold);
        self
    }

    /// Set default timeout for proposals (in seconds)
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.builder = self.builder.with_timeout(timeout);
        self
    }

    /// Set liveness criteria (how silent peers are counted)
    pub fn with_liveness_criteria(mut self, liveness_criteria_yes: bool) -> Self {
        self.builder = self.builder.with_liveness_criteria(liveness_criteria_yes);
        self
    }

    /// Override max rounds (if None, uses network_type defaults)
    pub fn with_max_rounds(mut self, max_rounds: Option<u32>) -> Self {
        self.builder = self.builder.with_max_rounds(max_rounds);
        self
    }

    /// Use P2P preset with common defaults
    pub fn p2p_preset(mut self) -> Self {
        self.builder = self.builder.p2p_preset();
        self
    }

    /// Use Gossipsub preset with common defaults
    pub fn gossipsub_preset(mut self) -> Self {
        self.builder = self.builder.gossipsub_preset();
        self
    }

    /// Use strict consensus (higher threshold = 0.9)
    pub fn strict_consensus(mut self) -> Self {
        self.builder = self.builder.strict_consensus();
        self
    }

    /// Use fast consensus (lower threshold = 0.6, shorter timeout = 30s)
    pub fn fast_consensus(mut self) -> Self {
        self.builder = self.builder.fast_consensus();
        self
    }

    /// Start with network-specific defaults
    pub fn with_network_defaults(mut self, network_type: NetworkType) -> Self {
        self.builder = self.builder.with_network_defaults(network_type);
        self
    }

    /// Initialize scope with the built configuration
    pub async fn initialize(self) -> Result<(), ConsensusError> {
        let config = self.builder.build()?;
        self.service.initialize_scope(&self.scope, config).await
    }

    /// Update existing scope configuration with the built configuration
    pub async fn update(self) -> Result<(), ConsensusError> {
        let config = self.builder.build()?;
        self.service
            .update_scope_config(&self.scope, |existing| {
                *existing = config;
                Ok(())
            })
            .await
    }

    /// Get the current configuration (useful for testing)
    pub fn get_config(&self) -> ScopeConfig {
        self.builder.get_config()
    }
}
