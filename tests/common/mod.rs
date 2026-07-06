//! Helpers shared by the integration-test crates.
//!
//! Each test binary compiles this module independently and uses only a
//! subset of the helpers, so unused-code warnings are suppressed.
#![allow(dead_code)]

use alloy::signers::local::PrivateKeySigner;
use hashgraph_like_consensus::{
    error::ConsensusError,
    protos::consensus::v1::{Proposal, Vote},
    scope::ScopeID,
    service::DefaultConsensusService,
    signing::EthereumConsensusSigner,
    storage::ConsensusStorage,
    utils::build_vote,
};

/// Current time in seconds since Unix epoch — the caller-supplied `now`
/// the library expects.
pub fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A service with in-memory storage and a fresh random signer.
pub fn make_service() -> DefaultConsensusService {
    DefaultConsensusService::new(EthereumConsensusSigner::new(PrivateKeySigner::random()))
}

/// Wrap a raw key into the Ethereum signature scheme.
pub fn wrap(signer: PrivateKeySigner) -> EthereumConsensusSigner {
    EthereumConsensusSigner::new(signer)
}

/// The signer's address bytes, as used for proposal/vote owner fields.
pub fn owner_bytes(signer: &PrivateKeySigner) -> Vec<u8> {
    signer.address().as_slice().to_vec()
}

/// Build and process a vote as if it arrived from a remote peer, returning
/// the vote for further gossip.
pub fn cast_remote_vote(
    service: &DefaultConsensusService,
    scope: &ScopeID,
    proposal_id: u32,
    choice: bool,
    signer: &EthereumConsensusSigner,
) -> Result<Vote, ConsensusError> {
    let proposal = service.storage().get_proposal(scope, proposal_id)?;
    let vote = build_vote(&proposal, choice, signer, now_ts())?;
    service.process_incoming_vote(scope, vote.clone(), now_ts())?;
    Ok(vote)
}

/// [`cast_remote_vote`], then return the updated proposal snapshot.
pub fn cast_remote_vote_and_get_proposal(
    service: &DefaultConsensusService,
    scope: &ScopeID,
    proposal_id: u32,
    choice: bool,
    signer: &EthereumConsensusSigner,
) -> Result<Proposal, ConsensusError> {
    cast_remote_vote(service, scope, proposal_id, choice, signer)?;
    service.storage().get_proposal(scope, proposal_id)
}
