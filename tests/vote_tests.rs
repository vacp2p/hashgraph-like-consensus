use alloy::signers::local::PrivateKeySigner;

use hashgraph_like_consensus::{
    scope::ScopeID,
    service::DefaultConsensusService,
    session::ConsensusConfig,
    types::CreateProposalRequest,
    utils::{build_vote, validate_proposal},
};

const SCOPE: &str = "vote_scope";
const PROPOSAL_NAME: &str = "Vote Test Proposal";
const PROPOSAL_PAYLOAD: &str = "";

const EXPIRATION: u64 = 120;
const EXPECTED_VOTERS_COUNT: u32 = 3;

const VOTE_YES: bool = true;
const VOTE_NO: bool = false;

fn owner_bytes(signer: &PrivateKeySigner) -> Vec<u8> {
    signer.address().as_slice().to_vec()
}

#[tokio::test]
async fn test_received_hash_for_new_voter() {
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
                EXPECTED_VOTERS_COUNT,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal");

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("proposal_owner vote");

    let other_voter = PrivateKeySigner::random();
    let vote = build_vote(&proposal, VOTE_YES, other_voter)
        .await
        .expect("second vote");

    assert!(
        vote.parent_hash.is_empty(),
        "new voter should have empty parent"
    );
    assert_eq!(
        vote.received_hash, proposal.votes[0].vote_hash,
        "received_hash should reference latest vote"
    );

    let mut proposal_with_vote = proposal.clone();
    proposal_with_vote.votes.push(vote);
    validate_proposal(&proposal_with_vote).expect("proposal with second voter should validate");
}

#[tokio::test]
async fn test_parent_hash_for_same_voter() {
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
                EXPECTED_VOTERS_COUNT,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("proposal");

    let proposal = service
        .cast_vote_and_get_proposal(
            &scope,
            proposal.proposal_id,
            VOTE_YES,
            proposal_owner.clone(),
        )
        .await
        .expect("proposal_owner vote");

    // Create a second vote from the same voter to exercise parent_hash logic.
    let second_vote = build_vote(&proposal, VOTE_NO, proposal_owner)
        .await
        .expect("second vote");

    assert!(
        second_vote.received_hash == proposal.votes[0].vote_hash,
        "same voter should chain received_hash to previous vote"
    );
    assert_eq!(
        second_vote.parent_hash, proposal.votes[0].vote_hash,
        "parent_hash should reference prior vote from same voter"
    );

    let mut proposal_with_vote = proposal.clone();
    proposal_with_vote.votes.push(second_vote);
    validate_proposal(&proposal_with_vote)
        .expect("proposal with parent hash chain should validate");
}
