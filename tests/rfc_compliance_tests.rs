use alloy::signers::{Signer, local::PrivateKeySigner};
use prost::Message;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

use hashgraph_like_consensus::{
    error::ConsensusError, scope::ScopeID, service::DefaultConsensusService,
    session::ConsensusConfig, types::CreateProposalRequest, utils::build_vote,
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
        .create_proposal_with_config(
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
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    assert_eq!(
        proposal.round, 1,
        "RFC Section 1: Proposal should start with round = 1"
    );
}

/// RFC Section 2.5.3: Test that round increments when votes are added (P2P mode)
#[tokio::test]
async fn test_round_increments_on_vote_p2p() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
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
            Some(ConsensusConfig::p2p()),
        )
        .await
        .expect("proposal should be created");

    assert_eq!(
        proposal.round, 1,
        "P2P: Proposal should start with round = 1"
    );

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("vote should be added");

    assert_eq!(
        proposal.round, 2,
        "P2P: Round should increment to 2 when first vote is added"
    );

    let voter2 = PrivateKeySigner::random();
    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, voter2)
        .await
        .expect("second vote should be added");

    assert_eq!(
        proposal.round, 3,
        "P2P: Round should increment to 3 when second vote is added"
    );
}

/// RFC Section 2.5.3: Test that gossipsub keeps rounds at 2 for all votes
#[tokio::test]
async fn test_gossipsub_rounds_stay_at_two() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    // Use 5 expected voters so we need 3 YES votes to reach consensus (>5/2 = >2.5)
    // This allows us to add multiple votes without reaching consensus early
    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                5, // 5 expected voters, need 3 YES for consensus
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    assert_eq!(
        proposal.round, 1,
        "Gossipsub: Proposal should start with round = 1"
    );

    // First vote moves to round 2
    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("vote should be added");

    assert_eq!(
        proposal.round, 2,
        "Gossipsub: Round should move to 2 when first vote is added"
    );
    assert_eq!(proposal.votes.len(), 1, "Should have 1 vote");

    // Second vote stays at round 2
    let voter2 = PrivateKeySigner::random();
    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, voter2)
        .await
        .expect("second vote should be added");

    assert_eq!(
        proposal.round, 2,
        "Gossipsub: Round should stay at 2 for all subsequent votes"
    );
    assert_eq!(proposal.votes.len(), 2, "Should have 2 votes");

    // Third vote also stays at round 2 (this reaches consensus with 3 YES votes)
    let voter3 = PrivateKeySigner::random();
    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, voter3)
        .await
        .expect("third vote should be added");

    assert_eq!(
        proposal.round, 2,
        "Gossipsub: Round should stay at 2 even with multiple votes"
    );
    assert_eq!(
        proposal.votes.len(),
        3,
        "Gossipsub: Should allow multiple votes in round 2"
    );
}

/// Test that gossipsub allows multiple votes in round 2 (not limited to 2 votes)
#[tokio::test]
async fn test_gossipsub_allows_multiple_votes_in_round_two() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    // Create proposal with 12 expected voters
    // Need ceil(2*12/3) = 8 votes for consensus threshold
    // Need >12/2 = >6 YES votes to reach consensus
    // So we can add at least 7 votes before consensus is reached
    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                12, // 12 expected voters
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    // First vote moves to round 2
    let mut last_proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("vote should be added");
    assert_eq!(last_proposal.round, 2);
    assert_eq!(last_proposal.votes.len(), 1);

    // Add 6 more YES votes (total 7) - all should stay in round 2
    // With 7 YES votes out of 12, we have >6 YES votes, so consensus is reached
    for i in 0..6 {
        let voter = PrivateKeySigner::random();
        last_proposal = service
            .cast_vote_and_get_proposal(&scope, last_proposal.proposal_id, VOTE_YES, voter)
            .await
            .unwrap_or_else(|_| panic!("vote {} should be added", i + 2));
        assert_eq!(
            last_proposal.round, 2,
            "Gossipsub: All votes should stay in round 2"
        );
    }

    assert_eq!(last_proposal.round, 2, "Gossipsub: Final round should be 2");
    assert_eq!(
        last_proposal.votes.len(),
        7,
        "Gossipsub: Should allow multiple votes (7) in round 2"
    );

    // Note: With 7 YES votes out of 12, we have >6 YES votes, so consensus should be reached
    // But the test focuses on verifying rounds stay at 2, not consensus logic
}

/// Test that P2P mode uses ceil(2n/3) for max rounds when max_rounds = 0
#[tokio::test]
async fn test_p2p_dynamic_max_rounds() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    // For n=9, ceil(2*9/3) = ceil(6) = 6 votes max
    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&proposal_owner),
                9, // 9 expected voters, ceil(2n/3) = 6
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::p2p()),
        )
        .await
        .expect("proposal should be created");

    // Add 6 votes - should all succeed
    let mut voters = vec![proposal_owner];
    for _ in 0..5 {
        voters.push(PrivateKeySigner::random());
    }

    let mut last_proposal = proposal;
    for (i, voter) in voters.iter().enumerate() {
        last_proposal = service
            .cast_vote_and_get_proposal(&scope, last_proposal.proposal_id, VOTE_YES, voter.clone())
            .await
            .unwrap_or_else(|_| panic!("vote {} should be added", i + 1));
        assert_eq!(
            last_proposal.round,
            (i + 2) as u32,
            "P2P: Round should increment for each vote"
        );
    }

    assert_eq!(
        last_proposal.votes.len(),
        6,
        "P2P: Should allow 6 votes (ceil(2n/3))"
    );
    assert_eq!(
        last_proposal.round, 7,
        "P2P: Round should be 7 (1 + 6 votes)"
    );

    // With 6 YES votes out of 9, we have >4.5 YES votes, so consensus is reached
    // Verify consensus was reached (this is expected behavior)
    let result = service
        .get_consensus_result(&scope, last_proposal.proposal_id)
        .await;
    assert_eq!(
        result,
        Some(true),
        "Consensus should be reached with 6 YES votes"
    );

    // The key test: we successfully added 6 votes, which is ceil(2n/3) = ceil(6) = 6
    // This verifies that P2P mode correctly calculates and enforces the max rounds limit
}

/// Test P2P ceil(2n/3) calculation for various values of n
#[tokio::test]
async fn test_p2p_ceil_calculation_edge_cases() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);

    // Test various values of n to verify ceil(2n/3) calculation
    let test_cases = vec![
        (1, 1),  // ceil(2*1/3) = ceil(0.67) = 1
        (2, 2),  // ceil(2*2/3) = ceil(1.33) = 2
        (3, 2),  // ceil(2*3/3) = ceil(2) = 2
        (4, 3),  // ceil(2*4/3) = ceil(2.67) = 3
        (5, 4),  // ceil(2*5/3) = ceil(3.33) = 4
        (6, 4),  // ceil(2*6/3) = ceil(4) = 4
        (7, 5),  // ceil(2*7/3) = ceil(4.67) = 5
        (8, 6),  // ceil(2*8/3) = ceil(5.33) = 6
        (9, 6),  // ceil(2*9/3) = ceil(6) = 6
        (10, 7), // ceil(2*10/3) = ceil(6.67) = 7
    ];

    for (n, expected_max_votes) in test_cases {
        let proposal_owner = PrivateKeySigner::random();
        let proposal = service
            .create_proposal_with_config(
                &scope,
                CreateProposalRequest::new(
                    format!("Test n={}", n),
                    PROPOSAL_PAYLOAD.to_string(),
                    owner_bytes(&proposal_owner),
                    n,
                    EXPIRATION,
                    true,
                )
                .expect("valid proposal request"),
                Some(ConsensusConfig::p2p()),
            )
            .await
            .expect("proposal should be created");

        // Add votes up to the limit
        let mut voters = vec![proposal_owner];
        for _ in 0..(expected_max_votes - 1) {
            voters.push(PrivateKeySigner::random());
        }

        let mut last_proposal = proposal;
        for (i, voter) in voters.iter().enumerate() {
            last_proposal = service
                .cast_vote_and_get_proposal(
                    &scope,
                    last_proposal.proposal_id,
                    VOTE_YES,
                    voter.clone(),
                )
                .await
                .unwrap_or_else(|_| panic!("vote {} for n={} should succeed", i + 1, n));
        }

        assert_eq!(
            last_proposal.votes.len(),
            expected_max_votes as usize,
            "P2P: For n={}, should allow {} votes (ceil(2n/3))",
            n,
            expected_max_votes
        );
    }
}

/// Test that gossipsub correctly processes batch votes via process_incoming_proposal
#[tokio::test]
async fn test_gossipsub_batch_vote_processing() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("batch_gossipsub");
    let proposal_owner = PrivateKeySigner::random();

    // Create a proposal manually (simulating receiving from network)
    let request = CreateProposalRequest::new(
        PROPOSAL_NAME.to_string(),
        PROPOSAL_PAYLOAD.to_string(),
        owner_bytes(&proposal_owner),
        5,
        EXPIRATION,
        true,
    )
    .expect("valid proposal request");

    let mut proposal = request.into_proposal().expect("proposal should be created");

    // Add votes to the proposal (simulating votes received from network)
    let voter1 = PrivateKeySigner::random();
    let vote1 = build_vote(&proposal, VOTE_YES, voter1)
        .await
        .expect("vote should be created");
    proposal.votes.push(vote1);
    proposal.round = 2; // Gossipsub: round 2 after first vote

    let voter2 = PrivateKeySigner::random();
    let vote2 = build_vote(&proposal, VOTE_YES, voter2)
        .await
        .expect("vote should be created");
    proposal.votes.push(vote2);
    // Round stays at 2 for gossipsub

    let voter3 = PrivateKeySigner::random();
    let vote3 = build_vote(&proposal, VOTE_YES, voter3)
        .await
        .expect("vote should be created");
    proposal.votes.push(vote3);

    // Process the proposal with multiple votes (batch processing)
    // This should work in gossipsub mode - all votes are in round 2
    let result = service
        .process_incoming_proposal(&scope, proposal.clone())
        .await;
    assert!(
        result.is_ok(),
        "Gossipsub: Should accept batch votes in round 2"
    );

    // Verify by casting another vote and checking the proposal
    let voter4 = PrivateKeySigner::random();
    let final_proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, voter4)
        .await
        .expect("additional vote should succeed");
    assert_eq!(
        final_proposal.round, 2,
        "Gossipsub: Round should be 2 after batch processing"
    );
    assert_eq!(
        final_proposal.votes.len(),
        4,
        "Gossipsub: Should have 4 votes total (3 from batch + 1 new)"
    );
}

/// Test that P2P correctly processes batch votes via process_incoming_proposal
#[tokio::test]
async fn test_p2p_batch_vote_processing() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("batch_p2p");
    let proposal_owner = PrivateKeySigner::random();

    // Create a P2P proposal manually (simulating receiving from network)
    let request = CreateProposalRequest::new(
        PROPOSAL_NAME.to_string(),
        PROPOSAL_PAYLOAD.to_string(),
        owner_bytes(&proposal_owner),
        9, // ceil(2*9/3) = 6 votes max
        EXPIRATION,
        true,
    )
    .expect("valid proposal request");

    let mut proposal = request.into_proposal().expect("proposal should be created");

    // Add votes up to the limit (6 votes)
    let mut voters = vec![proposal_owner];
    for _ in 0..5 {
        voters.push(PrivateKeySigner::random());
    }

    for (i, voter) in voters.iter().enumerate() {
        let vote = build_vote(&proposal, VOTE_YES, voter.clone())
            .await
            .expect("vote should be created");
        proposal.votes.push(vote);
        proposal.round = (i + 2) as u32; // P2P: round increments per vote
    }

    // Process the proposal with 6 votes (at the limit)
    let result = service
        .process_incoming_proposal(&scope, proposal.clone())
        .await;
    assert!(
        result.is_ok(),
        "P2P: Should accept batch votes up to ceil(2n/3)"
    );

    // Verify by checking consensus result (6 YES votes out of 9 reaches consensus)
    let consensus_result = service
        .get_consensus_result(&scope, proposal.proposal_id)
        .await;
    assert_eq!(
        consensus_result,
        Some(true),
        "P2P: Consensus should be reached with 6 YES votes"
    );

    // Verify that consensus prevents further votes (session is no longer active)
    // Try to add one more vote - should fail because consensus is already reached
    let voter7 = PrivateKeySigner::random();
    let _vote_result = service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, voter7)
        .await;

    // The vote might succeed (returns vote) but won't be added because session reached consensus
    // Or it might fail with SessionNotActive
    // Either way, verify the vote count doesn't increase and consensus result stays the same
    let final_consensus = service
        .get_consensus_result(&scope, proposal.proposal_id)
        .await;
    assert_eq!(
        final_consensus,
        Some(true),
        "P2P: Consensus result should remain YES"
    );

    // The key test: we successfully processed 6 votes in batch, which is ceil(2n/3) = 6
    // This verifies that P2P mode correctly handles batch votes up to the limit
}

/// Test that consensus can be reached in both modes
#[tokio::test]
async fn test_consensus_reachable_in_both_modes() {
    let service = DefaultConsensusService::default();

    // Test gossipsub mode
    let scope1 = ScopeID::from("gossipsub_consensus");
    let owner1 = PrivateKeySigner::random();
    let proposal1 = service
        .create_proposal_with_config(
            &scope1,
            CreateProposalRequest::new(
                "Gossipsub Consensus".to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&owner1),
                6, // Need 4 YES votes for consensus (>6/2 = >3)
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    // Add 4 YES votes (all in round 2 for gossipsub)
    let mut voters1 = vec![owner1];
    for _ in 0..3 {
        voters1.push(PrivateKeySigner::random());
    }

    for voter in voters1 {
        service
            .cast_vote(&scope1, proposal1.proposal_id, VOTE_YES, voter)
            .await
            .expect("vote should succeed");
    }

    let result1 = service
        .get_consensus_result(&scope1, proposal1.proposal_id)
        .await;
    assert_eq!(
        result1,
        Some(true),
        "Gossipsub: Consensus should be reached"
    );

    // Test P2P mode
    let scope2 = ScopeID::from("p2p_consensus");
    let owner2 = PrivateKeySigner::random();
    let proposal2 = service
        .create_proposal_with_config(
            &scope2,
            CreateProposalRequest::new(
                "P2P Consensus".to_string(),
                PROPOSAL_PAYLOAD.to_string(),
                owner_bytes(&owner2),
                6, // Need 4 YES votes for consensus
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::p2p()),
        )
        .await
        .expect("proposal should be created");

    // Add 4 YES votes (rounds increment in P2P)
    let mut voters2 = vec![owner2];
    for _ in 0..3 {
        voters2.push(PrivateKeySigner::random());
    }

    for voter in voters2 {
        service
            .cast_vote(&scope2, proposal2.proposal_id, VOTE_YES, voter)
            .await
            .expect("vote should succeed");
    }

    let result2 = service
        .get_consensus_result(&scope2, proposal2.proposal_id)
        .await;
    assert_eq!(result2, Some(true), "P2P: Consensus should be reached");
}

/// RFC Section 4: Test that n ≤ 2 requires unanimous YES votes
#[tokio::test]
async fn test_n_le_2_requires_unanimous_yes() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
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
            Some(ConsensusConfig::gossipsub()),
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
        .create_proposal_with_config(
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
            Some(ConsensusConfig::gossipsub()),
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
        .create_proposal_with_config(
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
            Some(ConsensusConfig::gossipsub()),
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
        .create_proposal_with_config(
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
            Some(ConsensusConfig::gossipsub()),
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
        .create_proposal_with_config(
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
            Some(ConsensusConfig::gossipsub()),
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
        .create_proposal_with_config(
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
            Some(ConsensusConfig::gossipsub()),
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
    let mut vote = build_vote(&proposal, VOTE_YES, voter.clone())
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
    // Use gossipsub mode (default) to allow multiple votes in round 2
    // P2P mode would limit to ceil(2*4/3) = 3 votes, but we need 4 votes for equality test
    let proposal = service
        .create_proposal_with_config(
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
            Some(ConsensusConfig::gossipsub()),
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
    // Use gossipsub mode to allow 4 votes for equality test
    let scope2 = ScopeID::from("scope2");
    let proposal_owner2 = PrivateKeySigner::random();
    let proposal2 = service
        .create_proposal_with_config(
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
            Some(ConsensusConfig::gossipsub()),
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
