use std::{collections::HashMap, time::Duration};

use crate::{
    error::ConsensusError,
    protos::consensus::v1::{Proposal, Vote},
    scope_config::{NetworkType, ScopeConfig},
    types::SessionTransition,
    utils::{
        calculate_consensus_result, calculate_max_rounds, current_timestamp, validate_proposal,
        validate_proposal_timestamp, validate_vote, validate_vote_chain,
    },
};

#[derive(Debug, Clone)]
pub struct ConsensusConfig {
    /// What fraction of expected voters must vote before consensus can be reached (default: 2/3).
    consensus_threshold: f64,
    /// How long to wait before timing out if consensus isn't reached.
    consensus_timeout: Duration,
    /// Maximum number of voting rounds (vote increments) before giving up.
    ///
    /// Creation starts at round 1, so this caps the number of votes that can be processed.
    /// Default (gossipsub) is 2 rounds; for P2P flows derive ceil(2n/3) via `ConsensusConfig::p2p()`.
    max_rounds: u32,
    /// Enable automatic two-round limit to mirror gossipsub behavior.
    ///
    /// When true, max_rounds limits the round number (round 1 = owner vote, round 2 = all other votes).
    /// When false, max_rounds limits the vote count (each vote increments round).
    use_gossipsub_rounds: bool,
    /// Whether to apply liveness criteria for silent peers (count silent as YES/NO depending on this flag).
    liveness_criteria: bool,
}

impl From<NetworkType> for ConsensusConfig {
    fn from(network_type: NetworkType) -> Self {
        ConsensusConfig::from(ScopeConfig::from(network_type))
    }
}

impl From<ScopeConfig> for ConsensusConfig {
    fn from(config: ScopeConfig) -> Self {
        let (max_rounds, use_gossipsub_rounds) = match config.network_type {
            NetworkType::Gossipsub => (config.max_rounds_override.unwrap_or(2), true),
            // 0 triggers dynamic calculation for P2P networks
            NetworkType::P2P => (config.max_rounds_override.unwrap_or(0), false),
        };

        ConsensusConfig::new(
            config.default_consensus_threshold,
            config.default_timeout,
            max_rounds,
            use_gossipsub_rounds,
            config.default_liveness_criteria_yes,
        )
    }
}

impl ConsensusConfig {
    /// Default configuration for P2P transport: derive round cap as ceil(2n/3).
    /// Max rounds is 0, so the round cap is calculated dynamically based on the expected voters count.
    pub fn p2p() -> Self {
        ConsensusConfig::from(NetworkType::P2P)
    }

    pub fn gossipsub() -> Self {
        ConsensusConfig::from(NetworkType::Gossipsub)
    }

    /// Set consensus timeout (validated) and return the updated config.
    pub fn with_timeout(mut self, consensus_timeout: Duration) -> Result<Self, ConsensusError> {
        crate::utils::validate_timeout(consensus_timeout)?;
        self.consensus_timeout = consensus_timeout;
        Ok(self)
    }

    /// Set consensus threshold (validated) and return the updated config.
    pub fn with_threshold(mut self, consensus_threshold: f64) -> Result<Self, ConsensusError> {
        crate::utils::validate_threshold(consensus_threshold)?;
        self.consensus_threshold = consensus_threshold;
        Ok(self)
    }

    /// Set liveness criteria and return the updated config.
    pub fn with_liveness_criteria(mut self, liveness_criteria: bool) -> Self {
        self.liveness_criteria = liveness_criteria;
        self
    }

    /// Create a new ConsensusConfig with the given values.
    /// This is used internally for scope configuration conversion.
    pub(crate) fn new(
        consensus_threshold: f64,
        consensus_timeout: Duration,
        max_rounds: u32,
        use_gossipsub_rounds: bool,
        liveness_criteria: bool,
    ) -> Self {
        Self {
            consensus_threshold,
            consensus_timeout,
            max_rounds,
            use_gossipsub_rounds,
            liveness_criteria,
        }
    }

    fn max_round_limit(&self, expected_voters_count: u32) -> u32 {
        if self.use_gossipsub_rounds {
            self.max_rounds
        } else if self.max_rounds == 0 {
            calculate_max_rounds(expected_voters_count, self.consensus_threshold)
        } else {
            self.max_rounds
        }
    }

    pub fn consensus_timeout(&self) -> Duration {
        self.consensus_timeout
    }

    pub fn consensus_threshold(&self) -> f64 {
        self.consensus_threshold
    }

    pub fn liveness_criteria(&self) -> bool {
        self.liveness_criteria
    }

    pub fn max_rounds(&self) -> u32 {
        self.max_rounds
    }

    pub fn use_gossipsub_rounds(&self) -> bool {
        self.use_gossipsub_rounds
    }
}

#[derive(Debug, Clone)]
pub enum ConsensusState {
    /// Votes still accepted.
    Active,
    /// Voting closed with a boolean result.
    ConsensusReached(bool),
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
    fn new(proposal: Proposal, config: ConsensusConfig) -> Self {
        let now = current_timestamp().unwrap_or(0);
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
    pub fn from_proposal(
        proposal: Proposal,
        config: ConsensusConfig,
    ) -> Result<(Self, SessionTransition), ConsensusError> {
        validate_proposal(&proposal)?;

        // Create clean proposal for session (votes will be added via initialize_with_votes)
        let existing_votes = proposal.votes.clone();
        let mut clean_proposal = proposal.clone();
        clean_proposal.votes.clear();
        // Always start with round 1 for new proposals as we at least have the proposal owner's vote.
        clean_proposal.round = 1;

        let mut session = Self::new(clean_proposal, config);
        let transition = session.initialize_with_votes(
            existing_votes,
            proposal.expiration_timestamp,
            proposal.timestamp,
        )?;

        Ok((session, transition))
    }

    /// Add a vote to the session.
    pub(crate) fn add_vote(&mut self, vote: Vote) -> Result<SessionTransition, ConsensusError> {
        match self.state {
            ConsensusState::Active => {
                validate_proposal_timestamp(self.proposal.expiration_timestamp)?;

                // Check if adding this vote would exceed round limits
                self.check_round_limit(1)?;

                if self.votes.contains_key(&vote.vote_owner) {
                    return Err(ConsensusError::DuplicateVote);
                }
                self.votes.insert(vote.vote_owner.clone(), vote.clone());
                self.proposal.votes.push(vote.clone());

                self.update_round(1);
                Ok(self.check_consensus())
            }
            ConsensusState::ConsensusReached(res) => Ok(SessionTransition::ConsensusReached(res)),
            _ => Err(ConsensusError::SessionNotActive),
        }
    }

    /// Initialize session with multiple votes, validating all before adding any.
    /// Validates duplicates, vote chain, and individual votes, then adds all atomically.
    pub(crate) fn initialize_with_votes(
        &mut self,
        votes: Vec<Vote>,
        expiration_timestamp: u64,
        creation_time: u64,
    ) -> Result<SessionTransition, ConsensusError> {
        if !matches!(self.state, ConsensusState::Active) {
            return Err(ConsensusError::SessionNotActive);
        }

        validate_proposal_timestamp(expiration_timestamp)?;

        if votes.is_empty() {
            return Ok(SessionTransition::StillActive);
        }

        let mut seen_owners = std::collections::HashSet::new();
        for vote in &votes {
            if !seen_owners.insert(&vote.vote_owner) {
                return Err(ConsensusError::DuplicateVote);
            }
        }

        validate_vote_chain(&votes)?;
        for vote in &votes {
            validate_vote(vote, expiration_timestamp, creation_time)?;
        }

        self.check_round_limit(votes.len())?;
        self.update_round(votes.len());

        for vote in votes {
            self.votes.insert(vote.vote_owner.clone(), vote.clone());
            self.proposal.votes.push(vote);
        }

        Ok(self.check_consensus())
    }

    /// Check if adding votes would exceed round limits.
    ///
    /// Unifies logic for both single-vote and batch processing:
    /// - For a single vote, pass `vote_count: 1`.
    /// - For P2P: Calculates `(current_round - 1) + vote_count`.
    /// - For Gossipsub: Moves to Round 2 if `vote_count > 0`.
    fn check_round_limit(&mut self, vote_count: usize) -> Result<(), ConsensusError> {
        // Determine the value to compare against the limit based on configuration
        let projected_value = if self.config.use_gossipsub_rounds {
            // Gossipsub Logic:
            // RFC Section 2.5.3: Round 1 = proposal, Round 2 = all parallel votes.
            // If we are already at Round 2, we stay there.
            // If we are at Round 1 and adding ANY votes (> 0), we move to Round 2.
            if self.proposal.round == 2 || (self.proposal.round == 1 && vote_count > 0) {
                2
            } else {
                self.proposal.round // Stays at 1 if vote_count is 0, or handles edge cases
            }
        } else {
            // P2P Logic:
            // RFC Section 2.5.3: Round increments per vote.
            // Current existing votes = round - 1.
            // Projected total = Existing votes + New votes.
            let current_votes = self.proposal.round.saturating_sub(1);
            current_votes.saturating_add(vote_count as u32)
        };

        if projected_value
            > self
                .config
                .max_round_limit(self.proposal.expected_voters_count)
        {
            self.state = ConsensusState::Failed;
            return Err(ConsensusError::MaxRoundsExceeded);
        }

        Ok(())
    }

    /// Update round after adding votes.
    ///
    /// Unifies logic for round updates:
    /// - Gossipsub: Moves from Round 1 -> 2 if adding votes. Stays at 2 otherwise.
    /// - P2P: Adds the number of votes to the current round.
    fn update_round(&mut self, vote_count: usize) {
        if self.config.use_gossipsub_rounds {
            // RFC Section 2.5.3: Gossipsub
            // Round 1 = proposal creation.
            // Round 2 = all subsequent votes.
            // If we are at Round 1 and add ANY votes (>0), we promote to Round 2.
            if self.proposal.round == 1 && vote_count > 0 {
                self.proposal.round = 2;
            }
        } else {
            // RFC Section 2.5.3: P2P
            // Round increments for every vote added.
            self.proposal.round = self.proposal.round.saturating_add(vote_count as u32);
        }
    }

    /// RFC Section 4 (Liveness): Check if consensus reached
    /// - n > 2: need >n/2 YES votes among at least 2n/3 distinct peers
    /// - n ≤ 2: require unanimous YES votes
    /// - Equality: use liveness_criteria_yes
    fn check_consensus(&mut self) -> SessionTransition {
        let expected_voters = self.proposal.expected_voters_count;
        let threshold = self.config.consensus_threshold;
        let liveness = self.proposal.liveness_criteria_yes;

        match calculate_consensus_result(&self.votes, expected_voters, threshold, liveness) {
            Some(result) => {
                self.state = ConsensusState::ConsensusReached(result);
                SessionTransition::ConsensusReached(result)
            }
            None => {
                self.state = ConsensusState::Active;
                SessionTransition::StillActive
            }
        }
    }

    /// Check if this proposal is still accepting votes.
    pub fn is_active(&self) -> bool {
        matches!(self.state, ConsensusState::Active)
    }

    /// Get the consensus result if one has been reached.
    ///
    /// Returns `Ok(true)` for YES, `Ok(false)` for NO, or `Err(ConsensusError::ConsensusNotReached)` if consensus
    /// hasn't been reached yet.
    pub fn get_consensus_result(&self) -> Result<bool, ConsensusError> {
        if let ConsensusState::ConsensusReached(result) = self.state {
            Ok(result)
        } else {
            Err(ConsensusError::ConsensusNotReached)
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy::signers::local::PrivateKeySigner;

    use crate::{
        error::ConsensusError,
        session::{ConsensusConfig, ConsensusSession},
        types::CreateProposalRequest,
        utils::build_vote,
    };

    #[tokio::test]
    async fn enforce_max_rounds_gossipsub() {
        // Gossipsub: max_rounds = 2 means round 1 (proposal) and round 2 (all votes)
        // Should allow multiple votes in round 2, but not exceed round 2
        let signer1 = PrivateKeySigner::random();
        let signer2 = PrivateKeySigner::random();
        let signer3 = PrivateKeySigner::random();
        let signer4 = PrivateKeySigner::random();

        let request = CreateProposalRequest::new(
            "Test".into(),
            "".into(),
            signer1.address().as_slice().to_vec(),
            4, // 4 expected voters
            60,
            false,
        )
        .unwrap();

        let proposal = request.into_proposal().unwrap();
        let config = ConsensusConfig::gossipsub();
        let mut session = ConsensusSession::new(proposal, config);

        // Round 1 -> Round 2 (first vote)
        let vote1 = build_vote(&session.proposal, true, signer1).await.unwrap();
        session.add_vote(vote1).unwrap();
        assert_eq!(session.proposal.round, 2);

        // Stay at round 2 (second vote)
        let vote2 = build_vote(&session.proposal, false, signer2).await.unwrap();
        session.add_vote(vote2).unwrap();
        assert_eq!(session.proposal.round, 2);

        // Stay at round 2 (third vote)
        let vote3 = build_vote(&session.proposal, true, signer3).await.unwrap();
        session.add_vote(vote3).unwrap();
        assert_eq!(session.proposal.round, 2);

        // Stay at round 2 (fourth vote) - should succeed
        let vote4 = build_vote(&session.proposal, true, signer4).await.unwrap();
        session.add_vote(vote4).unwrap();
        assert_eq!(session.proposal.round, 2);
        assert_eq!(session.votes.len(), 4);
    }

    #[tokio::test]
    async fn enforce_max_rounds_p2p() {
        // P2P defaults: max_rounds = 0 triggers dynamic calculation based on expected voters.
        // For threshold=2/3 and expected_voters=5, max_round_limit = ceil(2n/3) = 4 votes.
        // Round 1 = 0 votes, Round 2 = 1 vote, ... Round 5 = 4 votes.
        let signer1 = PrivateKeySigner::random();
        let signer2 = PrivateKeySigner::random();
        let signer3 = PrivateKeySigner::random();
        let signer4 = PrivateKeySigner::random();
        let signer5 = PrivateKeySigner::random();

        let request = CreateProposalRequest::new(
            "Test".into(),
            "".into(),
            signer1.address().as_slice().to_vec(),
            5,
            60,
            false,
        )
        .unwrap();

        let proposal = request.into_proposal().unwrap();
        let config = ConsensusConfig::p2p();
        let mut session = ConsensusSession::new(proposal, config);

        // Round 1 -> Round 2 (first vote, 1 vote total)
        let vote1 = build_vote(&session.proposal, true, signer1).await.unwrap();
        session.add_vote(vote1).unwrap();
        assert_eq!(session.proposal.round, 2);
        assert_eq!(session.votes.len(), 1);

        // Round 2 -> Round 3 (second vote, 2 votes total) - should succeed
        let vote2 = build_vote(&session.proposal, false, signer2).await.unwrap();
        session.add_vote(vote2).unwrap();
        assert_eq!(session.proposal.round, 3);
        assert_eq!(session.votes.len(), 2);

        // Round 3 -> Round 4 (third vote, 3 votes total) - should succeed
        let vote3 = build_vote(&session.proposal, true, signer3).await.unwrap();
        session.add_vote(vote3).unwrap();
        assert_eq!(session.proposal.round, 4);
        assert_eq!(session.votes.len(), 3);

        // Round 4 -> Round 5 (fourth vote, 4 votes total) - should succeed (dynamic limit = 4)
        let vote4 = build_vote(&session.proposal, true, signer4).await.unwrap();
        session.add_vote(vote4).unwrap();
        assert_eq!(session.proposal.round, 5);
        assert_eq!(session.votes.len(), 4);

        // Fifth vote would exceed dynamic max_round_limit (=4 votes)
        let vote5 = build_vote(&session.proposal, true, signer5).await.unwrap();
        let err = session.add_vote(vote5).unwrap_err();
        assert!(matches!(err, ConsensusError::MaxRoundsExceeded));
    }
}
