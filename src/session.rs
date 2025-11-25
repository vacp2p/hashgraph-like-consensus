use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    error::ConsensusError,
    protos::consensus::v1::{Proposal, Vote},
};

#[derive(Debug, Clone)]
pub enum ConsensusEvent {
    ConsensusReached { proposal_id: u32, result: bool },
    ConsensusFailed { proposal_id: u32, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsensusTransition {
    StillActive,
    ConsensusReached(bool),
}

/// Consensus configuration
#[derive(Debug, Clone)]
pub struct ConsensusConfig {
    /// Minimum number of votes required for consensus (as percentage of expected voters)
    pub consensus_threshold: f64,
    /// Timeout for consensus rounds in seconds
    pub consensus_timeout: u64,
    /// Maximum number of rounds before consensus is considered failed
    pub max_rounds: u32,
    /// Whether to use liveness criteria for silent peers
    pub liveness_criteria: bool,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            consensus_threshold: 0.67, // 67% supermajority
            consensus_timeout: 10,     // 10 seconds
            max_rounds: 3,             // Maximum 3 rounds
            liveness_criteria: true,
        }
    }
}

/// Consensus state for a proposal
#[derive(Debug, Clone)]
pub enum ConsensusState {
    Active,
    ConsensusReached(bool), // true for yes, false for no
    Expired,
}

/// Consensus session for a specific proposal
#[derive(Debug, Clone)]
pub struct ConsensusSession {
    pub proposal: Proposal,
    pub state: ConsensusState,
    pub votes: HashMap<Vec<u8>, Vote>, // vote_owner -> Vote
    pub created_at: u64,
    pub config: ConsensusConfig,
}

impl ConsensusSession {
    pub(crate) fn new(proposal: Proposal, config: ConsensusConfig) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Failed to get current time")
            .as_secs();

        Self {
            proposal,
            state: ConsensusState::Active,
            votes: HashMap::new(),
            created_at: now,
            config,
        }
    }

    pub(crate) fn set_consensus_threshold(&mut self, consensus_threshold: f64) {
        self.config.consensus_threshold = consensus_threshold
    }

    pub(crate) fn add_vote(&mut self, vote: Vote) -> Result<ConsensusTransition, ConsensusError> {
        match self.state {
            ConsensusState::Active => {
                if self.votes.contains_key(&vote.vote_owner) {
                    return Err(ConsensusError::DuplicateVote);
                }
                self.votes.insert(vote.vote_owner.clone(), vote.clone());
                self.proposal.votes.push(vote.clone());
                Ok(self.check_consensus())
            }
            ConsensusState::ConsensusReached(res) => Ok(ConsensusTransition::ConsensusReached(res)),
            _ => Err(ConsensusError::SessionNotActive),
        }
    }

    /// Count the number of required votes to reach consensus
    /// If the number of expected voters is less than or equal to 2, we require all votes to reach consensus
    /// Otherwise, we require a supermajority of votes to reach consensus
    fn count_required_votes(&self) -> usize {
        let expected_voters = self.proposal.expected_voters_count as usize;
        if expected_voters <= 2 {
            expected_voters
        } else {
            ((expected_voters as f64) * self.config.consensus_threshold) as usize
        }
    }

    /// Check if consensus has been reached
    ///
    /// - `ConsensusReached(true)`
    ///     - if yes votes > no votes
    ///     - if no votes == yes votes && liveness criteria is true && we have get all possible votes
    /// - `ConsensusReached(false)`
    ///     - if no votes > yes votes
    ///     - if no votes == yes votes && liveness criteria is false && we have get all possible votes
    /// - `StillActive`
    ///     - if no votes == yes votes and we don't get all votes, but reach the required threshold
    ///     - if total votes < required votes (we wait for more votes)
    fn check_consensus(&mut self) -> ConsensusTransition {
        let total_votes = self.votes.len();
        let yes_votes = self.votes.values().filter(|v| v.vote).count();
        let no_votes = total_votes - yes_votes;

        let expected_voters = self.proposal.expected_voters_count as usize;
        let required_votes = self.count_required_votes();
        if total_votes >= required_votes {
            if yes_votes > no_votes {
                self.state = ConsensusState::ConsensusReached(true);
                ConsensusTransition::ConsensusReached(true)
            } else if no_votes > yes_votes {
                self.state = ConsensusState::ConsensusReached(false);
                ConsensusTransition::ConsensusReached(false)
            } else if total_votes == expected_voters {
                self.state = ConsensusState::ConsensusReached(self.config.liveness_criteria);
                ConsensusTransition::ConsensusReached(self.config.liveness_criteria)
            } else {
                self.state = ConsensusState::Active;
                ConsensusTransition::StillActive
            }
        } else {
            self.state = ConsensusState::Active;
            ConsensusTransition::StillActive
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, ConsensusState::Active)
    }
}
