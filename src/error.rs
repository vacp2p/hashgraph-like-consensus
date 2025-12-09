use alloy::primitives::SignatureError;

#[derive(Debug, thiserror::Error)]
pub enum ConsensusError {
    // Configuration Validation Errors
    #[error("consensus_threshold must be between 0.0 and 1.0")]
    InvalidConsensusThreshold,
    #[error("timeout must be greater than 0")]
    InvalidTimeout,
    #[error("expected_voters_count must be greater than 0")]
    InvalidExpectedVotersCount,
    #[error("max_rounds must be greater than 0")]
    InvalidMaxRounds,

    // Vote Validation Errors
    #[error("Invalid vote signature")]
    InvalidVoteSignature,
    #[error("Empty signature")]
    EmptySignature,
    #[error("Duplicate vote")]
    DuplicateVote,
    #[error("User already voted")]
    UserAlreadyVoted,
    #[error("Vote expired")]
    VoteExpired,
    #[error("Empty vote owner")]
    EmptyVoteOwner,
    #[error("Invalid vote hash")]
    InvalidVoteHash,
    #[error("Empty vote hash")]
    EmptyVoteHash,
    #[error("Vote proposal_id mismatch: vote belongs to different proposal")]
    VoteProposalIdMismatch,
    #[error("Received hash mismatch")]
    ReceivedHashMismatch,
    #[error("Parent hash mismatch")]
    ParentHashMismatch,
    #[error("Invalid vote timestamp")]
    InvalidVoteTimestamp,
    #[error("Vote timestamp is older than creation time")]
    TimestampOlderThanCreationTime,
    #[error("Mismatched length: expected {expect}, actual {actual}")]
    MismatchedLength { expect: usize, actual: usize },

    // Session/State Errors
    #[error("Session not active")]
    SessionNotActive,
    #[error("Session not found")]
    SessionNotFound,
    #[error("Proposal already exist in consensus service")]
    ProposalAlreadyExist,

    // Consensus Result Errors
    #[error("Insufficient votes at timeout")]
    InsufficientVotesAtTimeout,
    #[error("Consensus exceeded configured max rounds")]
    MaxRoundsExceeded,

    #[error("Invalid signature: {0}")]
    InvalidSignature(#[from] SignatureError),
    #[error("Failed to sign message: {0}")]
    FailedToSignMessage(#[from] alloy_signer::Error),
    #[error("Failed to get current time")]
    FailedToGetCurrentTime(#[from] std::time::SystemTimeError),
}
