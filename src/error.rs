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
    #[error("Received hash mismatch")]
    ReceivedHashMismatch,
    #[error("Parent hash mismatch")]
    ParentHashMismatch,
    #[error("Invalid vote timestamp")]
    InvalidVoteTimestamp,

    #[error("Session not active")]
    SessionNotActive,
    #[error("Group not found")]
    GroupNotFound,
    #[error("Session not found")]
    SessionNotFound,

    #[error("User already voted")]
    UserAlreadyVoted,

    #[error("Proposal already exist in consensus service")]
    ProposalAlreadyExist,

    #[error("Empty signature")]
    EmptySignature,
    #[error("Invalid signature: {0}")]
    InvalidSignature(String),

    #[error("Failed to get current time")]
    FailedToGetCurrentTime(#[from] std::time::SystemTimeError),
}
