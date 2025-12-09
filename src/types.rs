use std::time::Duration;

use crate::{
    error::ConsensusError,
    protos::consensus::v1::Proposal,
    utils::{current_timestamp, generate_id, validate_expected_voters_count, validate_timeout},
};

#[derive(Debug, Clone)]
pub enum ConsensusEvent {
    /// Consensus was reached! The proposal has a final result (yes or no).
    ConsensusReached { proposal_id: u32, result: bool },
    /// Consensus failed - not enough votes were collected before the timeout.
    ConsensusFailed { proposal_id: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTransition {
    /// Session remains active with no outcome yet.
    StillActive,
    /// Session converged to a boolean result.
    ConsensusReached(bool),
}

#[derive(Debug, Clone)]
pub struct CreateProposalRequest {
    /// A short name for the proposal (e.g., "Upgrade to v2").
    pub name: String,
    /// Additional details about what's being voted on.
    pub payload: String,
    /// The address (public key bytes) of whoever created this proposal.
    pub proposal_owner: Vec<u8>,
    /// How many people are expected to vote (used to calculate consensus threshold).
    pub expected_voters_count: u32,
    /// The timestamp at which the proposal becomes outdated.
    pub expiration_timestamp: u64,
    /// What happens if votes are tied: `true` means YES wins, `false` means NO wins.
    pub liveness_criteria_yes: bool,
}

impl CreateProposalRequest {
    /// Create a new proposal request with validation.
    pub fn new(
        name: String,
        payload: String,
        proposal_owner: Vec<u8>,
        expected_voters_count: u32,
        expiration_timestamp: u64,
        liveness_criteria_yes: bool,
    ) -> Result<Self, ConsensusError> {
        validate_expected_voters_count(expected_voters_count)?;
        validate_timeout(Duration::from_secs(expiration_timestamp))?;
        let request = Self {
            name,
            payload,
            proposal_owner,
            expected_voters_count,
            expiration_timestamp,
            liveness_criteria_yes,
        };
        Ok(request)
    }

    /// Convert this request into an actual proposal.
    ///
    /// Generates a unique proposal ID and sets the creation timestamp. The proposal
    /// starts with round 1 and no votes.
    pub fn into_proposal(self) -> Result<Proposal, ConsensusError> {
        let proposal_id = generate_id();
        let now = current_timestamp()?;

        Ok(Proposal {
            name: self.name,
            payload: self.payload,
            proposal_id,
            proposal_owner: self.proposal_owner,
            votes: vec![],
            expected_voters_count: self.expected_voters_count,
            round: 1,
            timestamp: now,
            expiration_timestamp: now + self.expiration_timestamp,
            liveness_criteria_yes: self.liveness_criteria_yes,
        })
    }
}
