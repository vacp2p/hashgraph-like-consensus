use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    error::ConsensusError,
    protos::consensus::v1::{Proposal, Vote},
    utils::{
        calculate_required_votes, generate_id, validate_proposal, validate_vote,
        validate_vote_chain,
    },
};

#[derive(Debug, Clone)]
pub enum ConsensusEvent {
    /// Consensus was reached! The proposal has a final result (yes or no).
    ConsensusReached { proposal_id: u32, result: bool },
    /// Consensus failed - not enough votes were collected before the timeout.
    ConsensusFailed { proposal_id: u32, reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsensusTransition {
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
    /// How long until voting expires, in seconds from creation time.
    pub expiration_time: u64,
    /// What happens if votes are tied: `true` means YES wins, `false` means NO wins.
    pub liveness_criteria_yes: bool,
}

impl CreateProposalRequest {
    /// Create a new proposal request with validation.
    ///
    /// Returns an error if `expected_voters_count` is zero.
    pub fn new(
        name: String,
        payload: String,
        proposal_owner: Vec<u8>,
        expected_voters_count: u32,
        expiration_time: u64,
        liveness_criteria_yes: bool,
    ) -> Result<Self, ConsensusError> {
        if expected_voters_count == 0 {
            return Err(ConsensusError::InvalidProposalConfiguration(
                "expected_voters_count must be greater than 0".to_string(),
            ));
        }
        let request = Self {
            name,
            payload,
            proposal_owner,
            expected_voters_count,
            expiration_time,
            liveness_criteria_yes,
        };
        Ok(request)
    }

    /// Convert this request into an actual proposal.
    ///
    /// Generates a unique proposal ID and sets the creation timestamp. The proposal
    /// starts with round 1 and no votes - votes will be added as people participate.
    pub fn into_proposal(self) -> Result<Proposal, ConsensusError> {
        let proposal_id = generate_id();
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        Ok(Proposal {
            name: self.name,
            payload: self.payload,
            proposal_id,
            proposal_owner: self.proposal_owner,
            votes: vec![],
            expected_voters_count: self.expected_voters_count,
            round: 1,
            timestamp: now,
            expiration_time: now + self.expiration_time,
            liveness_criteria_yes: self.liveness_criteria_yes,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ConsensusConfig {
    /// What fraction of expected voters must vote before consensus can be reached (default: 2/3).
    pub consensus_threshold: f64,
    /// How long to wait (in seconds) before timing out if consensus isn't reached.
    pub consensus_timeout: u64,
    /// Maximum number of voting rounds before giving up (not currently enforced).
    pub max_rounds: u32,
    /// Whether to apply liveness criteria for peers that don't vote (not currently used).
    pub liveness_criteria: bool,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            consensus_threshold: 2.0 / 3.0, // RFC Section 4: 2n/3 threshold
            consensus_timeout: 10,
            max_rounds: 3,
            liveness_criteria: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConsensusState {
    /// Votes still accepted.
    Active,
    /// Voting closed with a boolean result.
    ConsensusReached(bool),
    /// Proposal expired before reaching consensus.
    Expired,
    /// Consensus could not be determined (typically on timeout with insufficient votes).
    Failed,
}

#[derive(Debug, Clone)]
pub struct ConsensusSession {
    /// Current snapshot of the proposal including aggregated votes.
    pub proposal: Proposal,
    /// Session state tracking whether voting is still open.
    pub state: ConsensusState,
    /// Map of vote owner -> vote to enforce single vote per participant.
    pub votes: HashMap<Vec<u8>, Vote>, // vote_owner -> Vote
    /// Seconds since Unix epoch when the session was created.
    pub created_at: u64,
    /// Per-session runtime configuration.
    pub config: ConsensusConfig,
}

impl ConsensusSession {
    /// Create a new session from a validated proposal (no votes).
    /// Used when creating proposals locally where we know the proposal is clean.
    pub(crate) fn new(proposal: Proposal, config: ConsensusConfig) -> Self {
        // Fallback to 0 if system time is before UNIX_EPOCH (should never happen)
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs();

        Self {
            proposal,
            state: ConsensusState::Active,
            votes: HashMap::new(),
            created_at: now,
            config,
        }
    }

    /// Create a session from a proposal, validating the proposal and all votes.
    /// This validates the proposal structure, vote chain, and individual votes before creating the session.
    /// The session is created with votes already processed and rounds correctly set.
    pub(crate) fn from_proposal(
        proposal: Proposal,
        config: ConsensusConfig,
    ) -> Result<(Self, ConsensusTransition), ConsensusError> {
        validate_proposal(&proposal)?;

        // Create clean proposal for session (votes will be added via initialize_with_votes)
        // RFC Section 1: Proposals start with round = 1 (proposal creation)
        let existing_votes = proposal.votes.clone();
        let mut clean_proposal = proposal.clone();
        clean_proposal.votes.clear();
        clean_proposal.round = 1;

        let mut session = Self::new(clean_proposal, config);
        let transition = session.initialize_with_votes(existing_votes, proposal.expiration_time)?;

        Ok((session, transition))
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
                // RFC Section 2.5.3
                self.proposal.round += 1;
                Ok(self.check_consensus())
            }
            ConsensusState::ConsensusReached(res) => Ok(ConsensusTransition::ConsensusReached(res)),
            _ => Err(ConsensusError::SessionNotActive),
        }
    }

    /// Initialize session with multiple votes, validating all before adding any.
    /// Validates duplicates, vote chain, and individual votes, then adds all atomically.
    pub(crate) fn initialize_with_votes(
        &mut self,
        votes: Vec<Vote>,
        expiration_time: u64,
    ) -> Result<ConsensusTransition, ConsensusError> {
        if !matches!(self.state, ConsensusState::Active) {
            return Err(ConsensusError::SessionNotActive);
        }

        if votes.is_empty() {
            return Ok(ConsensusTransition::StillActive);
        }

        let mut seen_owners = std::collections::HashSet::new();
        for vote in &votes {
            if !seen_owners.insert(&vote.vote_owner) {
                return Err(ConsensusError::DuplicateVote);
            }
        }

        validate_vote_chain(&votes)?;
        for vote in &votes {
            validate_vote(vote, expiration_time)?;
        }

        // RFC Section 1: Proposals start with round = 1 (proposal creation)
        // RFC Section 2.5.3: Round increments for each vote
        // So final round = 1 (creation) + vote_count
        self.proposal.round = 1;
        for vote in votes {
            self.votes.insert(vote.vote_owner.clone(), vote.clone());
            self.proposal.votes.push(vote);
            self.proposal.round += 1;
        }

        Ok(self.check_consensus())
    }

    /// RFC Section 4 (Liveness): Check if consensus reached
    /// - n > 2: need >n/2 YES votes among at least 2n/3 distinct peers
    /// - n ≤ 2: require unanimous YES votes
    /// - Equality: use liveness_criteria_yes
    fn check_consensus(&mut self) -> ConsensusTransition {
        let total_votes = self.votes.len() as u32;
        let yes_votes = self.votes.values().filter(|v| v.vote).count() as u32;
        let no_votes = total_votes - yes_votes;

        let expected_voters = self.proposal.expected_voters_count;
        let required_votes = calculate_required_votes(
            self.proposal.expected_voters_count,
            self.config.consensus_threshold,
        );

        if total_votes >= required_votes {
            if expected_voters <= 2 {
                // RFC Section 4: n ≤ 2 requires unanimous YES
                if yes_votes == expected_voters && total_votes == expected_voters {
                    self.state = ConsensusState::ConsensusReached(true);
                    return ConsensusTransition::ConsensusReached(true);
                } else if total_votes == expected_voters {
                    self.state = ConsensusState::ConsensusReached(false);
                    return ConsensusTransition::ConsensusReached(false);
                }
            } else {
                // RFC Section 4: n > 2 requires >n/2 YES votes
                let half_voters = expected_voters / 2;
                if yes_votes > half_voters {
                    self.state = ConsensusState::ConsensusReached(true);
                    return ConsensusTransition::ConsensusReached(true);
                } else if no_votes > half_voters {
                    self.state = ConsensusState::ConsensusReached(false);
                    return ConsensusTransition::ConsensusReached(false);
                } else if total_votes == expected_voters {
                    // RFC Section 4: Equality - use liveness criteria
                    self.state =
                        ConsensusState::ConsensusReached(self.proposal.liveness_criteria_yes);
                    return ConsensusTransition::ConsensusReached(
                        self.proposal.liveness_criteria_yes,
                    );
                }
            }
        }

        self.state = ConsensusState::Active;
        ConsensusTransition::StillActive
    }

    /// Check if this proposal is still accepting votes.
    pub fn is_active(&self) -> bool {
        matches!(self.state, ConsensusState::Active)
    }

    /// Get the consensus result if one has been reached.
    ///
    /// Returns `Some(true)` for YES, `Some(false)` for NO, or `None` if consensus
    /// hasn't been reached yet.
    pub fn is_reached(&self) -> Option<bool> {
        match self.state {
            ConsensusState::ConsensusReached(result) => Some(result),
            _ => None,
        }
    }
}
