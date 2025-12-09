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

const SIGNATURE_LENGTH: usize = 65;

/// Generate a unique 32-bit ID from a UUID.
pub fn generate_id() -> u32 {
    let uuid = Uuid::new_v4();
    uuid.as_u128() as u32
}

/// Compute the hash of a vote for signing and validation.
///
/// This creates a deterministic hash from all the vote's fields (ID, owner, proposal ID,
/// timestamp, vote choice, and parent/received hashes). Everyone computes the same hash
/// for the same vote, which is important for verification.
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

/// Create a new vote for a proposal with proper hash chain linking.
///
/// This builds a vote that links to previous votes in the hashgraph structure.
/// The vote is signed with the provided signer and includes all the necessary
/// fields for validation (parent_hash, received_hash, vote_hash, signature).
pub async fn build_vote<S: Signer + Sync>(
    proposal: &Proposal,
    user_vote: bool,
    signer: S,
) -> Result<Vote, ConsensusError> {
    let now = current_timestamp()?;

    let voter_address = signer.address().as_slice().to_vec();
    // RFC Section 2.2: Define `parent_hash` as hash of previous owner's vote (empty if none).
    // RFC Section 2.3: Set `received_hash` to hash of immediately previous vote (last vote in list).
    let (parent_hash, received_hash) = if let Some(latest_vote) = proposal.votes.last() {
        let own_last_vote = proposal
            .votes
            .iter()
            .rfind(|v| v.vote_owner == voter_address);

        if let Some(own_vote) = own_last_vote {
            (own_vote.vote_hash.clone(), latest_vote.vote_hash.clone())
        } else {
            (Vec::new(), latest_vote.vote_hash.clone())
        }
    } else {
        (Vec::new(), Vec::new())
    };

    let vote_id = generate_id();

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
    let signature = signer.sign_message(&vote_bytes).await?;
    vote.signature = signature.as_bytes().to_vec();
    Ok(vote)
}

/// Verify that a vote's signature is valid and matches the vote owner.
///
/// Checks that the signature was created by the owner's private key and that
/// it signs the correct vote data. Returns `true` if valid, `false` otherwise.
pub fn verify_vote_hash(
    signature: &[u8],
    public_key: &[u8],
    message: &[u8],
) -> Result<bool, ConsensusError> {
    let signature_bytes: [u8; SIGNATURE_LENGTH] =
        signature
            .try_into()
            .map_err(|_| ConsensusError::MismatchedLength {
                expect: SIGNATURE_LENGTH,
                actual: signature.len(),
            })?;
    let signature = Signature::from_raw_array(&signature_bytes)?;
    let address = signature.recover_address_from_msg(message)?;
    let address_bytes = address.as_slice().to_vec();
    Ok(address_bytes == public_key)
}

/// Validate a proposal and all its votes.
///
/// Checks that the proposal hasn't expired.
/// Also validates that all votes belong to this proposal, vote signatures are valid,
/// and the vote chain (parent_hash/received_hash) is correct.
/// Should be called when receiving a proposal from the network.
pub fn validate_proposal(proposal: &Proposal) -> Result<(), ConsensusError> {
    validate_proposal_timestamp(proposal.expiration_timestamp)?;

    for vote in proposal.votes.iter() {
        if vote.proposal_id != proposal.proposal_id {
            return Err(ConsensusError::VoteProposalIdMismatch);
        }
        validate_vote(vote, proposal.expiration_timestamp, proposal.timestamp)?;
    }
    validate_vote_chain(&proposal.votes)?;
    Ok(())
}

/// Validate a single vote.
///
/// RFC Section 3.4: Validates timestamps (reject future timestamps and votes older than 1 hour).
/// Also checks that the vote hash is correct, the signature is valid, and the vote hasn't expired.
/// This prevents replay attacks and ensures vote integrity.
pub fn validate_vote(
    vote: &Vote,
    expiration_timestamp: u64,
    creation_time: u64,
) -> Result<(), ConsensusError> {
    if vote.vote_owner.is_empty() {
        return Err(ConsensusError::EmptyVoteOwner);
    }

    if vote.vote_hash.is_empty() {
        return Err(ConsensusError::EmptyVoteHash);
    }

    if vote.signature.is_empty() {
        return Err(ConsensusError::EmptySignature);
    }

    if vote.signature.len() != SIGNATURE_LENGTH {
        return Err(ConsensusError::MismatchedLength {
            expect: SIGNATURE_LENGTH,
            actual: vote.signature.len(),
        });
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

    let now = current_timestamp()?;

    // RFC Section 3.4:  Check the `timestamp` against the replay attack.
    // In particular, the `timestamp` cannot be the old in the determined threshold.
    if vote.timestamp < creation_time {
        return Err(ConsensusError::TimestampOlderThanCreationTime);
    }

    if vote.timestamp > expiration_timestamp || now > expiration_timestamp {
        return Err(ConsensusError::VoteExpired);
    }

    Ok(())
}

/// Validate that votes form a correct hashgraph chain.
/// RFC Section 2.2 and 2.3.
pub fn validate_vote_chain(votes: &[Vote]) -> Result<(), ConsensusError> {
    if votes.len() <= 1 {
        return Ok(());
    }

    let mut hash_index: HashMap<&[u8], (&[u8], u64, usize)> = HashMap::new();
    for (idx, vote) in votes.iter().enumerate() {
        hash_index.insert(&vote.vote_hash, (&vote.vote_owner, vote.timestamp, idx));
    }

    for (idx, vote) in votes.iter().enumerate() {
        // RFC Section 2.3: If there are multiple votes in a proposal,
        // check that the hash of a vote is equal to the `received_hash` of the next one.
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

        // RFC Section 2.2: If there are repeated votes from the same sender,
        // check that the hash of the former vote is equal to the `parent_hash` of the later vote.
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

/// Calculate the consensus result from collected votes.
///
/// RFC Section 4 (Liveness): Determines consensus based on vote counts and liveness criteria.
/// Returns `true` if YES wins, `false` if NO wins. If votes are tied, uses
/// `liveness_criteria_yes` as the tie-breaker (RFC Section 4: Equality of votes).
pub fn calculate_consensus_result(
    votes: &HashMap<Vec<u8>, Vote>,
    expected_voters: u32,
    consensus_threshold: f64,
    liveness_criteria_yes: bool,
) -> Option<bool> {
    let total_votes = votes.len() as u32;
    let yes_votes = votes.values().filter(|v| v.vote).count() as u32;
    let no_votes = total_votes.saturating_sub(yes_votes);
    let silent_votes = expected_voters.saturating_sub(total_votes);

    if expected_voters <= 2 {
        if total_votes < expected_voters {
            return None;
        }
        return Some(yes_votes == expected_voters);
    }

    let required_votes = calculate_required_votes(expected_voters, consensus_threshold);
    if total_votes < required_votes {
        return None;
    }

    let required_choice_votes =
        calculate_threshold_based_value(expected_voters, consensus_threshold);
    let yes_weight = yes_votes
        + if liveness_criteria_yes {
            silent_votes
        } else {
            0
        };
    let no_weight = no_votes
        + if liveness_criteria_yes {
            0
        } else {
            silent_votes
        };

    if yes_weight >= required_choice_votes && yes_weight > no_weight {
        return Some(true);
    }

    if no_weight >= required_choice_votes && no_weight > yes_weight {
        return Some(false);
    }

    if total_votes == expected_voters && yes_weight == no_weight {
        return Some(liveness_criteria_yes);
    }

    None
}

pub fn calculate_required_votes(expected_voters: u32, consensus_threshold: f64) -> u32 {
    // RFC Section 4: For n ≤ 2, require all votes. For n > 2, use threshold (default 2n/3)
    if expected_voters <= 2 {
        expected_voters
    } else {
        calculate_threshold_based_value(expected_voters, consensus_threshold)
    }
}

pub fn calculate_max_rounds(expected_voters: u32, consensus_threshold: f64) -> u32 {
    calculate_threshold_based_value(expected_voters, consensus_threshold)
}

/// Calculate a value based on threshold (shared logic for required votes and max rounds).
fn calculate_threshold_based_value(expected_voters: u32, consensus_threshold: f64) -> u32 {
    if (consensus_threshold - (2.0 / 3.0)).abs() < f64::EPSILON {
        (2 * expected_voters).div_ceil(3)
    } else {
        ((expected_voters as f64) * consensus_threshold).ceil() as u32
    }
}

pub(crate) fn current_timestamp() -> Result<u64, ConsensusError> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    Ok(now)
}

/// Check if a proposal has expired.
///
/// RFC Section 2.5.4: Verifies that the proposal has not expired by checking that
/// the current time is less than the expiration timestamp.
/// Returns an error if the proposal has expired.
pub fn validate_proposal_timestamp(expiration_timestamp: u64) -> Result<(), ConsensusError> {
    let now = current_timestamp()?;
    if now >= expiration_timestamp {
        return Err(ConsensusError::ProposalExpired);
    }
    Ok(())
}

/// Validate that a consensus threshold is in the valid range [0.0, 1.0].
pub fn validate_threshold(threshold: f64) -> Result<(), ConsensusError> {
    if !(0.0..=1.0).contains(&threshold) {
        return Err(ConsensusError::InvalidConsensusThreshold);
    }
    Ok(())
}

/// Validate that a timeout is greater than 0.
pub fn validate_timeout(timeout: u64) -> Result<(), ConsensusError> {
    if timeout == 0 {
        return Err(ConsensusError::InvalidTimeout);
    }
    Ok(())
}

pub fn validate_expected_voters_count(expected_voters_count: u32) -> Result<(), ConsensusError> {
    if expected_voters_count == 0 {
        return Err(ConsensusError::InvalidExpectedVotersCount);
    }
    Ok(())
}

/// Check if enough votes have been collected to potentially reach consensus.
///
/// This checks if the vote count meets the threshold, but doesn't determine the actual
/// result. You still need to check if YES or NO has a majority.
pub fn has_sufficient_votes(
    total_votes: u32,
    expected_voters: u32,
    consensus_threshold: f64,
) -> bool {
    let required_votes = calculate_required_votes(expected_voters, consensus_threshold);
    total_votes >= required_votes
}
