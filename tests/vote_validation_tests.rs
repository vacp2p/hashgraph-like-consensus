use alloy::signers::{Signer, local::PrivateKeySigner};

use prost::Message;

use hashgraph_like_consensus::{
    error::ConsensusError,
    scope::ScopeID,
    service::DefaultConsensusService,
    session::ConsensusConfig,
    types::CreateProposalRequest,
    utils::{build_vote, compute_vote_hash, validate_proposal},
};

const SCOPE: &str = "validation_scope";
const PROPOSAL_NAME: &str = "Proposal";
const PROPOSAL_PAYLOAD: Vec<u8> = vec![];

const EXPIRATION: u64 = 120;

const EXPECTED_VOTERS_COUNT_3: u32 = 3;
const EXPECTED_VOTERS_COUNT_2: u32 = 2;

const VOTE_YES: bool = true;
const VOTE_NO: bool = false;

fn owner_bytes(signer: &PrivateKeySigner) -> Vec<u8> {
    signer.address().as_slice().to_vec()
}

#[tokio::test]
async fn test_vote_created_with_helper_is_valid() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

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
        .expect("proposal");

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("proposal_owner vote");

    let voter = PrivateKeySigner::random();
    let vote = build_vote(&proposal, VOTE_YES, voter)
        .await
        .expect("vote should be created");

    service
        .process_incoming_vote(&scope, vote)
        .await
        .expect("vote should validate");
}

#[tokio::test]
async fn test_invalid_signature_is_rejected() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                owner_bytes(&proposal_owner),
                EXPECTED_VOTERS_COUNT_2,
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

    let voter = PrivateKeySigner::random();
    let mut vote = build_vote(&proposal, VOTE_YES, voter).await.expect("vote");

    let wrong_signer = PrivateKeySigner::random();
    let vote_bytes = vote.encode_to_vec();
    let wrong_sig = wrong_signer
        .sign_message(&vote_bytes)
        .await
        .expect("should sign with wrong key");
    vote.signature = wrong_sig.as_bytes().to_vec();

    let mut invalid_proposal = proposal.clone();
    invalid_proposal.votes.push(vote);

    let err = validate_proposal(&invalid_proposal).expect_err("validation should fail");
    assert!(
        matches!(err, ConsensusError::InvalidVoteSignature),
        "error: {err:?}"
    );
}

#[tokio::test]
async fn test_vote_chain_validation_rejects_bad_received_hash() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE);
    let proposal_owner = PrivateKeySigner::random();

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
        .expect("proposal");

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES, proposal_owner)
        .await
        .expect("proposal_owner vote");

    let voter_one = PrivateKeySigner::random();
    let voter_two = PrivateKeySigner::random();

    let vote_one = build_vote(&proposal, VOTE_YES, voter_one)
        .await
        .expect("vote one");
    let mut vote_two = build_vote(&proposal, VOTE_NO, voter_two.clone())
        .await
        .expect("vote two");

    vote_two.received_hash = vec![0; 32];
    vote_two.vote_hash = compute_vote_hash(&vote_two);
    vote_two.signature.clear();
    let vote_bytes = vote_two.encode_to_vec();
    vote_two.signature = voter_two
        .sign_message(&vote_bytes)
        .await
        .expect("sign corrupted vote")
        .as_bytes()
        .to_vec();

    let mut invalid = proposal.clone();
    invalid.votes.push(vote_one);
    invalid.votes.push(vote_two);

    let err = validate_proposal(&invalid).expect_err("should fail chain validation");
    assert!(
        matches!(err, ConsensusError::ReceivedHashMismatch),
        "error: {err:?}"
    );
}
