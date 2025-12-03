use alloy::signers::local::PrivateKeySigner;
use std::time::Duration;
use tokio::time::timeout;

use hashgraph_like_consensus::{
    scope::ScopeID,
    service::DefaultConsensusService,
    session::ConsensusConfig,
    types::{ConsensusEvent, CreateProposalRequest},
};

const SCOPE1_NAME: &str = "scope1";
const SCOPE2_NAME: &str = "scope2";
const PROPOSAL_NAME: &str = "Test Proposal";
const PROPOSAL_PAYLOAD: &str = "";
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
        .create_proposal_with_config(&scope, CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(), PROPOSAL_PAYLOAD.to_string(),
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

    assert_eq!(service.get_active_proposals(&scope).await.len(), 1);
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
        .create_proposal_with_config(&scope1, CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(), PROPOSAL_PAYLOAD.to_string(),
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
        .create_proposal_with_config(&scope2, CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(), PROPOSAL_PAYLOAD.to_string(),
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

    assert_eq!(service.get_active_proposals(&scope1).await.len(), 1);
    assert_eq!(service.get_active_proposals(&scope2).await.len(), 0); // scope2 reached consensus

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
        .create_proposal_with_config(&scope, CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(), PROPOSAL_PAYLOAD.to_string(),
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
