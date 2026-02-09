//! Public API trait for the consensus service.
//!
//! [`ConsensusServiceAPI`] defines the full set of operations available to callers:
//! creating proposals, casting votes, processing network messages, and querying state.

use alloy_signer::Signer;

use crate::{
    error::ConsensusError,
    events::ConsensusEventBus,
    protos::consensus::v1::{Proposal, Vote},
    scope::ConsensusScope,
    session::ConsensusConfig,
    storage::ConsensusStorage,
    types::CreateProposalRequest,
};

/// Defines the public contract for a consensus service.
///
/// Generic over the scope type (`Scope`), storage backend (`S`), and event bus (`E`).
/// The default implementation is provided by
/// [`ConsensusService`](crate::service::ConsensusService).
pub trait ConsensusServiceAPI<Scope, S, E>
where
    Scope: ConsensusScope,
    S: ConsensusStorage<Scope>,
    E: ConsensusEventBus<Scope>,
{
    /// Create a new proposal using scope-level (or global default) configuration.
    fn create_proposal(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
    ) -> impl Future<Output = Result<Proposal, ConsensusError>> + Send;

    /// Create a new proposal with an explicit [`ConsensusConfig`] override.
    ///
    /// Pass `None` to fall back to scope defaults (same as [`create_proposal`](Self::create_proposal)).
    fn create_proposal_with_config(
        &self,
        scope: &Scope,
        request: CreateProposalRequest,
        config: Option<ConsensusConfig>,
    ) -> impl Future<Output = Result<Proposal, ConsensusError>> + Send;

    /// Cast a vote on an active proposal.
    ///
    /// The vote is cryptographically signed with `signer` and linked into the
    /// hashgraph chain. Returns the signed [`Vote`] for network propagation.
    fn cast_vote<SN: Signer + Sync + Send>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        signer: SN,
    ) -> impl Future<Output = Result<Vote, ConsensusError>> + Send;

    /// Cast a vote and return the updated [`Proposal`] (with the new vote included).
    ///
    /// Convenience method useful for the proposal creator who wants to immediately
    /// gossip the updated proposal to peers.
    fn cast_vote_and_get_proposal<SN: Signer + Sync + Send>(
        &self,
        scope: &Scope,
        proposal_id: u32,
        choice: bool,
        signer: SN,
    ) -> impl Future<Output = Result<Proposal, ConsensusError>> + Send;

    /// Process a proposal received from the network.
    ///
    /// Validates the proposal and all embedded votes, then stores it locally.
    /// If enough votes are already present, consensus is reached immediately.
    fn process_incoming_proposal(
        &self,
        scope: &Scope,
        proposal: Proposal,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;

    /// Process a single vote received from the network.
    ///
    /// Validates the vote (signature, timestamp, chain) and adds it to the
    /// corresponding proposal session. May trigger consensus.
    fn process_incoming_vote(
        &self,
        scope: &Scope,
        vote: Vote,
    ) -> impl Future<Output = Result<(), ConsensusError>> + Send;

    /// Retrieve a proposal by ID, including all votes collected so far.
    fn get_proposal(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> impl Future<Output = Result<Proposal, ConsensusError>> + Send;

    /// Retrieve only the payload bytes of a proposal.
    fn get_proposal_payload(
        &self,
        scope: &Scope,
        proposal_id: u32,
    ) -> impl Future<Output = Result<Vec<u8>, ConsensusError>> + Send;
}
