use alloy::signers::Signer;
use alloy::signers::local::PrivateKeySigner;
use prost::Message;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::timeout;

use hashgraph_like_consensus::{
    api::ConsensusServiceAPI,
    error::ConsensusError,
    scope::ScopeID,
    service::DefaultConsensusService,
    session::ConsensusConfig,
    types::{ConsensusEvent, CreateProposalRequest},
    utils::{build_vote, compute_vote_hash},
};

const SCOPE1_NAME: &str = "scope1";
const SCOPE2_NAME: &str = "scope2";
const PROPOSAL_NAME: &str = "Test Proposal";
const PROPOSAL_PAYLOAD: Vec<u8> = vec![];
const PROPOSAL_EXPIRATION_TIME: u64 = 60;

const EXPECTED_VOTERS_COUNT_4: u32 = 4;
const EXPECTED_VOTERS_COUNT_3: u32 = 3;
const EXPECTED_VOTERS_COUNT_2: u32 = 2;
const EXPECTED_VOTERS_COUNT_1: u32 = 1;

const VOTE_YES: bool = true;

fn proposal_owner_from_signer(signer: &PrivateKeySigner) -> Vec<u8> {
    signer.address().as_slice().to_vec()
}

async fn setup_proposal(
    service: &DefaultConsensusService,
    scope: &ScopeID,
    proposal_owner: &PrivateKeySigner,
    expected_voters_count: u32,
    liveness_criteria_yes: bool,
    consensus_config: ConsensusConfig,
) -> hashgraph_like_consensus::protos::consensus::v1::Proposal {
    service
        .create_proposal_with_config(
            scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(proposal_owner),
                expected_voters_count,
                PROPOSAL_EXPIRATION_TIME,
                liveness_criteria_yes,
            )
            .expect("valid proposal request"),
            Some(consensus_config),
        )
        .await
        .expect("proposal should be created")
}

async fn cast_vote_or_panic(
    service: &DefaultConsensusService,
    scope: &ScopeID,
    proposal_id: u32,
    choice: bool,
    signer: PrivateKeySigner,
    msg: &str,
) {
    service
        .cast_vote(scope, proposal_id, choice, signer)
        .await
        .expect(msg);
}

#[tokio::test]
async fn test_basic_consensus_flow() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                PROPOSAL_EXPIRATION_TIME,
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
        .expect("proposal_owner vote should succeed");

    assert_eq!(
        service
            .get_active_proposals(&scope)
            .await
            .unwrap()
            .unwrap()
            .len(),
        1
    );
    let stats = service.get_scope_stats(&scope).await;
    assert_eq!(stats.total_sessions, 1);
    assert!(
        !service
            .has_sufficient_votes_for_proposal(&scope, proposal.proposal_id)
            .await
            .expect("check should work")
    );

    let voter_two = PrivateKeySigner::random();
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, voter_two)
        .await
        .expect("second vote should succeed");

    let voter_three = PrivateKeySigner::random();
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, voter_three)
        .await
        .expect("third vote should succeed");

    assert!(
        service
            .has_sufficient_votes_for_proposal(&scope, proposal.proposal_id)
            .await
            .expect("check should work")
    );
}

#[tokio::test]
async fn test_multi_scope_isolation() {
    let service = DefaultConsensusService::new_with_max_sessions(5);
    let scope1 = ScopeID::from(SCOPE1_NAME);
    let scope2 = ScopeID::from(SCOPE2_NAME);

    let signer1 = PrivateKeySigner::random();
    let signer2 = PrivateKeySigner::random();

    let proposal_1 = service
        .create_proposal_with_config(
            &scope1,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&signer1),
                EXPECTED_VOTERS_COUNT_2,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("scope1 proposal");

    service
        .cast_vote_and_get_proposal(&scope1, proposal_1.proposal_id, VOTE_YES, signer1)
        .await
        .expect("scope1 proposal_owner vote");

    let proposal_2 = service
        .create_proposal_with_config(
            &scope2,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&signer2),
                EXPECTED_VOTERS_COUNT_1,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("scope2 proposal");

    service
        .cast_vote_and_get_proposal(&scope2, proposal_2.proposal_id, VOTE_YES, signer2)
        .await
        .expect("scope2 proposal_owner vote");

    assert_eq!(
        service
            .get_active_proposals(&scope1)
            .await
            .unwrap()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(service.get_active_proposals(&scope2).await.unwrap(), None); // scope2 reached consensus

    let stats1 = service.get_scope_stats(&scope1).await;
    assert_eq!(stats1.total_sessions, 1);
    assert_eq!(stats1.active_sessions, 1);

    let stats2 = service.get_scope_stats(&scope2).await;
    assert_eq!(stats2.total_sessions, 1);
    assert_eq!(stats2.active_sessions, 0);
}

#[tokio::test]
async fn test_consensus_threshold_emits_event() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_4,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("proposal_owner vote");

    for _ in 0..3 {
        let signer = PrivateKeySigner::random();
        service
            .cast_vote(&scope, proposal.proposal_id, VOTE_YES, signer)
            .await
            .expect("additional vote");
    }

    let proposal_id = proposal.proposal_id;
    let result = timeout(Duration::from_secs(5), async {
        while let Ok((event_scope, event)) = events.recv().await {
            if event_scope == scope
                && let ConsensusEvent::ConsensusReached {
                    proposal_id: event_proposal_id,
                    result,
                    timestamp: _event_timestamp,
                } = event
                && proposal_id == event_proposal_id
            {
                return Some(result);
            }
        }
        None
    })
    .await
    .expect("event timeout")
    .expect("consensus event missing");

    assert!(result);
}

#[tokio::test]
async fn test_handle_consensus_timeout_already_reached() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create proposal and reach consensus
    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_2,
        true,
        ConsensusConfig::gossipsub(),
    )
    .await;

    // Cast votes to reach consensus
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    )
    .await;

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    )
    .await;

    // Wait a bit to ensure consensus is reached
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now call handle_consensus_timeout - should return the already reached consensus
    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect("should return consensus result");

    assert!(result, "should return true (YES consensus)");
}

#[tokio::test]
async fn test_handle_consensus_timeout_reaches_consensus() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create proposal with enough votes but not quite at threshold yet
    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_3,
        true,
        ConsensusConfig::gossipsub(),
    )
    .await;

    // Cast 2 YES votes (need 2 for threshold with 3 expected voters)
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    )
    .await;

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    )
    .await;

    // Call handle_consensus_timeout - should calculate consensus and reach it
    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect("should reach consensus");

    assert!(result, "should return true (YES consensus)");

    // Verify event was emitted
    let event_received = timeout(Duration::from_secs(1), async {
        while let Ok((event_scope, event)) = events.recv().await {
            if event_scope == scope
                && let ConsensusEvent::ConsensusReached {
                    proposal_id: event_proposal_id,
                    result: event_result,
                    timestamp: _event_timestamp,
                } = event
                && event_proposal_id == proposal.proposal_id
            {
                return Some(event_result);
            }
        }
        None
    })
    .await
    .expect("event timeout")
    .expect("consensus event should be emitted");

    assert!(event_received, "event should indicate YES consensus");
}

#[tokio::test]
async fn test_handle_consensus_timeout_reaches_no_consensus_with_multiple_votes() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from(SCOPE1_NAME);

    let yes_voter = PrivateKeySigner::random();
    let no_voter_1 = PrivateKeySigner::random();
    let no_voter_2 = PrivateKeySigner::random();

    let proposal = setup_proposal(
        &service,
        &scope,
        &yes_voter,
        EXPECTED_VOTERS_COUNT_4,
        false,
        ConsensusConfig::gossipsub(),
    )
    .await;

    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        true,
        yes_voter,
        "YES vote should succeed",
    )
    .await;

    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        false,
        no_voter_1,
        "first NO vote should succeed",
    )
    .await;

    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        false,
        no_voter_2,
        "second NO vote should succeed",
    )
    .await;

    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect("should reach consensus at timeout");

    assert!(!result, "should return false (NO consensus)");

    let event_result = timeout(Duration::from_secs(1), async {
        while let Ok((event_scope, event)) = events.recv().await {
            if event_scope == scope
                && let ConsensusEvent::ConsensusReached {
                    proposal_id: event_proposal_id,
                    result: event_result,
                    timestamp: _event_timestamp,
                } = event
                && event_proposal_id == proposal.proposal_id
            {
                return Some(event_result);
            }
        }
        None
    })
    .await
    .expect("event timeout")
    .expect("consensus event should be emitted");

    assert!(!event_result, "event should indicate NO consensus");
}

#[tokio::test]
async fn test_handle_consensus_timeout_resolves_with_liveness_yes() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create proposal: expected_voters=4, liveness=true
    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_4,
        true,
        ConsensusConfig::gossipsub(),
    )
    .await;

    // Cast 1 YES vote (3 silent peers counted as YES at timeout → 4 YES total)
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    )
    .await;

    // At timeout with liveness=true, silent peers count as YES → consensus reached
    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect("should reach consensus with silent peers as YES");

    assert!(result, "should return true (YES consensus)");

    // Verify ConsensusReached event was emitted
    let event_received = timeout(Duration::from_secs(1), async {
        while let Ok((event_scope, event)) = events.recv().await {
            if event_scope == scope
                && let ConsensusEvent::ConsensusReached {
                    proposal_id: event_proposal_id,
                    result: event_result,
                    timestamp: _event_timestamp,
                } = event
                && event_proposal_id == proposal.proposal_id
            {
                return Some(event_result);
            }
        }
        None
    })
    .await
    .expect("event timeout")
    .expect("ConsensusReached event should be emitted");

    assert!(event_received, "event should indicate YES consensus");

    // Verify get_consensus_result returns Ok(true)
    let consensus_result = service
        .get_consensus_result(&scope, proposal.proposal_id)
        .await
        .expect("consensus result should be available");
    assert!(consensus_result, "consensus result should be true");
}

#[tokio::test]
async fn test_handle_consensus_timeout_insufficient_votes() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create proposal with liveness=false: silent peers count as NO at timeout
    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_4,
        false,
        ConsensusConfig::gossipsub(),
    )
    .await;

    // Cast 2 YES votes (2 silent count as NO → 2 YES, 2 NO → tied → no consensus)
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    )
    .await;

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    )
    .await;

    // Call handle_consensus_timeout - should fail (tied votes, no majority)
    let err = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect_err("should fail with insufficient votes");

    assert!(
        matches!(err, ConsensusError::InsufficientVotesAtTimeout),
        "should return InsufficientVotesAtTimeout error"
    );

    // Verify ConsensusFailed event was emitted
    let event_received = timeout(Duration::from_secs(1), async {
        while let Ok((event_scope, event)) = events.recv().await {
            if event_scope == scope
                && let ConsensusEvent::ConsensusFailed {
                    proposal_id: event_proposal_id,
                    timestamp: _event_timestamp,
                } = event
                && event_proposal_id == proposal.proposal_id
            {
                return true;
            }
        }
        false
    })
    .await
    .expect("event timeout");

    assert!(event_received, "ConsensusFailed event should be emitted");

    // Verify session is marked as Failed
    let consensus_result = service
        .get_consensus_result(&scope, proposal.proposal_id)
        .await;
    assert!(matches!(
        consensus_result,
        Err(ConsensusError::ConsensusFailed)
    ));

    let active_proposals = service
        .get_active_proposals(&scope)
        .await
        .expect("should not return error while getting active proposals");
    assert!(
        active_proposals.is_none(),
        "proposal should not be in active proposals"
    );
}

#[tokio::test]
async fn test_handle_consensus_timeout_no_votes_liveness_true() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create proposal with liveness=true but don't cast any votes
    // At timeout, all 3 silent peers count as YES → YES consensus
    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_3,
        true,
        ConsensusConfig::gossipsub(),
    )
    .await;

    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect("should reach consensus with silent peers as YES");

    assert!(result, "should return true (all silent peers as YES)");

    // Verify ConsensusReached event was emitted
    let event_received = timeout(Duration::from_secs(1), async {
        while let Ok((event_scope, event)) = events.recv().await {
            if event_scope == scope
                && let ConsensusEvent::ConsensusReached {
                    proposal_id: event_proposal_id,
                    result: event_result,
                    timestamp: _event_timestamp,
                } = event
                && event_proposal_id == proposal.proposal_id
            {
                return Some(event_result);
            }
        }
        None
    })
    .await
    .expect("event timeout")
    .expect("ConsensusReached event should be emitted");

    assert!(event_received, "event should indicate YES consensus");
}

#[tokio::test]
async fn test_handle_consensus_timeout_no_votes_liveness_false() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create proposal with liveness=false but don't cast any votes
    // At timeout, all 3 silent peers count as NO → NO consensus
    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_3,
        false,
        ConsensusConfig::gossipsub(),
    )
    .await;

    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect("should reach NO consensus with silent peers as NO");

    assert!(!result, "should return false (all silent peers as NO)");

    // Verify ConsensusReached event was emitted
    let event_received = timeout(Duration::from_secs(1), async {
        while let Ok((event_scope, event)) = events.recv().await {
            if event_scope == scope
                && let ConsensusEvent::ConsensusReached {
                    proposal_id: event_proposal_id,
                    result: event_result,
                    timestamp: _event_timestamp,
                } = event
                && event_proposal_id == proposal.proposal_id
            {
                return Some(event_result);
            }
        }
        None
    })
    .await
    .expect("event timeout")
    .expect("ConsensusReached event should be emitted");

    assert!(!event_received, "event should indicate NO consensus");
}

#[tokio::test]
async fn test_handle_consensus_timeout_reaches_consensus_p2p() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from("scope_p2p_reaches_consensus");
    let proposal_owner = PrivateKeySigner::random();

    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_3,
        true,
        ConsensusConfig::p2p(),
    )
    .await;

    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    )
    .await;

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    )
    .await;

    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect("should reach consensus");

    assert!(result, "should return true (YES consensus)");

    let event_received = timeout(Duration::from_secs(1), async {
        while let Ok((event_scope, event)) = events.recv().await {
            if event_scope == scope
                && let ConsensusEvent::ConsensusReached {
                    proposal_id: event_proposal_id,
                    result: event_result,
                    timestamp: _event_timestamp,
                } = event
                && event_proposal_id == proposal.proposal_id
            {
                return Some(event_result);
            }
        }
        None
    })
    .await
    .expect("event timeout")
    .expect("consensus event should be emitted");

    assert!(event_received, "event should indicate YES consensus");
}

#[tokio::test]
async fn test_handle_consensus_timeout_insufficient_votes_p2p() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from("scope_p2p_insufficient_votes");
    let proposal_owner = PrivateKeySigner::random();

    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_4,
        false,
        ConsensusConfig::p2p(),
    )
    .await;

    // Cast 2 YES votes (2 silent count as NO → 2 YES, 2 NO → tied → no consensus)
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    )
    .await;

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    )
    .await;

    let err = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect_err("should fail with insufficient votes");

    assert!(
        matches!(err, ConsensusError::InsufficientVotesAtTimeout),
        "should return InsufficientVotesAtTimeout error"
    );

    let event_received = timeout(Duration::from_secs(1), async {
        while let Ok((event_scope, event)) = events.recv().await {
            if event_scope == scope
                && let ConsensusEvent::ConsensusFailed {
                    proposal_id: event_proposal_id,
                    timestamp: _event_timestamp,
                } = event
                && event_proposal_id == proposal.proposal_id
            {
                return true;
            }
        }
        false
    })
    .await
    .expect("event timeout");

    assert!(event_received, "ConsensusFailed event should be emitted");
}

#[tokio::test]
async fn test_cast_vote_rejects_same_voter_twice() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    service
        .cast_vote(
            &scope,
            proposal.proposal_id,
            VOTE_YES,
            proposal_owner.clone(),
        )
        .await
        .expect("first vote should succeed");

    let err = service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect_err("second vote from same voter should fail");

    assert!(
        matches!(err, ConsensusError::UserAlreadyVoted),
        "should return UserAlreadyVoted"
    );
}

#[tokio::test]
async fn test_process_incoming_proposal_rejects_duplicate_proposal() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_2,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    let err = service
        .process_incoming_proposal(&scope, proposal)
        .await
        .expect_err("duplicate proposal should be rejected");

    assert!(
        matches!(err, ConsensusError::ProposalAlreadyExist),
        "should return ProposalAlreadyExist"
    );
}

#[tokio::test]
async fn test_process_incoming_vote_rejects_unknown_session() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);

    let proposal_owner = PrivateKeySigner::random();
    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_2,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    let unknown_proposal_id = proposal.proposal_id.wrapping_add(1);
    let vote_owner = PrivateKeySigner::random();
    let vote = service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, vote_owner.clone())
        .await
        .expect("valid signed vote");

    let mut invalid_vote = vote;
    invalid_vote.proposal_id = unknown_proposal_id;
    invalid_vote.vote_hash = compute_vote_hash(&invalid_vote);
    invalid_vote.signature.clear();
    let vote_bytes = invalid_vote.encode_to_vec();
    invalid_vote.signature = vote_owner
        .sign_message(&vote_bytes)
        .await
        .expect("vote should be resignable")
        .as_bytes()
        .to_vec();

    let err = service
        .process_incoming_vote(&scope, invalid_vote)
        .await
        .expect_err("vote for unknown proposal should fail");

    assert!(
        matches!(err, ConsensusError::SessionNotFound),
        "should return SessionNotFound"
    );
}

#[tokio::test]
async fn test_handle_consensus_timeout_rejects_unknown_session() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);

    let err = service
        .handle_consensus_timeout(&scope, u32::MAX)
        .await
        .expect_err("timeout handling for unknown proposal should fail");

    assert!(
        matches!(err, ConsensusError::SessionNotFound),
        "should return SessionNotFound"
    );
}

#[tokio::test]
async fn test_process_incoming_proposal_rejects_expired_proposal() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    let request = CreateProposalRequest::new(
        PROPOSAL_NAME.to_string(),
        PROPOSAL_PAYLOAD,
        proposal_owner_from_signer(&proposal_owner),
        EXPECTED_VOTERS_COUNT_3,
        1,
        true,
    )
    .expect("valid proposal request");
    let proposal = request.into_proposal().expect("proposal should be created");

    tokio::time::sleep(Duration::from_secs(2)).await;

    let err = service
        .process_incoming_proposal(&scope, proposal)
        .await
        .expect_err("expired incoming proposal should fail");

    assert!(
        matches!(err, ConsensusError::ProposalExpired),
        "should return ProposalExpired"
    );
}

#[tokio::test]
async fn test_process_incoming_vote_rejects_invalid_vote_hash() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                PROPOSAL_EXPIRATION_TIME,
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
        .expect("first vote should succeed");

    let voter = PrivateKeySigner::random();
    let mut vote = build_vote(&proposal, VOTE_YES, voter)
        .await
        .expect("valid vote should be built");
    vote.vote_hash = vec![1; 32];

    let err = service
        .process_incoming_vote(&scope, vote)
        .await
        .expect_err("tampered vote hash should fail");

    assert!(
        matches!(err, ConsensusError::InvalidVoteHash),
        "should return InvalidVoteHash"
    );
}

#[tokio::test]
async fn test_process_incoming_vote_rejects_invalid_vote_signature() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    let proposal = service
        .cast_vote_and_get_proposal(
            &scope,
            proposal.proposal_id,
            VOTE_YES,
            proposal_owner.clone(),
        )
        .await
        .expect("first vote should succeed");

    let voter = PrivateKeySigner::random();
    let mut vote = build_vote(&proposal, VOTE_YES, voter)
        .await
        .expect("valid vote should be built");
    let wrong_signer = PrivateKeySigner::random();
    let vote_bytes = vote.encode_to_vec();
    let wrong_sig = wrong_signer
        .sign_message(&vote_bytes)
        .await
        .expect("wrong signer should sign");
    vote.signature = wrong_sig.as_bytes().to_vec();

    let err = service
        .process_incoming_vote(&scope, vote)
        .await
        .expect_err("tampered signature should fail");

    assert!(
        matches!(err, ConsensusError::InvalidVoteSignature),
        "should return InvalidVoteSignature"
    );
}

#[tokio::test]
async fn test_process_incoming_vote_rejects_duplicate_vote_owner() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    let proposal = service
        .cast_vote_and_get_proposal(
            &scope,
            proposal.proposal_id,
            VOTE_YES,
            proposal_owner.clone(),
        )
        .await
        .expect("first owner vote should succeed");

    let duplicate_vote = build_vote(&proposal, VOTE_YES, proposal_owner)
        .await
        .expect("duplicate vote should be signable");

    let err = service
        .process_incoming_vote(&scope, duplicate_vote)
        .await
        .expect_err("duplicate vote owner should fail");

    assert!(
        matches!(err, ConsensusError::DuplicateVote),
        "should return DuplicateVote"
    );
}

#[tokio::test]
async fn test_process_incoming_vote_rejects_expired_vote_timestamp() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                PROPOSAL_EXPIRATION_TIME,
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
        .expect("first vote should succeed");

    let voter = PrivateKeySigner::random();
    let mut vote = build_vote(&proposal, VOTE_YES, voter.clone())
        .await
        .expect("valid vote should be built");
    vote.timestamp = proposal.expiration_timestamp.saturating_add(1);
    vote.vote_hash = compute_vote_hash(&vote);
    vote.signature.clear();
    let vote_bytes = vote.encode_to_vec();
    vote.signature = voter
        .sign_message(&vote_bytes)
        .await
        .expect("vote should be resignable")
        .as_bytes()
        .to_vec();

    let err = service
        .process_incoming_vote(&scope, vote)
        .await
        .expect_err("expired vote timestamp should fail");

    assert!(
        matches!(err, ConsensusError::VoteExpired),
        "should return VoteExpired"
    );
}

#[tokio::test]
async fn test_handle_consensus_timeout_is_idempotent_for_failed_session() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // liveness=false, 4 expected: 2 YES + 2 silent(NO) → tied → fails
    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_4,
                PROPOSAL_EXPIRATION_TIME,
                false,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("first vote should succeed");

    let voter2 = PrivateKeySigner::random();
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, voter2)
        .await
        .expect("second vote should succeed");

    let err_first = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect_err("first timeout should fail consensus");
    assert!(matches!(
        err_first,
        ConsensusError::InsufficientVotesAtTimeout
    ));

    let err_second = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect_err("second timeout should keep failed consensus");
    assert!(matches!(
        err_second,
        ConsensusError::InsufficientVotesAtTimeout
    ));

    let state = service
        .get_consensus_result(&scope, proposal.proposal_id)
        .await;
    assert!(matches!(state, Err(ConsensusError::ConsensusFailed)));
}

#[tokio::test]
async fn test_handle_consensus_timeout_rejects_unknown_scope() {
    let service = DefaultConsensusService::default();
    let unknown_scope = ScopeID::from("unknown_scope");

    let err = service
        .handle_consensus_timeout(&unknown_scope, 1)
        .await
        .expect_err("unknown scope timeout handling should fail");

    assert!(
        matches!(err, ConsensusError::SessionNotFound),
        "should return SessionNotFound for unknown scope"
    );
}

#[tokio::test]
async fn test_cast_vote_still_active_does_not_emit_consensus_event() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from("still_active_no_event_scope");
    let proposal_owner = PrivateKeySigner::random();

    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_4,
        true,
        ConsensusConfig::gossipsub(),
    )
    .await;

    // One vote is insufficient for n=4; transition should remain StillActive.
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("vote should succeed");

    let no_event = timeout(Duration::from_millis(150), events.recv()).await;
    assert!(
        no_event.is_err(),
        "no terminal consensus event should be emitted while session is still active"
    );
}

#[tokio::test]
async fn test_process_incoming_proposal_resolve_config_uses_base_timeout_when_expiration_not_after_timestamp()
 {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("resolve_config_base_timeout_scope");

    let request = CreateProposalRequest::new(
        PROPOSAL_NAME.to_string(),
        PROPOSAL_PAYLOAD,
        vec![1u8; 20],
        EXPECTED_VOTERS_COUNT_3,
        PROPOSAL_EXPIRATION_TIME,
        false, // ensure liveness comes from proposal fields
    )
    .expect("valid proposal request");

    let mut incoming = request.into_proposal().expect("proposal");

    // Force expiration_timestamp <= timestamp while keeping both in the future,
    // so proposal remains non-expired but resolve_config must fall back to base timeout.
    let future = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_secs()
        + 120;
    incoming.timestamp = future;
    incoming.expiration_timestamp = future;

    service
        .process_incoming_proposal(&scope, incoming.clone())
        .await
        .expect("incoming proposal should be accepted");

    let resolved = service
        .get_proposal_config(&scope, incoming.proposal_id)
        .await
        .expect("resolved config");

    assert_eq!(
        resolved.consensus_timeout(),
        ConsensusConfig::gossipsub().consensus_timeout(),
        "timeout should use base config when expiration_timestamp <= timestamp"
    );
    assert!(
        !resolved.liveness_criteria(),
        "liveness criteria should still be sourced from proposal fields"
    );
}

#[tokio::test]
async fn test_get_reached_proposals_with_consensus() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create a proposal that will reach consensus
    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_1,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    // Cast vote to reach consensus
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("vote should succeed");

    // Get reached proposals
    let reached = service
        .get_reached_proposals(&scope)
        .await
        .expect("should not return error");

    assert!(reached.is_some(), "should have reached proposals");
    let reached_map = reached.unwrap();
    assert_eq!(
        reached_map.len(),
        1,
        "should have exactly one reached proposal"
    );
    assert!(
        reached_map.contains_key(&proposal.proposal_id),
        "should contain the proposal"
    );
    assert_eq!(
        reached_map.get(&proposal.proposal_id),
        Some(&true),
        "proposal should have YES consensus result"
    );
}

#[tokio::test]
async fn test_get_reached_proposals_no_consensus() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create a proposal that won't reach consensus
    let _proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner),
                EXPECTED_VOTERS_COUNT_3,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    // Don't cast enough votes - proposal remains active

    // Get reached proposals
    let reached = service
        .get_reached_proposals(&scope)
        .await
        .expect("should not return error");

    assert!(
        reached.is_none(),
        "should return None when no proposals have reached consensus"
    );
}

#[tokio::test]
async fn test_get_reached_proposals_mixed_states() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner1 = PrivateKeySigner::random();
    let proposal_owner2 = PrivateKeySigner::random();
    let proposal_owner3 = PrivateKeySigner::random();

    // Create proposal 1 that reaches consensus (YES)
    let proposal1 = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner1),
                EXPECTED_VOTERS_COUNT_1,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    service
        .cast_vote(&scope, proposal1.proposal_id, true, proposal_owner1)
        .await
        .expect("vote should succeed");

    // Create proposal 2 that reaches consensus (NO)
    let proposal2 = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner2),
                EXPECTED_VOTERS_COUNT_1,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    service
        .cast_vote(&scope, proposal2.proposal_id, false, proposal_owner2)
        .await
        .expect("vote should succeed");

    // Create proposal 3 that remains active (doesn't reach consensus)
    let proposal3 = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner_from_signer(&proposal_owner3),
                EXPECTED_VOTERS_COUNT_3,
                PROPOSAL_EXPIRATION_TIME,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal should be created");

    // Don't cast enough votes for proposal3

    // Get reached proposals
    let reached = service
        .get_reached_proposals(&scope)
        .await
        .expect("should not return error");

    assert!(reached.is_some(), "should have reached proposals");
    let reached_map = reached.unwrap();
    assert_eq!(
        reached_map.len(),
        2,
        "should have exactly two reached proposals"
    );
    assert!(
        reached_map.contains_key(&proposal1.proposal_id),
        "should contain proposal1"
    );
    assert!(
        reached_map.contains_key(&proposal2.proposal_id),
        "should contain proposal2"
    );
    assert!(
        !reached_map.contains_key(&proposal3.proposal_id),
        "should not contain active proposal3"
    );
    assert_eq!(
        reached_map.get(&proposal1.proposal_id),
        Some(&true),
        "proposal1 should have YES consensus"
    );
    assert_eq!(
        reached_map.get(&proposal2.proposal_id),
        Some(&false),
        "proposal2 should have NO consensus"
    );
}

#[tokio::test]
async fn test_get_reached_proposals_nonexistent_scope() {
    let service = DefaultConsensusService::default();
    let nonexistent_scope = ScopeID::from("nonexistent");

    // Get reached proposals for non-existent scope
    let result = service.get_reached_proposals(&nonexistent_scope).await;

    assert!(
        result.is_err(),
        "should return error for non-existent scope"
    );
    assert!(
        matches!(result.unwrap_err(), ConsensusError::ScopeNotFound),
        "should return ScopeNotFound error"
    );
}

#[tokio::test]
async fn test_unknown_scope_queries_stats_and_active_proposals() {
    let service = DefaultConsensusService::default();
    let unknown_scope = ScopeID::from("unknown_scope");

    let stats = service.get_scope_stats(&unknown_scope).await;
    assert_eq!(
        stats.total_sessions, 0,
        "unknown scope should have zero total sessions"
    );
    assert_eq!(
        stats.active_sessions, 0,
        "unknown scope should have zero active sessions"
    );
    assert_eq!(
        stats.consensus_reached, 0,
        "unknown scope should have zero reached consensus sessions"
    );
    assert_eq!(
        stats.failed_sessions, 0,
        "unknown scope should have zero failed sessions"
    );

    let active_result = service.get_active_proposals(&unknown_scope).await;
    assert!(
        matches!(active_result, Err(ConsensusError::ScopeNotFound)),
        "get_active_proposals should return ScopeNotFound for unknown scope"
    );
}
