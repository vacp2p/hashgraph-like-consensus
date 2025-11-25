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

    let (parent_hash, received_hash) = if let Some(latest_vote) = proposal.votes.last() {
        let is_same_voter = latest_vote.vote_owner == signer.address().as_slice().to_vec();
        if is_same_voter {
            // Same voter: parent_hash should be the hash of our previous vote
            (latest_vote.vote_hash.clone(), Vec::new())
        } else {
            // Different voter: parent_hash is empty, received_hash is the hash of the latest vote
            (Vec::new(), latest_vote.vote_hash.clone())
        }
    } else {
        (Vec::new(), Vec::new())
    };

    let mut vote = Vote {
        vote_id: Uuid::new_v4().as_u128() as u32,
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
        .map_err(|e| ConsensusError::InvalidSignature(e.to_string()))?;
    let address_bytes = address.as_slice().to_vec();
    Ok(address_bytes == public_key)
}

pub fn validate_proposal(proposal: &Proposal) -> Result<(), ConsensusError> {
    for vote in proposal.votes.iter() {
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

    if vote.timestamp > now {
        return Err(ConsensusError::InvalidVoteTimestamp);
    }

    if now - vote.timestamp > expiration_time {
        return Err(ConsensusError::VoteExpired);
    }

    Ok(())
}

fn validate_vote_chain(votes: &[Vote]) -> Result<(), ConsensusError> {
    if votes.len() <= 1 {
        return Ok(());
    }

    for i in 0..votes.len() - 1 {
        let current_vote = &votes[i];
        let next_vote = &votes[i + 1];

        if current_vote.vote_hash != next_vote.received_hash {
            return Err(ConsensusError::ReceivedHashMismatch);
        }

        if current_vote.vote_owner == next_vote.vote_owner
            && current_vote.vote_hash != next_vote.parent_hash
        {
            return Err(ConsensusError::ParentHashMismatch);
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

fn calculate_required_votes(expected_voters: u32, consensus_threshold: f64) -> u32 {
    if expected_voters == 1 || expected_voters == 2 {
        expected_voters
    } else {
        ((expected_voters as f64) * consensus_threshold) as u32
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
