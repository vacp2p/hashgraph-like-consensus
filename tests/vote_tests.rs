use alloy::signers::local::PrivateKeySigner;

use hashgraph_like_consensus::{
    scope::ScopeID,
    service::DefaultConsensusService,
    session::ConsensusConfig,
    signing::{ConsensusSignatureScheme, EthereumConsensusSigner},
    types::CreateProposalRequest,
    utils::{build_vote, validate_proposal},
};

fn wrap(signer: PrivateKeySigner) -> EthereumConsensusSigner {
    EthereumConsensusSigner::new(signer)
}

const SCOPE: &str = "vote_scope";
const PROPOSAL_NAME: &str = "Vote Test Proposal";
const PROPOSAL_PAYLOAD: Vec<u8> = vec![];

const EXPIRATION: u64 = 120;
const EXPECTED_VOTERS_COUNT: u32 = 3;

const VOTE_YES: bool = true;
const VOTE_NO: bool = false;

#[test]
fn test_received_hash_for_new_voter() {
    let proposal_owner = wrap(PrivateKeySigner::random());
    let service = DefaultConsensusService::new(proposal_owner.clone());
    let scope = ScopeID::from(SCOPE);

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner.identity().to_vec(),
                EXPECTED_VOTERS_COUNT,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .expect("proposal");

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES)
        .expect("proposal_owner vote");

    let other_voter = wrap(PrivateKeySigner::random());
    let vote = build_vote(&proposal, VOTE_YES, &other_voter).expect("second vote");

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
    validate_proposal::<EthereumConsensusSigner>(&proposal_with_vote)
        .expect("proposal with second voter should validate");
}

#[test]
fn test_parent_hash_for_same_voter() {
    let proposal_owner = wrap(PrivateKeySigner::random());
    let service = DefaultConsensusService::new(proposal_owner.clone());
    let scope = ScopeID::from(SCOPE);

    let proposal = service
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                proposal_owner.identity().to_vec(),
                EXPECTED_VOTERS_COUNT,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .expect("proposal");

    let proposal = service
        .cast_vote_and_get_proposal(&scope, proposal.proposal_id, VOTE_YES)
        .expect("proposal_owner vote");

    // Create a second vote from the same voter to exercise parent_hash logic.
    let second_vote = build_vote(&proposal, VOTE_NO, &proposal_owner).expect("second vote");

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
    validate_proposal::<EthereumConsensusSigner>(&proposal_with_vote)
        .expect("proposal with parent hash chain should validate");
}
