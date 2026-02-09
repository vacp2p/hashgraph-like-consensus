//! Implementation of [`ConsensusServiceAPI`] for [`ConsensusService`].

use alloy_signer::Signer;

use crate::{
    api::ConsensusServiceAPI,
    error::ConsensusError,
    events::ConsensusEventBus,
    protos::consensus::v1::{Proposal, Vote},
    scope::ConsensusScope,
    service::ConsensusService,
    session::{ConsensusConfig, ConsensusSession},
    storage::ConsensusStorage,
    types::CreateProposalRequest,
    utils::{build_vote, validate_proposal_timestamp, validate_vote},
};

impl<Scope, S, E> ConsensusServiceAPI<Scope, S, E> for ConsensusService<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    /// Create a new proposal and start the voting process.
    ///
    /// This creates the proposal, sets up a session to track votes, and schedules automatic
    /// timeout handling. The proposal will expire after the time specified in the request.
    ///
    /// Configuration is resolved from: proposal config > scope config > global default.
    /// If no config is provided, the scope's default configuration is used.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hashgraph_like_consensus::{api::ConsensusServiceAPI, scope::ScopeID,
    /// scope_config::NetworkType, service::DefaultConsensusService, types::CreateProposalRequest};
    ///
    /// async fn example() -> Result<(), Box<dyn std::error::Error>> {
    ///     let service = DefaultConsensusService::default();
    ///     let scope = ScopeID::from("my_scope");
    ///
    ///     service
    ///         .scope(&scope)
    ///         .await?
    ///         .with_network_type(NetworkType::P2P)
    ///         .with_threshold(0.75)
    ///         .initialize()
    ///         .await?;
    ///
    ///     let request = CreateProposalRequest::new(
    ///         "Test Proposal".to_string(),
    ///         b"payload".to_vec(),
    ///         vec![0u8; 20],
    ///         3,
    ///         100,
    ///         true,
    ///     )?;
    ///     let proposal = service.create_proposal(&scope, request).await?;
    ///     Ok(())
    /// }
    /// ```
    async fn create_proposal(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
    ) -> Result<Proposal, ConsensusError> {
        self.create_proposal_with_config(scope, request, None).await
    }

    /// Create a new proposal with explicit configuration override.
    ///
    /// This allows you to override the scope's default configuration for a specific proposal.
    /// The override takes precedence over scope config.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hashgraph_like_consensus::{api::ConsensusServiceAPI, scope::ScopeID,
    /// service::DefaultConsensusService, session::ConsensusConfig, types::CreateProposalRequest};
    ///
    /// async fn example() -> Result<(), Box<dyn std::error::Error>> {
    ///     let service = DefaultConsensusService::default();
    ///     let scope = ScopeID::from("my_scope");
    ///     let request = CreateProposalRequest::new(
    ///         "Test Proposal".to_string(),
    ///         b"payload".to_vec(),
    ///         vec![0u8; 20],
    ///         3,
    ///         100,
    ///         true,
    ///     )?;
    ///
    ///     let proposal = service.create_proposal_with_config(
    ///         &scope,
    ///         request,
    ///         Some(ConsensusConfig::p2p())
    ///     ).await?;
    ///
    ///     let request2 = CreateProposalRequest::new(
    ///         "Another Proposal".to_string(),
    ///         b"payload2".to_vec(),
    ///         vec![0u8; 20],
    ///         3,
    ///         100,
    ///         true,
    ///     )?;
    ///     let proposal2 = service.create_proposal_with_config(
    ///         &scope,
    ///         request2,
    ///         None
    ///     ).await?;
    ///     Ok(())
    /// }
    /// ```
    async fn create_proposal_with_config(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
        config: Option<ConsensusConfig>,
    ) -> Result<Proposal, ConsensusError> {
        let proposal = request.into_proposal()?;

        // Resolve config: override > scope config > global default, aligning timeout with proposal
        let config = self.resolve_config(scope, config, Some(&proposal)).await?;

        let (session, _) = ConsensusSession::from_proposal(proposal.clone(), config.clone())?;
        self.save_session(scope, session).await?;
        self.trim_scope_sessions(scope).await?;

        Ok(proposal)
    }

    /// Cast your vote on a proposal (yes or no).
    ///
    /// Vote is cryptographically signed and linked to previous votes in the hashgraph.
    /// Returns the signed vote, which you can then send to other peers in the network.
    /// Each voter can only vote once per proposal.
    async fn cast_vote<SN: Signer + Sync + Send>(
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

    /// Cast a vote and immediately get back the updated proposal.
    ///
    /// This is a convenience method that combines `cast_vote` and fetching the proposal.
    /// Useful for proposal creator as they can immediately see the proposal with their vote
    /// and share it with other peers.
    async fn cast_vote_and_get_proposal<SN: Signer + Sync + Send>(
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

    /// Process a proposal you received from another peer in the network.
    ///
    /// This validates the proposal and all its votes (signatures, vote chains, timestamps),
    /// then stores it locally.
    /// If it necessary the consensus configuration is resolved from the proposal.
    /// If the proposal already has enough votes, consensus is reached
    /// immediately and an event is emitted.
    async fn process_incoming_proposal(
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

    /// Process a vote you received from another peer.
    ///
    /// The vote is validated (signature, timestamp, vote chain) and added to the proposal.
    /// If this vote brings the total to the consensus threshold, consensus is reached and
    /// an event is emitted.
    async fn process_incoming_vote(&self, scope: &Scope, vote: Vote) -> Result<(), ConsensusError> {
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

    async fn get_proposal(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<Proposal, ConsensusError> {
        let session = self.get_session(scope, proposal_id).await?;
        Ok(session.proposal)
    }

    async fn get_proposal_payload(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> Result<Vec<u8>, ConsensusError> {
        let session = self.get_session(scope, proposal_id).await?;
        Ok(session.proposal.payload)
    }
}
