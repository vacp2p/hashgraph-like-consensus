use alloy::primitives::SignatureError;

#[derive(Debug, thiserror::Error)]
pub enum ConsensusError {
    #[error("Mismatched length: expected {expect}, actual {actual}")]
    MismatchedLength { expect: usize, actual: usize },
    #[error("Invalid vote signature")]
    InvalidVoteSignature,
    #[error("Duplicate vote")]
    DuplicateVote,
    #[error("Empty vote owner")]
    EmptyVoteOwner,
    #[error("Vote expired")]
    VoteExpired,
    #[error("Invalid vote hash")]
    InvalidVoteHash,
    #[error("Empty vote hash")]
    EmptyVoteHash,
    #[error("Invalid proposal configuration: {0}")]
    InvalidProposalConfiguration(String),
    #[error("Vote proposal_id mismatch: vote belongs to different proposal")]
    VoteProposalIdMismatch,
    #[error("Received hash mismatch")]
    ReceivedHashMismatch,
    #[error("Parent hash mismatch")]
    ParentHashMismatch,
    #[error("Invalid vote timestamp")]
    InvalidVoteTimestamp,

    #[error("Session not active")]
    SessionNotActive,
    #[error("Session not found")]
    SessionNotFound,

    #[error("User already voted")]
    UserAlreadyVoted,

    #[error("Proposal already exist in consensus service")]
    ProposalAlreadyExist,

    #[error("Empty signature")]
    EmptySignature,
    #[error("Invalid signature: {0}")]
    InvalidSignature(#[from] SignatureError),
    #[error("Failed to sign message: {0}")]
    FailedToSignMessage(#[from] alloy_signer::Error),
    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    #[error("Consensus failed: {0}")]
    ConsensusFailed(String),
    #[error("Consensus exceeded configured max rounds")]
    MaxRoundsExceeded,
    #[error("Invalid consensus threshold: {0}")]
    InvalidConsensusThreshold(String),

    #[error("Failed to get current time")]
    FailedToGetCurrentTime(#[from] std::time::SystemTimeError),
}
