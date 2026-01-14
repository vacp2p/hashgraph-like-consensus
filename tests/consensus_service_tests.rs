use alloy::signers::local::PrivateKeySigner;
use std::time::Duration;
use tokio::time::timeout;

use hashgraph_like_consensus::{
    error::ConsensusError,
    scope::ScopeID,
    service::DefaultConsensusService,
    session::ConsensusConfig,
    types::{ConsensusEvent, CreateProposalRequest},
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

    // Cast votes to reach consensus
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("first vote");

    let voter2 = PrivateKeySigner::random();
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, voter2)
        .await
        .expect("second vote");

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

    // Cast 2 YES votes (need 2 for threshold with 3 expected voters)
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("first vote");

    let voter2 = PrivateKeySigner::random();
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, voter2)
        .await
        .expect("second vote");

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
async fn test_handle_consensus_timeout_insufficient_votes() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create proposal with insufficient votes
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

    // Cast only 1 vote (need at least 3 for threshold with 4 expected voters)
    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("first vote");

    // Call handle_consensus_timeout - should fail with insufficient votes
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

    // Verify session is marked as Failed (no consensus result and not in active proposals)
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
async fn test_handle_consensus_timeout_no_votes() {
    let service = DefaultConsensusService::default();
    let mut events = service.subscribe_to_events();
    let scope = ScopeID::from(SCOPE1_NAME);
    let proposal_owner = PrivateKeySigner::random();

    // Create proposal but don't cast any votes
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

    // Call handle_consensus_timeout with no votes
    let err = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .await
        .expect_err("should fail with no votes");

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

    // Verify session is marked as Failed (no consensus result and not in active proposals)
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
