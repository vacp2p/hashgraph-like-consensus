mod common;
use common::{now_ts, wrap};

use alloy::signers::local::PrivateKeySigner;
use std::{
    sync::{Arc, Barrier},
    thread,
    time::Duration,
};

use hashgraph_like_consensus::{
    error::ConsensusError,
    events::BroadcastEventBus,
    scope::ScopeID,
    service::{ConsensusService, DefaultConsensusService},
    session::ConsensusConfig,
    signing::{ConsensusSignatureScheme, EthereumConsensusSigner},
    storage::{ConsensusStorage, InMemoryConsensusStorage},
    types::CreateProposalRequest,
};

fn peer_service(
    storage: &InMemoryConsensusStorage<ScopeID>,
    bus: &BroadcastEventBus<ScopeID>,
    signer: EthereumConsensusSigner,
) -> DefaultConsensusService {
    ConsensusService::new_with_components(storage.clone(), bus.clone(), signer, 10)
}

const SCOPE: &str = "concurrency_scope";
const PROPOSAL_NAME: &str = "Concurrency Test";
const PROPOSAL_PAYLOAD: Vec<u8> = vec![];

const EXPIRATION: u64 = 120;
const EXPIRATION_WAIT_TIME: u64 = 100;

const EXPECTED_VOTERS_COUNT_10: u32 = 10;
const EXPECTED_VOTERS_COUNT_3: u32 = 3;
const EXPECTED_VOTERS_COUNT_5: u32 = 5;

const EXPECTED_PROPOSALS_COUNT_5: u32 = 5;

// Verifies 10 parallel votes succeed under gossipsub and reach consensus;
#[test]
fn test_concurrent_vote_casting() {
    let storage = InMemoryConsensusStorage::<ScopeID>::new();
    let bus = BroadcastEventBus::<ScopeID>::default();
    let scope = ScopeID::from(SCOPE);

    let owner = peer_service(&storage, &bus, wrap(PrivateKeySigner::random()));

    // Use gossipsub mode (default) to allow all 10 votes in round 2
    // P2P mode would limit to ceil(2*10/3) = 7 votes, but we need 10 votes for this test
    let proposal = owner
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                owner.signer().identity().to_vec(),
                EXPECTED_VOTERS_COUNT_10,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
            now_ts(),
        )
        .expect("proposal should be created");

    let proposal_id = proposal.proposal_id;
    let barrier = Arc::new(Barrier::new(EXPECTED_VOTERS_COUNT_10 as usize));

    // Each concurrent peer has its own service sharing storage + bus.
    let mut handles = Vec::new();
    for i in 0..EXPECTED_VOTERS_COUNT_10 {
        let storage = storage.clone();
        let bus = bus.clone();
        let scope_clone = scope.clone();
        let barrier_clone = Arc::clone(&barrier);
        let handle = thread::spawn(move || {
            barrier_clone.wait();
            let peer = peer_service(&storage, &bus, wrap(PrivateKeySigner::random()));
            peer.cast_vote(&scope_clone, proposal_id, i % 2 == 0, now_ts())
        });
        handles.push(handle);
    }

    let results: Vec<_> = handles.into_iter().map(|h| h.join()).collect();
    let success_count = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(success_count, 10, "All 10 unique votes should succeed");

    thread::sleep(Duration::from_millis(EXPIRATION_WAIT_TIME));
    let result = owner.storage().get_consensus_result(&scope, proposal_id);
    assert!(
        result.is_ok(),
        "Consensus should be reached with concurrent votes"
    );
}

// Test concurrent proposal creation and vote processing
#[test]
fn test_concurrent_proposal_operations() {
    let storage = InMemoryConsensusStorage::<ScopeID>::new();
    let bus = BroadcastEventBus::<ScopeID>::default();
    let scope = ScopeID::from(SCOPE);

    let mut handles = Vec::new();
    for i in 0..EXPECTED_PROPOSALS_COUNT_5 {
        let storage = storage.clone();
        let bus = bus.clone();
        let scope_clone = scope.clone();
        let handle = thread::spawn(move || {
            let proposal_owner = peer_service(&storage, &bus, wrap(PrivateKeySigner::random()));
            proposal_owner.create_proposal_with_config(
                &scope_clone,
                CreateProposalRequest::new(
                    format!("Proposal {i}"),
                    PROPOSAL_PAYLOAD,
                    proposal_owner.signer().identity().to_vec(),
                    EXPECTED_VOTERS_COUNT_3,
                    EXPIRATION,
                    true,
                )
                .expect("valid proposal request"),
                Some(ConsensusConfig::gossipsub()),
                now_ts(),
            )
        });
        handles.push(handle);
    }

    let results: Vec<_> = handles.into_iter().map(|h| h.join()).collect();
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
#[test]
fn test_concurrent_duplicate_vote_rejection() {
    let storage = InMemoryConsensusStorage::<ScopeID>::new();
    let bus = BroadcastEventBus::<ScopeID>::default();
    let scope = ScopeID::from(SCOPE);

    let owner_signer = wrap(PrivateKeySigner::random());
    let voter_signer = wrap(PrivateKeySigner::random());

    let owner = peer_service(&storage, &bus, owner_signer.clone());

    let proposal = owner
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                owner_signer.identity().to_vec(),
                EXPECTED_VOTERS_COUNT_3,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
            now_ts(),
        )
        .expect("proposal should be created");

    let proposal_id = proposal.proposal_id;

    // Many parallel handles all try to cast as the SAME voter. The voter has
    // one service; the concurrent cast_vote calls go through interior-mutable
    // storage atomically, so only one should succeed.
    let voter = Arc::new(peer_service(&storage, &bus, voter_signer.clone()));
    let mut handles = Vec::new();
    for _ in 0..EXPECTED_VOTERS_COUNT_5 {
        let voter = Arc::clone(&voter);
        let scope_clone = scope.clone();
        let handle =
            thread::spawn(move || voter.cast_vote(&scope_clone, proposal_id, true, now_ts()));
        handles.push(handle);
    }

    let results: Vec<_> = handles.into_iter().map(|h| h.join()).collect();
    let vote_results: Vec<_> = results
        .into_iter()
        .map(|r| r.expect("Task should complete"))
        .collect();

    let voter_address = voter_signer.identity().to_vec();
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
