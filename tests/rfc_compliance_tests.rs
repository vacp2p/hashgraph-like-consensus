use alloy::signers::{Signer, local::PrivateKeySigner};
use prost::Message;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

use hashgraph_like_consensus::{
    error::ConsensusError, scope::ScopeID, service::DefaultConsensusService,
    session::CreateProposalRequest, utils::create_vote_for_proposal,
};

const SCOPE: &str = "rfc_compliance_scope";
const PROPOSAL_NAME: &str = "RFC Compliance Test";
const PROPOSAL_PAYLOAD: &str = "";

const EXPIRATION: u64 = 120;
const EXPIRATION_WAIT_TIME: u64 = 100;

const EXPIRATION_1_SECOND: u64 = 1;
const EXPIRATION_WAIT_TIME_2_SECOND: u64 = 2;

const EXPECTED_VOTERS_COUNT_3: u32 = 3;
const EXPECTED_VOTERS_COUNT_2: u32 = 2;
const EXPECTED_VOTERS_COUNT_4: u32 = 4;
const EXPECTED_VOTERS_COUNT_1: u32 = 1;

const VOTE_YES: bool = true;
const VOTE_NO: bool = false;

fn owner_bytes(signer: &PrivateKeySigner) -> Vec<u8> {
    signer.address().as_slice().to_vec()
}

/// RFC Section 1: Test that proposal initialization has round = 1
#[tokio::test]
async fn test_proposal_initialization_round_is_one() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    assert_eq!(
        proposal.round, 1,
        "RFC Section 1: Proposal should start with round = 1"
    );
}

/// RFC Section 2.5.3: Test that round increments when votes are added
#[tokio::test]
async fn test_round_increments_on_vote() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    assert_eq!(proposal.round, 1);

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("vote should be added");

    assert_eq!(
        proposal.round, 2,
        "RFC Section 2.5.3: Round should increment when vote is added"
    );

    let voter2 = PrivateKeySigner::random();
    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, voter2)
        .await
        .expect("second vote should be added");

    assert_eq!(proposal.round, 3, "Round should increment again");
}

/// RFC Section 4: Test that n ≤ 2 requires unanimous YES votes
#[tokio::test]
async fn test_n_le_2_requires_unanimous_yes() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                EXPECTED_VOTERS_COUNT_1,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("vote should be added");

    // Should reach consensus immediately with unanimous YES
    let result = service
        .get_consensus_result(&scope, proposal.proposal_id)
        .await;
    assert_eq!(
        result,
        Some(true),
        "RFC Section 4: n=1 requires unanimous YES"
    );

    // Test with n = 2, both YES
    let scope2 = ScopeID::from("scope2");
    let proposal_owner2 = PrivateKeySigner::random();
    let proposal2 = service
        .create_proposal(
            &scope2,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner2),
                EXPECTED_VOTERS_COUNT_2,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    let proposal2 = service
        .cast_vote_and_get_proposal(&scope2, proposal2.proposal_id, VOTE_YES, proposal_owner2)
        .await
        .expect("first vote");

    let voter2 = PrivateKeySigner::random();
    service
        .cast_vote(&scope2, proposal2.proposal_id, VOTE_YES, voter2)
        .await
        .expect("second vote");

    sleep(Duration::from_millis(EXPIRATION_WAIT_TIME)).await;
    let result = service
        .get_consensus_result(&scope2, proposal2.proposal_id)
        .await;
    assert_eq!(
        result,
        Some(true),
        "RFC Section 4: n=2 requires unanimous YES"
    );

    // Test with n = 2, one YES one NO (should fail)
    let scope3 = ScopeID::from("scope3");
    let proposal_owner3 = PrivateKeySigner::random();
    let proposal3 = service
        .create_proposal(
            &scope3,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner3),
                EXPECTED_VOTERS_COUNT_2,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    let proposal3 = service
        .cast_vote_and_get_proposal(&scope3, proposal3.proposal_id, VOTE_YES, proposal_owner3)
        .await
        .expect("first vote");

    let voter3 = PrivateKeySigner::random();
    service
        .cast_vote(&scope3, proposal3.proposal_id, VOTE_NO, voter3)
        .await
        .expect("second vote");

    sleep(Duration::from_millis(EXPIRATION_WAIT_TIME)).await;
    let result = service
        .get_consensus_result(&scope3, proposal3.proposal_id)
        .await;
    assert_eq!(
        result,
        Some(false),
        "RFC Section 4: n=2 with non-unanimous should be NO"
    );
}

/// RFC Section 4: Test that n > 2 requires more than n/2 YES votes among 2n/3 distinct peers
#[tokio::test]
async fn test_n_gt_2_consensus_requirements() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("first vote");

    // Only 1 YES vote, not enough (need 2)
    let result = service
        .get_consensus_result(&scope, proposal.proposal_id)
        .await;
    assert_eq!(result, None, "Should not reach consensus with only 1 vote");

    let voter2 = PrivateKeySigner::random();
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, voter2)
        .await
        .expect("second vote");

    sleep(Duration::from_millis(EXPIRATION_WAIT_TIME)).await;
    let result = service
        .get_consensus_result(&scope, proposal.proposal_id)
        .await;
    assert_eq!(
        result,
        Some(true),
        "RFC Section 4: Should reach YES consensus with 2 YES votes out of 3"
    );
}

/// RFC Section 2.5.4: Test that expired proposals are rejected
#[tokio::test]
async fn test_expired_proposal_rejected() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                EXPIRATION_1_SECOND,
                true,
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    sleep(Duration::from_secs(EXPIRATION_WAIT_TIME_2_SECOND)).await;
    let voter = PrivateKeySigner::random();
    let err = service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, voter)
        .await
        .expect_err("Should reject vote on expired proposal");

    assert!(
        matches!(err, ConsensusError::VoteExpired),
        "RFC Section 2.5.4: Should reject votes on expired proposals"
    );
}

/// RFC Section 3.4: Test timestamp replay attack protection
#[tokio::test]
async fn test_timestamp_replay_attack_protection() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("first vote");

    // Create a vote with old timestamp (more than 1 hour ago)
    let old_timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(4000); // More than 1 hour

    let voter = PrivateKeySigner::random();
    let mut vote = create_vote_for_proposal(&proposal, VOTE_YES, voter.clone())
        .await
        .expect("create vote");

    // Manually set old timestamp
    vote.timestamp = old_timestamp;
    vote.vote_hash = hashgraph_like_consensus::utils::compute_vote_hash(&vote);
    vote.signature.clear();
    let vote_bytes = vote.encode_to_vec();
    vote.signature = voter
        .sign_message(&vote_bytes)
        .await
        .expect("sign vote")
        .as_bytes()
        .to_vec();

    let err = service
        .process_incoming_vote(&scope, vote)
        .await
        .expect_err("Should reject vote with old timestamp");

    assert!(
        matches!(err, ConsensusError::InvalidVoteTimestamp),
        "RFC Section 3.4: Should reject votes with timestamps that are too old (replay attack)"
    );
}

/// RFC Section 4: Test equality of votes handling
#[tokio::test]
async fn test_equality_of_votes_handling() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    // Test with liveness_criteria_yes = true (silent peers count as YES)
    let proposal = service
        .create_proposal(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                EXPECTED_VOTERS_COUNT_4,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("first vote");

    let voter2 = PrivateKeySigner::random();
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, voter2)
        .await
        .expect("second vote");

    let voter3 = PrivateKeySigner::random();
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_NO, voter3)
        .await
        .expect("third vote");

    let voter4 = PrivateKeySigner::random();
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_NO, voter4)
        .await
        .expect("fourth vote");

    sleep(Duration::from_millis(EXPIRATION_WAIT_TIME)).await;
    // We have 4 votes: 2 YES, 2 NO (equality)
    // With liveness_criteria_yes = true, should resolve to YES
    let result = service
        .get_consensus_result(&scope, proposal.proposal_id)
        .await;
    assert_eq!(
        result,
        Some(true),
        "RFC Section 4: Equality with liveness_criteria_yes=true should be YES"
    );

    // Test with liveness_criteria_yes = false
    let scope2 = ScopeID::from("scope2");
    let proposal_owner2 = PrivateKeySigner::random();
    let proposal2 = service
        .create_proposal(
            &scope2,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner2),
                EXPECTED_VOTERS_COUNT_4,
                EXPIRATION,
                false, // liveness_criteria_yes = false
            )
            .expect("valid proposal request"),
        )
        .await
        .expect("proposal should be created");

    let proposal2 = service
        .cast_vote_and_get_proposal(&scope2, proposal2.proposal_id, VOTE_YES, proposal_owner2)
        .await
        .expect("first vote");

    let voter2_2 = PrivateKeySigner::random();
    service
        .cast_vote(&scope2, proposal2.proposal_id, VOTE_YES, voter2_2)
        .await
        .expect("second vote");

    let voter3_2 = PrivateKeySigner::random();
    service
        .cast_vote(&scope2, proposal2.proposal_id, VOTE_NO, voter3_2)
        .await
        .expect("third vote");

    let voter4_2 = PrivateKeySigner::random();
    service
        .cast_vote(&scope2, proposal2.proposal_id, VOTE_NO, voter4_2)
        .await
        .expect("fourth vote");

    sleep(Duration::from_millis(EXPIRATION_WAIT_TIME)).await;
    // Equality with liveness_criteria_yes = false should resolve to NO
    let result = service
        .get_consensus_result(&scope2, proposal2.proposal_id)
        .await;
    assert_eq!(
        result,
        Some(false),
        "RFC Section 4: Equality with liveness_criteria_yes=false should be NO"
    );
}
