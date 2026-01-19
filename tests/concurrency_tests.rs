use alloy::signers::local::PrivateKeySigner;
use futures::future::join_all;
use std::{sync::Arc, time::Duration};
use tokio::{spawn, sync::Barrier, time::sleep};

use hashgraph_like_consensus::{
    error::ConsensusError, scope::ScopeID, service::DefaultConsensusService,
    session::ConsensusConfig, types::CreateProposalRequest,
};

const SCOPE: &str = "concurrency_scope";
const PROPOSAL_NAME: &str = "Concurrency Test";
const PROPOSAL_PAYLOAD: Vec<u8> = vec![];

const EXPIRATION: u64 = 120;
const EXPIRATION_WAIT_TIME: u64 = 100;

const EXPECTED_VOTERS_COUNT_10: u32 = 10;
const EXPECTED_VOTERS_COUNT_3: u32 = 3;
const EXPECTED_VOTERS_COUNT_5: u32 = 5;

const EXPECTED_PROPOSALS_COUNT_5: u32 = 5;

fn owner_bytes(signer: &PrivateKeySigner) -> Vec<u8> {
    signer.address().as_slice().to_vec()
}

// Verifies 10 parallel votes succeed under gossipsub and reach consensus;
#[tokio::test]
async fn test_concurrent_vote_casting() {
    let service = Arc::new(DefaultConsensusService::default());
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    // Use gossipsub mode (default) to allow all 10 votes in round 2
    // P2P mode would limit to ceil(2*10/3) = 7 votes, but we need 10 votes for this test
    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                owner_bytes(&proposal_owner),
                EXPECTED_VOTERS_COUNT_10,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    let proposal_id = proposal.proposal_id;
    let barrier = Arc::new(Barrier::new(EXPECTED_VOTERS_COUNT_10 as usize));

    let mut handles = Vec::new();
    for i in 0..EXPECTED_VOTERS_COUNT_10 {
        let service_clone = Arc::clone(&service);
        let scope_clone = scope.clone();
        let barrier_clone = Arc::clone(&barrier);
        let handle = spawn(async move {
            barrier_clone.wait().await;
            let voter = PrivateKeySigner::random();
            service_clone
                .cast_vote(&scope_clone, proposal_id, i % 2 == 0, voter)
                .await
        });
        handles.push(handle);
    }

    let results: Vec<_> = join_all(handles).await;
    let success_count = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(success_count, 10, "All 10 unique votes should succeed");

    sleep(Duration::from_millis(EXPIRATION_WAIT_TIME)).await;
    let result = service.get_consensus_result(&scope, proposal_id).await;
    assert!(
        result.is_ok(),
        "Consensus should be reached with concurrent votes"
    );
}

// Test concurrent proposal creation and vote processing
#[tokio::test]
async fn test_concurrent_proposal_operations() {
    let service = Arc::new(DefaultConsensusService::default());
    let scope = ScopeID::from(SCOPE);

    let mut handles = Vec::new();
    for i in 0..EXPECTED_PROPOSALS_COUNT_5 {
        let service_clone = Arc::clone(&service);
        let scope_clone = scope.clone();
        let handle = spawn(async move {
            let proposal_owner = PrivateKeySigner::random();
            service_clone
                .create_proposal_with_config(
                    &scope_clone,
                    CreateProposalRequest::new(
                        format!("Proposal {i}"),
                        PROPOSAL_PAYLOAD,
                        owner_bytes(&proposal_owner),
                        EXPECTED_VOTERS_COUNT_3,
                        EXPIRATION,
                        true,
                    )
                    .expect("valid proposal request"),
                    Some(ConsensusConfig::gossipsub()),
                )
                .await
        });
        handles.push(handle);
    }

    let results: Vec<_> = join_all(handles).await;
    let success_count = results
        .iter()
        .filter(|r| r.is_ok() && r.as_ref().unwrap().is_ok())
        .count();
    assert_eq!(
        success_count, EXPECTED_PROPOSALS_COUNT_5 as usize,
        "All proposals should be created successfully"
    );
}

// Test that duplicate votes are properly rejected
#[tokio::test]
async fn test_concurrent_duplicate_vote_rejection() {
    let service = Arc::new(DefaultConsensusService::default());
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();
    let voter = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
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

    let proposal_id = proposal.proposal_id;

    // Try to cast the same vote concurrently multiple times
    let mut handles = Vec::new();
    for _ in 0..EXPECTED_VOTERS_COUNT_5 {
        let service_clone = Arc::clone(&service);
        let scope_clone = scope.clone();
        let voter_clone = voter.clone();
        let handle = spawn(async move {
            service_clone
                .cast_vote(&scope_clone, proposal_id, true, voter_clone)
                .await
        });
        handles.push(handle);
    }

    let results: Vec<_> = join_all(handles).await;
    let vote_results: Vec<_> = results
        .into_iter()
        .map(|r| r.expect("Task should complete"))
        .collect();

    let voter_address = owner_bytes(&voter);
    let successful_votes: Vec<_> = vote_results
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .collect();

    for vote in &successful_votes {
        assert_eq!(
            vote.vote_owner, voter_address,
            "All votes should be from the same voter"
        );
    }

    let success_count = vote_results.iter().filter(|r| r.is_ok()).count();
    let duplicate_errors = vote_results
        .iter()
        .filter(|r| {
            if let Err(e) = r {
                matches!(e, ConsensusError::UserAlreadyVoted)
                    || matches!(e, ConsensusError::DuplicateVote)
            } else {
                false
            }
        })
        .count();

    assert_eq!(
        success_count, 1,
        "Only one vote from the same voter should succeed with atomic updates"
    );
    assert_eq!(
        duplicate_errors, 4,
        "Four votes should be rejected as duplicates"
    );
}
