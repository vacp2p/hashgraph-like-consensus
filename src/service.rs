use std::marker::PhantomData;

use alloy_signer::Signer;
use tokio::time::Duration;

use crate::{
    error::ConsensusError,
    events::{BroadcastEventBus, ConsensusEventBus},
    protos::consensus::v1::{Proposal, Vote},
    scope::{ConsensusScope, ScopeID},
    scope_config::{NetworkType, ScopeConfig, ScopeConfigBuilder},
    session::{ConsensusConfig, ConsensusSession, ConsensusState},
    storage::{ConsensusStorage, InMemoryConsensusStorage},
    types::{ConsensusEvent, CreateProposalRequest, SessionTransition},
    utils::{
        build_vote, calculate_consensus_result, current_timestamp, validate_proposal_timestamp,
        validate_vote,
    },
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
    /// Eviction is silent — no event is emitted. Archive results you need before they
    /// are evicted.
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

    // ── Accessors ──────────────────────────────────────────────────────

    /// Access the underlying storage backend.
    ///
    /// Use this for reading state (sessions, proposals, scope config) and for
    /// lifecycle operations like [`delete_scope`](ConsensusStorage::delete_scope).
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Access the underlying event bus.
    ///
    /// Use this to [`subscribe`](ConsensusEventBus::subscribe) to consensus events.
    pub fn event_bus(&self) -> &E {
        &self.event_bus
    }

    // ── Consensus operations (business logic) ──────────────────────────

    /// Create a new proposal and start the voting process.
    ///
    /// This creates the proposal and sets up a session to track votes.
    /// The proposal will expire after the time specified in the request.
    ///
    /// **Important:** The library does not schedule timeouts automatically.
    /// Your application MUST call [`handle_consensus_timeout`](Self::handle_consensus_timeout)
    /// when the proposal's timeout elapses (e.g. via `tokio::time::sleep`).
    /// Without this call, proposals with offline voters will remain stuck in the
    /// `Active` state indefinitely, and the liveness criteria for silent peers
    /// will never take effect.
    ///
    /// Configuration is resolved from: proposal config > scope config > global default.
    /// If no config is provided, the scope's default configuration is used.
    pub async fn create_proposal(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
    ) -> Result<Proposal, ConsensusError> {
        self.create_proposal_with_config(scope, request, None).await
    }

    /// Create a new proposal with an explicit [`ConsensusConfig`] override.
    ///
    /// Pass `None` to fall back to scope defaults (same as [`create_proposal`](Self::create_proposal)).
    pub async fn create_proposal_with_config(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
        config: Option<ConsensusConfig>,
    ) -> Result<Proposal, ConsensusError> {
        let proposal = request.into_proposal()?;
        let config = self.resolve_config(scope, config, Some(&proposal)).await?;
        let (session, _) = ConsensusSession::from_proposal(proposal.clone(), config.clone())?;
        self.save_session(scope, session).await?;
        self.trim_scope_sessions(scope).await?;
        Ok(proposal)
    }

    /// Cast a vote on an active proposal.
    ///
    /// The vote is cryptographically signed with `signer` and linked into the
    /// hashgraph chain. Returns the signed [`Vote`] for network propagation.
    /// Each voter can only vote once per proposal.
    pub async fn cast_vote<SN: Signer + Sync + Send>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        signer: SN,
    ) -> Result<Vote, ConsensusError> {
        let session = self.get_session(scope, proposal_id).await?;
        validate_proposal_timestamp(session.proposal.expiration_timestamp)?;

        let voter_address = signer.address().as_slice().to_vec();
        if session.votes.contains_key(&voter_address) {
            return Err(ConsensusError::UserAlreadyVoted);
        }

        let vote = build_vote(&session.proposal, choice, signer).await?;
        let vote_clone = vote.clone();
        let transition = self
            .update_session(scope, proposal_id, move |session| {
                session.add_vote(vote_clone)
            })
            .await?;
        self.handle_transition(scope, proposal_id, transition);
        Ok(vote)
    }

    /// Cast a vote and return the updated [`Proposal`] (with the new vote included).
    ///
    /// Convenience method useful for the proposal creator who wants to immediately
    /// gossip the updated proposal to peers.
    pub async fn cast_vote_and_get_proposal<SN: Signer + Sync + Send>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        signer: SN,
    ) -> Result<Proposal, ConsensusError> {
        self.cast_vote(scope, proposal_id, choice, signer).await?;
        let session = self.get_session(scope, proposal_id).await?;
        Ok(session.proposal)
    }

    /// Process a proposal received from the network.
    ///
    /// Call this when your networking layer delivers a proposal from another peer.
    /// The library performs no I/O — your application must handle gossip/transport
    /// and call this method on receipt.
    ///
    /// Validates the proposal and all embedded votes, then stores it locally.
    /// If enough votes are already present, consensus is reached immediately.
    pub async fn process_incoming_proposal(
        &self,
        scope: &Scope,
        proposal: Proposal,
    ) -> Result<(), ConsensusError> {
        if self.get_session(scope, proposal.proposal_id).await.is_ok() {
            return Err(ConsensusError::ProposalAlreadyExist);
        }
        let config = self.resolve_config(scope, None, Some(&proposal)).await?;
        let (session, transition) = ConsensusSession::from_proposal(proposal, config)?;
        self.handle_transition(scope, session.proposal.proposal_id, transition);
        self.save_session(scope, session).await?;
        self.trim_scope_sessions(scope).await?;
        Ok(())
    }

    /// Process a single vote received from the network.
    ///
    /// Call this when your networking layer delivers a vote from another peer.
    /// Validates the vote (signature, timestamp, chain) and adds it to the
    /// corresponding proposal session. May trigger consensus.
    pub async fn process_incoming_vote(
        &self,
        scope: &Scope,
        vote: Vote,
    ) -> Result<(), ConsensusError> {
        let session = self.get_session(scope, vote.proposal_id).await?;
        validate_vote(
            &vote,
            session.proposal.expiration_timestamp,
            session.proposal.timestamp,
        )?;
        let proposal_id = vote.proposal_id;
        let transition = self
            .update_session(scope, proposal_id, move |session| session.add_vote(vote))
            .await?;
        self.handle_transition(scope, proposal_id, transition);
        Ok(())
    }

    /// Handle the timeout for a proposal.
    ///
    /// **The library does not call this automatically.** Your application MUST
    /// schedule a timer (e.g. `tokio::time::sleep(config.consensus_timeout())`)
    /// and invoke this method when it fires. Without this call, proposals with
    /// offline voters will stay in the `Active` state forever and silent-peer
    /// liveness logic will never run.
    ///
    /// At timeout, silent peers are counted toward quorum using the proposal's
    /// `liveness_criteria_yes` flag (RFC Section 4, Silent Node Management):
    ///
    /// - `liveness_criteria_yes = true` — silent peers count as YES
    /// - `liveness_criteria_yes = false` — silent peers count as NO
    ///
    /// Returns the consensus result if determinable, or
    /// [`InsufficientVotesAtTimeout`](ConsensusError::InsufficientVotesAtTimeout)
    /// if the result is a tie after counting silent peers.
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
                let result = calculate_consensus_result(
                    &session.votes,
                    session.proposal.expected_voters_count,
                    session.config.consensus_threshold(),
                    session.proposal.liveness_criteria_yes,
                    true,
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
                        timestamp: current_timestamp()?,
                    },
                );
                Ok(consensus_result)
            }
            None => {
                self.emit_event(
                    scope,
                    ConsensusEvent::ConsensusFailed {
                        proposal_id,
                        timestamp: current_timestamp()?,
                    },
                );
                Err(ConsensusError::InsufficientVotesAtTimeout)
            }
        }
    }

    // ── Scope management ─────────────────────────────────────────────

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
    async fn resolve_config(
        &self,
        scope: &Scope,
        proposal_override: Option<ConsensusConfig>,
        proposal: Option<&Proposal>,
    ) -> Result<ConsensusConfig, ConsensusError> {
        // 1. If explicit config override exists, use it as base
        // NOTE: if a per-proposal override is provided, we should not stomp its timeout
        // from the proposal's expiration fields (the caller explicitly chose it).
        let has_explicit_override = proposal_override.is_some();
        let base_config = if let Some(override_config) = proposal_override {
            override_config
        } else if let Some(scope_config) = self.storage.get_scope_config(scope).await? {
            ConsensusConfig::from(scope_config)
        } else {
            ConsensusConfig::gossipsub()
        };

        // 2. Apply proposal field overrides if proposal is provided
        if let Some(prop) = proposal {
            // Calculate timeout from expiration_timestamp (absolute timestamp) - timestamp (creation time),
            // unless an explicit override was supplied.
            let timeout_seconds = if has_explicit_override {
                base_config.consensus_timeout()
            } else if prop.expiration_timestamp > prop.timestamp {
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

    async fn get_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<ConsensusSession, ConsensusError> {
        self.storage
            .get_session(scope, proposal_id)
            .await?
            .ok_or(ConsensusError::SessionNotFound)
    }

    async fn update_session<R, F>(
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

    async fn save_session(
        &self,
        scope: &Scope,
        session: ConsensusSession,
    ) -> Result<(), ConsensusError> {
        self.storage.save_session(scope, session).await
    }

    async fn trim_scope_sessions(&self, scope: &Scope) -> Result<(), ConsensusError> {
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

    fn handle_transition(&self, scope: &Scope, proposal_id: u32, transition: SessionTransition) {
        if let SessionTransition::ConsensusReached(result) = transition {
            self.emit_event(
                scope,
                ConsensusEvent::ConsensusReached {
                    proposal_id,
                    result,
                    timestamp: current_timestamp().unwrap_or(0),
                },
            );
        }
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
