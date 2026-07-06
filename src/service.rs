use std::marker::PhantomData;
use std::time::Duration;

use crate::{
    error::ConsensusError,
    events::ConsensusEventBus,
    protos::consensus::v1::{Proposal, Vote},
    scope::ConsensusScope,
    scope_config::{NetworkType, ScopeConfig, ScopeConfigBuilder},
    session::{ConsensusConfig, ConsensusSession, ConsensusState},
    signing::ConsensusSignatureScheme,
    storage::ConsensusStorage,
    types::{ConsensusEvent, CreateProposalRequest, SessionTransition},
    utils::{build_vote, calculate_consensus_result, validate_proposal_timestamp, validate_vote},
};
#[cfg(feature = "ethereum")]
use crate::{
    events::BroadcastEventBus, scope::ScopeID, signing::EthereumConsensusSigner,
    storage::InMemoryConsensusStorage,
};
/// The main service that handles proposals, votes, and consensus.
///
/// This is the main entry point for using the consensus service.
/// It handles creating proposals, processing votes, and managing timeouts.
///
/// Each `ConsensusService` represents **one peer's view**: it holds the
/// storage handle, the event bus, and that peer's signer. The multi-peer
/// pattern is one service per peer with shared storage and (optionally)
/// shared event bus — see the README "Service shape" section.
///
/// Generic parameters:
///
/// - `Scope`: scope key type (see [`ConsensusScope`]).
/// - `Storage`: storage backend (see [`ConsensusStorage`]).
/// - `Event`: event bus backend (see [`ConsensusEventBus`]).
/// - `Signer`: signature scheme (see [`ConsensusSignatureScheme`]).
///   The held instance signs this peer's outgoing votes; the same type
///   provides `Signer::verify` for validating incoming votes.
pub struct ConsensusService<Scope, Storage, Event, Signer>
where
    Scope: ConsensusScope,
    Storage: ConsensusStorage<Scope>,
    Event: ConsensusEventBus<Scope>,
    Signer: ConsensusSignatureScheme,
{
    storage: Storage,
    max_sessions_per_scope: usize,
    event_bus: Event,
    signer: Signer,
    _scope: PhantomData<Scope>,
}

impl<Scope, Storage, Event, Signer> Clone for ConsensusService<Scope, Storage, Event, Signer>
where
    Scope: ConsensusScope,
    Storage: ConsensusStorage<Scope>,
    Event: ConsensusEventBus<Scope>,
    Signer: ConsensusSignatureScheme,
{
    fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
            max_sessions_per_scope: self.max_sessions_per_scope,
            event_bus: self.event_bus.clone(),
            signer: self.signer.clone(),
            _scope: PhantomData,
        }
    }
}

/// A ready-to-use service with in-memory storage, broadcast events, and the
/// [`EthereumConsensusSigner`] scheme. Intended for tests and single-node
/// integrations. Production deployments typically construct
/// [`ConsensusService`] directly with their own scheme.
///
/// Requires the default `ethereum` feature.
#[cfg(feature = "ethereum")]
pub type DefaultConsensusService = ConsensusService<
    ScopeID,
    InMemoryConsensusStorage<ScopeID>,
    BroadcastEventBus<ScopeID>,
    EthereumConsensusSigner,
>;

#[cfg(feature = "ethereum")]
impl DefaultConsensusService {
    /// Create a service with in-memory storage, broadcast events, and the
    /// given signer. Defaults to 10 max sessions per scope.
    pub fn new(signer: EthereumConsensusSigner) -> Self {
        Self::new_with_max_sessions(signer, 10)
    }

    /// Create a service with a custom limit on how many sessions can exist per scope.
    ///
    /// When the limit is reached, older sessions are automatically removed to make room.
    /// Eviction is silent — no event is emitted. Archive results you need before they
    /// are evicted.
    pub fn new_with_max_sessions(
        signer: EthereumConsensusSigner,
        max_sessions_per_scope: usize,
    ) -> Self {
        Self::new_with_components(
            InMemoryConsensusStorage::new(),
            BroadcastEventBus::default(),
            signer,
            max_sessions_per_scope,
        )
    }
}

impl<Scope, Storage, Event, Signer> ConsensusService<Scope, Storage, Event, Signer>
where
    Scope: ConsensusScope,
    Storage: ConsensusStorage<Scope>,
    Event: ConsensusEventBus<Scope>,
    Signer: ConsensusSignatureScheme,
{
    /// Build a service with your own storage and event bus implementations.
    ///
    /// The signature scheme `Signer` is selected via turbofish or type inference at
    /// the call site (or by using a type alias like
    /// [`DefaultConsensusService`]). Use this when you need custom persistence
    /// (like a database) or event handling. The `max_sessions_per_scope`
    /// parameter controls how many sessions can exist per scope. When the
    /// limit is reached, older sessions are automatically removed.
    pub fn new_with_components(
        storage: Storage,
        event_bus: Event,
        signer: Signer,
        max_sessions_per_scope: usize,
    ) -> Self {
        Self {
            storage,
            max_sessions_per_scope,
            event_bus,
            signer,
            _scope: PhantomData,
        }
    }

    // ── Accessors ──────────────────────────────────────────────────────

    /// Access the underlying storage backend.
    ///
    /// Use this for reading state (sessions, proposals, scope config) and for
    /// lifecycle operations like [`delete_scope`](ConsensusStorage::delete_scope).
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// Access the underlying event bus.
    ///
    /// Use this to [`subscribe`](ConsensusEventBus::subscribe) to consensus events.
    pub fn event_bus(&self) -> &Event {
        &self.event_bus
    }

    /// Access this peer's signer.
    ///
    /// Use this to read the peer's identity (e.g. for proposal owner fields):
    /// `service.signer().identity()`.
    pub fn signer(&self) -> &Signer {
        &self.signer
    }

    // ── Consensus operations (business logic) ──────────────────────────

    /// Create a new proposal and start the voting process.
    ///
    /// This creates the proposal and sets up a session to track votes.
    /// The proposal will expire after the time specified in the request.
    ///
    /// **Important:** The library does not schedule timeouts automatically.
    /// Your application MUST call [`handle_consensus_timeout`](Self::handle_consensus_timeout)
    /// when the proposal's timeout elapses. Without this call, proposals with
    /// offline voters will remain stuck in the `Active` state indefinitely, and
    /// the liveness criteria for silent peers will never take effect.
    ///
    /// Configuration is resolved from: proposal config > scope config > global default.
    /// If no config is provided, the scope's default configuration is used.
    ///
    /// `now` is the current time in seconds since Unix epoch, supplied by the caller.
    pub fn create_proposal(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
        now: u64,
    ) -> Result<Proposal, ConsensusError> {
        self.create_proposal_with_config(scope, request, None, now)
    }

    /// Create a new proposal with an explicit [`ConsensusConfig`] override.
    ///
    /// Pass `None` to fall back to scope defaults (same as [`create_proposal`](Self::create_proposal)).
    pub fn create_proposal_with_config(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
        config: Option<ConsensusConfig>,
        now: u64,
    ) -> Result<Proposal, ConsensusError> {
        let proposal = request.into_proposal(now)?;
        let config = self.resolve_config(scope, config, Some(&proposal))?;
        let (session, _) =
            ConsensusSession::from_proposal::<Signer>(proposal.clone(), config.clone(), now)?;
        self.save_session(scope, session)?;
        self.trim_scope_sessions(scope)?;
        Ok(proposal)
    }

    /// Cast a vote on an active proposal using this service's held signer.
    ///
    /// The vote is cryptographically signed and linked into the hashgraph
    /// chain. Returns the signed [`Vote`] for network propagation. Each peer
    /// (identity) can only vote once per proposal.
    pub fn cast_vote(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        now: u64,
    ) -> Result<Vote, ConsensusError> {
        let session = self.get_session(scope, proposal_id)?;
        validate_proposal_timestamp(session.proposal.expiration_timestamp, now)?;

        if session.votes.contains_key(self.signer.identity()) {
            return Err(ConsensusError::UserAlreadyVoted);
        }

        let vote = build_vote(&session.proposal, choice, &self.signer, now)?;
        let vote_clone = vote.clone();
        let transition = self.update_session(scope, proposal_id, move |session| {
            session.add_vote(vote_clone, now)
        })?;
        self.handle_transition(scope, proposal_id, transition, now);
        Ok(vote)
    }

    /// Cast a vote and return the updated [`Proposal`] (with the new vote included).
    ///
    /// Convenience method useful for the proposal creator who wants to immediately
    /// gossip the updated proposal to peers.
    pub fn cast_vote_and_get_proposal(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        now: u64,
    ) -> Result<Proposal, ConsensusError> {
        self.cast_vote(scope, proposal_id, choice, now)?;
        let session = self.get_session(scope, proposal_id)?;
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
    pub fn process_incoming_proposal(
        &self,
        scope: &Scope,
        proposal: Proposal,
        now: u64,
    ) -> Result<(), ConsensusError> {
        if self.get_session(scope, proposal.proposal_id).is_ok() {
            return Err(ConsensusError::ProposalAlreadyExist);
        }
        let config = self.resolve_config(scope, None, Some(&proposal))?;
        let (session, transition) =
            ConsensusSession::from_proposal::<Signer>(proposal, config, now)?;
        self.handle_transition(scope, session.proposal.proposal_id, transition, now);
        self.save_session(scope, session)?;
        self.trim_scope_sessions(scope)?;
        Ok(())
    }

    /// Process a single vote received from the network.
    ///
    /// Call this when your networking layer delivers a vote from another peer.
    /// Validates the vote (signature, timestamp, chain) and adds it to the
    /// corresponding proposal session. May trigger consensus.
    pub fn process_incoming_vote(
        &self,
        scope: &Scope,
        vote: Vote,
        now: u64,
    ) -> Result<(), ConsensusError> {
        let session = self.get_session(scope, vote.proposal_id)?;
        validate_vote::<Signer>(
            &vote,
            session.proposal.expiration_timestamp,
            session.proposal.timestamp,
            now,
        )?;
        let proposal_id = vote.proposal_id;
        let transition = self.update_session(scope, proposal_id, move |session| {
            session.add_vote(vote, now)
        })?;
        self.handle_transition(scope, proposal_id, transition, now);
        Ok(())
    }

    /// Handle the timeout for a proposal.
    ///
    /// **The library does not call this automatically.** Your application MUST
    /// schedule a timer for `config.consensus_timeout()` and invoke this method
    /// when it fires. Without this call, proposals with offline voters will stay
    /// in the `Active` state forever and silent-peer liveness logic will never run.
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
    pub fn handle_consensus_timeout(
        &self,
        scope: &Scope,
        proposal_id: u32,
        now: u64,
    ) -> Result<bool, ConsensusError> {
        let timeout_result: Result<Option<bool>, ConsensusError> =
            self.update_session(scope, proposal_id, |session| {
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
            });

        match timeout_result? {
            Some(consensus_result) => {
                self.emit_event(
                    scope,
                    ConsensusEvent::ConsensusReached {
                        proposal_id,
                        result: consensus_result,
                        timestamp: now,
                    },
                );
                Ok(consensus_result)
            }
            None => {
                self.emit_event(
                    scope,
                    ConsensusEvent::ConsensusFailed {
                        proposal_id,
                        timestamp: now,
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
    /// use hashgraph_like_consensus::{
    ///     scope::ScopeID,
    ///     scope_config::NetworkType,
    ///     service::DefaultConsensusService,
    ///     signing::EthereumConsensusSigner,
    /// };
    /// use alloy::signers::local::PrivateKeySigner;
    /// use std::time::Duration;
    ///
    /// fn example() -> Result<(), Box<dyn std::error::Error>> {
    ///   let signer = EthereumConsensusSigner::new(PrivateKeySigner::random());
    ///   let service = DefaultConsensusService::new(signer);
    ///   let scope = ScopeID::from("my_scope");
    ///
    ///   // Initialize new scope
    ///   service
    ///     .scope(&scope)?
    ///     .with_network_type(NetworkType::P2P)
    ///     .with_threshold(0.75)
    ///     .with_timeout(Duration::from_secs(120))
    ///     .initialize()?;
    ///
    ///   // Update existing scope (single field)
    ///   service
    ///     .scope(&scope)?
    ///     .with_threshold(0.8)
    ///     .update()?;
    ///   Ok(())
    /// }
    /// ```
    pub fn scope(
        &self,
        scope: &Scope,
    ) -> Result<ScopeConfigBuilderWrapper<Scope, Storage, Event, Signer>, ConsensusError> {
        let existing_config = self.storage.get_scope_config(scope)?;
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

    fn initialize_scope(&self, scope: &Scope, config: ScopeConfig) -> Result<(), ConsensusError> {
        config.validate()?;
        self.storage.set_scope_config(scope, config)
    }

    fn update_scope_config<F>(&self, scope: &Scope, updater: F) -> Result<(), ConsensusError>
    where
        F: FnOnce(&mut ScopeConfig) -> Result<(), ConsensusError>,
    {
        self.storage.update_scope_config(scope, updater)
    }

    /// Resolve configuration for a proposal.
    ///
    /// Priority: proposal override > proposal fields (expiration_timestamp, liveness_criteria_yes)
    ///   > scope config > global default
    fn resolve_config(
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
        } else if let Some(scope_config) = self.storage.get_scope_config(scope)? {
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

    fn get_session(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<ConsensusSession, ConsensusError> {
        self.storage
            .get_session(scope, proposal_id)?
            .ok_or(ConsensusError::SessionNotFound)
    }

    fn update_session<R, F>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        mutator: F,
    ) -> Result<R, ConsensusError>
    where
        F: FnOnce(&mut ConsensusSession) -> Result<R, ConsensusError>,
    {
        self.storage.update_session(scope, proposal_id, mutator)
    }

    fn save_session(&self, scope: &Scope, session: ConsensusSession) -> Result<(), ConsensusError> {
        self.storage.save_session(scope, session)
    }

    fn trim_scope_sessions(&self, scope: &Scope) -> Result<(), ConsensusError> {
        self.storage.update_scope_sessions(scope, |sessions| {
            if sessions.len() <= self.max_sessions_per_scope {
                return Ok(());
            }

            sessions.sort_by_key(|s| std::cmp::Reverse(s.created_at));
            sessions.truncate(self.max_sessions_per_scope);
            Ok(())
        })
    }

    pub(crate) fn list_scope_sessions(
        &self,
        scope: &Scope,
    ) -> Result<Vec<ConsensusSession>, ConsensusError> {
        self.storage
            .list_scope_sessions(scope)?
            .ok_or(ConsensusError::ScopeNotFound)
    }

    fn handle_transition(
        &self,
        scope: &Scope,
        proposal_id: u32,
        transition: SessionTransition,
        now: u64,
    ) {
        if let SessionTransition::ConsensusReached(result) = transition {
            self.emit_event(
                scope,
                ConsensusEvent::ConsensusReached {
                    proposal_id,
                    result,
                    timestamp: now,
                },
            );
        }
    }

    fn emit_event(&self, scope: &Scope, event: ConsensusEvent) {
        self.event_bus.publish(scope.clone(), event);
    }
}

/// Wrapper around ScopeConfigBuilder that stores service and scope for convenience methods.
pub struct ScopeConfigBuilderWrapper<Scope, Storage, Event, Signer>
where
    Scope: ConsensusScope,
    Storage: ConsensusStorage<Scope>,
    Event: ConsensusEventBus<Scope>,
    Signer: ConsensusSignatureScheme,
{
    service: ConsensusService<Scope, Storage, Event, Signer>,
    scope: Scope,
    builder: ScopeConfigBuilder,
}

impl<Scope, Storage, Event, Signer> ScopeConfigBuilderWrapper<Scope, Storage, Event, Signer>
where
    Scope: ConsensusScope,
    Storage: ConsensusStorage<Scope>,
    Event: ConsensusEventBus<Scope>,
    Signer: ConsensusSignatureScheme,
{
    fn new(
        service: ConsensusService<Scope, Storage, Event, Signer>,
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
    pub fn initialize(self) -> Result<(), ConsensusError> {
        let config = self.builder.build()?;
        self.service.initialize_scope(&self.scope, config)
    }

    /// Update existing scope configuration with the built configuration
    pub fn update(self) -> Result<(), ConsensusError> {
        let config = self.builder.build()?;
        self.service.update_scope_config(&self.scope, |existing| {
            *existing = config;
            Ok(())
        })
    }

    /// Get the current configuration (useful for testing)
    pub fn get_config(&self) -> ScopeConfig {
        self.builder.get_config()
    }
}
