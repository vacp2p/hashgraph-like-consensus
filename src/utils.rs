use alloy_signer::{Signature, Signer};
use prost::Message;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

use crate::{
    error::ConsensusError,
    protos::consensus::v1::{Proposal, Vote},
};

/// Generate 32-bit ID from UUID using first 4 bytes with bit manipulation to avoid truncation collisions.
pub fn generate_proposal_id() -> u32 {
    let uuid = Uuid::new_v4();
    let uuid_bytes = uuid.as_bytes();
    ((uuid_bytes[0] as u32) << 24)
        | ((uuid_bytes[1] as u32) << 16)
        | ((uuid_bytes[2] as u32) << 8)
        | (uuid_bytes[3] as u32)
}

/// Generate 32-bit ID from UUID using first 4 bytes with bit manipulation to avoid truncation collisions.
pub fn generate_vote_id() -> u32 {
    let uuid = Uuid::new_v4();
    let uuid_bytes = uuid.as_bytes();
    ((uuid_bytes[0] as u32) << 24)
        | ((uuid_bytes[1] as u32) << 16)
        | ((uuid_bytes[2] as u32) << 8)
        | (uuid_bytes[3] as u32)
}

pub fn compute_vote_hash(vote: &Vote) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(vote.vote_id.to_le_bytes());
    hasher.update(&vote.vote_owner);
    hasher.update(vote.proposal_id.to_le_bytes());
    hasher.update(vote.timestamp.to_le_bytes());
    hasher.update([vote.vote as u8]);
    hasher.update(&vote.parent_hash);
    hasher.update(&vote.received_hash);
    hasher.finalize().to_vec()
}

pub async fn create_vote_for_proposal<S: Signer + Sync>(
    proposal: &Proposal,
    user_vote: bool,
    signer: S,
) -> Result<Vote, ConsensusError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let voter_address = signer.address().as_slice().to_vec();
    let (parent_hash, received_hash) = if let Some(latest_vote) = proposal.votes.last() {
        // RFC Section 2.4: Find voter's own last vote for parent_hash (may have other votes in between)
        let own_last_vote = proposal
            .votes
            .iter()
            .rev()
            .find(|v| v.vote_owner == voter_address);

        if let Some(own_vote) = own_last_vote {
            (own_vote.vote_hash.clone(), latest_vote.vote_hash.clone())
        } else {
            (Vec::new(), latest_vote.vote_hash.clone())
        }
    } else {
        (Vec::new(), Vec::new())
    };

    let vote_id = generate_vote_id();

    let mut vote = Vote {
        vote_id,
        vote_owner: signer.address().as_slice().to_vec(),
        proposal_id: proposal.proposal_id,
        timestamp: now,
        vote: user_vote,
        parent_hash,
        received_hash,
        vote_hash: Vec::new(),
        signature: Vec::new(),
    };

    vote.vote_hash = compute_vote_hash(&vote);
    let vote_bytes = vote.encode_to_vec();
    let signature = signer
        .sign_message(&vote_bytes)
        .await
        .map_err(|e| ConsensusError::InvalidSignature(e.to_string()))?;
    vote.signature = signature.as_bytes().to_vec();
    Ok(vote)
}

pub fn verify_vote_hash(
    signature: &[u8],
    public_key: &[u8],
    message: &[u8],
) -> Result<bool, ConsensusError> {
    let signature_bytes: [u8; 65] =
        signature
            .try_into()
            .map_err(|_| ConsensusError::MismatchedLength {
                expect: 65,
                actual: signature.len(),
            })?;
    let signature = Signature::from_raw_array(&signature_bytes)
        .map_err(|e| ConsensusError::InvalidSignature(e.to_string()))?;
    let address = signature
        .recover_address_from_msg(message)
        .map_err(|e| ConsensusError::InvalidAddress(e.to_string()))?;
    let address_bytes = address.as_slice().to_vec();
    Ok(address_bytes == public_key)
}

pub fn validate_proposal(proposal: &Proposal) -> Result<(), ConsensusError> {
    // RFC Section 2.5.4
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    if now >= proposal.expiration_time {
        return Err(ConsensusError::VoteExpired);
    }

    for vote in proposal.votes.iter() {
        if vote.proposal_id != proposal.proposal_id {
            return Err(ConsensusError::VoteProposalIdMismatch);
        }
        validate_vote(vote, proposal.expiration_time)?;
    }
    validate_vote_chain(&proposal.votes)?;
    Ok(())
}

pub fn validate_vote(vote: &Vote, expiration_time: u64) -> Result<(), ConsensusError> {
    if vote.vote_owner.is_empty() {
        return Err(ConsensusError::EmptyVoteOwner);
    }

    if vote.vote_hash.is_empty() {
        return Err(ConsensusError::EmptyVoteHash);
    }

    if vote.signature.is_empty() {
        return Err(ConsensusError::EmptySignature);
    }

    let expected_hash = compute_vote_hash(vote);
    if vote.vote_hash != expected_hash {
        return Err(ConsensusError::InvalidVoteHash);
    }

    let mut vote_copy = vote.clone();
    vote_copy.signature = Vec::new();
    let vote_copy_bytes = vote_copy.encode_to_vec();

    let verified = verify_vote_hash(&vote.signature, &vote.vote_owner, &vote_copy_bytes)?;

    if !verified {
        return Err(ConsensusError::InvalidVoteSignature);
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

    // RFC Section 3.4: Reject future timestamps and votes older than 1 hour (replay attack protection)
    if vote.timestamp > now {
        return Err(ConsensusError::InvalidVoteTimestamp);
    }
    const MAX_VOTE_AGE_SECONDS: u64 = 3600;
    if now.saturating_sub(vote.timestamp) > MAX_VOTE_AGE_SECONDS {
        return Err(ConsensusError::InvalidVoteTimestamp);
    }

    if vote.timestamp > expiration_time || now > expiration_time {
        return Err(ConsensusError::VoteExpired);
    }

    Ok(())
}

pub fn validate_vote_chain(votes: &[Vote]) -> Result<(), ConsensusError> {
    if votes.len() <= 1 {
        return Ok(());
    }

    let mut hash_index: HashMap<&[u8], (&[u8], u64, usize)> = HashMap::new();
    for (idx, vote) in votes.iter().enumerate() {
        hash_index.insert(&vote.vote_hash, (&vote.vote_owner, vote.timestamp, idx));
    }

    // RFC Section 2.3: received_hash must point to immediately previous vote
    for (idx, vote) in votes.iter().enumerate() {
        if idx > 0 {
            let prev_vote = &votes[idx - 1];
            if !vote.received_hash.is_empty() {
                if vote.received_hash != prev_vote.vote_hash {
                    return Err(ConsensusError::ReceivedHashMismatch);
                }
                if prev_vote.timestamp > vote.timestamp {
                    return Err(ConsensusError::ReceivedHashMismatch);
                }
            }
        }

        // RFC Section 2.3: parent_hash must point to voter's own previous vote (may have other votes in between)
        if !vote.parent_hash.is_empty() {
            match hash_index.get(&vote.parent_hash.as_slice()) {
                Some((owner, ts, parent_idx))
                    if *owner == vote.vote_owner.as_slice()
                        && *ts <= vote.timestamp
                        && *parent_idx < idx => {}
                Some(_) => return Err(ConsensusError::ParentHashMismatch),
                None => return Err(ConsensusError::ParentHashMismatch),
            }
        }
    }

    Ok(())
}

pub fn calculate_consensus_result(
    votes: &HashMap<Vec<u8>, Vote>,
    liveness_criteria_yes: bool,
) -> bool {
    let total_votes = votes.len() as u32;
    let yes_votes = votes.values().filter(|v| v.vote).count() as u32;
    let no_votes = total_votes - yes_votes;

    if yes_votes > no_votes {
        true
    } else if no_votes > yes_votes {
        false
    } else {
        liveness_criteria_yes
    }
}

pub fn calculate_required_votes(expected_voters: u32, consensus_threshold: f64) -> u32 {
    // RFC Section 4: For n ≤ 2, require all votes. For n > 2, use threshold (default 2n/3)
    if expected_voters <= 2 {
        expected_voters
    } else if (consensus_threshold - (2.0 / 3.0)).abs() < f64::EPSILON {
        (2 * expected_voters).div_ceil(3)
    } else {
        ((expected_voters as f64) * consensus_threshold).ceil() as u32
    }
}

pub fn check_sufficient_votes(
    total_votes: u32,
    expected_voters: u32,
    consensus_threshold: f64,
) -> bool {
    let required_votes = calculate_required_votes(expected_voters, consensus_threshold);
    total_votes >= required_votes
}
