mod common;
use common::now_ts;

use alloy::signers::{SignerSync, local::PrivateKeySigner};
use hashgraph_like_consensus::signing::EthereumConsensusSigner;

use prost::Message;

use hashgraph_like_consensus::{
    error::ConsensusError,
    scope::ScopeID,
    service::DefaultConsensusService,
    session::ConsensusConfig,
    storage::ConsensusStorage,
    types::CreateProposalRequest,
    utils::{build_vote, compute_vote_hash, validate_proposal},
};

fn cast_remote_vote(
    service: &DefaultConsensusService,
    scope: &ScopeID,
    proposal_id: u32,
    choice: bool,
    signer: &EthereumConsensusSigner,
) -> Result<
    hashgraph_like_consensus::protos::consensus::v1::Vote,
    hashgraph_like_consensus::error::ConsensusError,
> {
    let proposal = service.storage().get_proposal(scope, proposal_id)?;
    let vote = build_vote(&proposal, choice, signer, now_ts())?;
    service.process_incoming_vote(scope, vote.clone(), now_ts())?;
    Ok(vote)
}

fn cast_remote_vote_and_get_proposal(
    service: &DefaultConsensusService,
    scope: &ScopeID,
    proposal_id: u32,
    choice: bool,
    signer: &EthereumConsensusSigner,
) -> Result<
    hashgraph_like_consensus::protos::consensus::v1::Proposal,
    hashgraph_like_consensus::error::ConsensusError,
> {
    cast_remote_vote(service, scope, proposal_id, choice, signer)?;
    service.storage().get_proposal(scope, proposal_id)
}

fn make_service() -> DefaultConsensusService {
    DefaultConsensusService::new(EthereumConsensusSigner::new(PrivateKeySigner::random()))
}

fn wrap(signer: PrivateKeySigner) -> EthereumConsensusSigner {
    EthereumConsensusSigner::new(signer)
}

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

fn resign_vote(
    vote: &mut hashgraph_like_consensus::protos::consensus::v1::Vote,
    signer: &PrivateKeySigner,
) {
    vote.vote_hash = compute_vote_hash(vote);
    vote.signature.clear();
    let vote_bytes = vote.encode_to_vec();
    vote.signature = signer
        .sign_message_sync(&vote_bytes)
        .expect("vote should be resignable")
        .as_bytes()
        .to_vec();
}

#[test]
fn test_vote_created_with_helper_is_valid() {
    let service = make_service();
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
            now_ts(),
        )
        .expect("proposal");

    let proposal = cast_remote_vote_and_get_proposal(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("proposal_owner vote");

    let voter = PrivateKeySigner::random();
    let vote =
        build_vote(&proposal, VOTE_YES, &wrap(voter), now_ts()).expect("vote should be created");

    service
        .process_incoming_vote(&scope, vote, now_ts())
        .expect("vote should validate");
}

#[test]
fn test_invalid_signature_is_rejected() {
    let service = make_service();
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
            now_ts(),
        )
        .expect("proposal");

    let proposal = cast_remote_vote_and_get_proposal(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("proposal_owner vote");

    let voter = PrivateKeySigner::random();
    let mut vote = build_vote(&proposal, VOTE_YES, &wrap(voter), now_ts()).expect("vote");

    let wrong_signer = PrivateKeySigner::random();
    let vote_bytes = vote.encode_to_vec();
    let wrong_sig = wrong_signer
        .sign_message_sync(&vote_bytes)
        .expect("should sign with wrong key");
    vote.signature = wrong_sig.as_bytes().to_vec();

    let mut invalid_proposal = proposal.clone();
    invalid_proposal.votes.push(vote);

    let err = validate_proposal::<EthereumConsensusSigner>(&invalid_proposal, now_ts())
        .expect_err("validation should fail");
    assert!(
        matches!(err, ConsensusError::InvalidVoteSignature),
        "error: {err:?}"
    );
}

#[test]
fn test_vote_chain_validation_rejects_bad_received_hash() {
    let service = make_service();
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
            now_ts(),
        )
        .expect("proposal");

    let proposal = cast_remote_vote_and_get_proposal(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("proposal_owner vote");

    let voter_one = PrivateKeySigner::random();
    let voter_two = PrivateKeySigner::random();

    let vote_one = build_vote(&proposal, VOTE_YES, &wrap(voter_one), now_ts()).expect("vote one");
    let mut vote_two =
        build_vote(&proposal, VOTE_NO, &wrap(voter_two.clone()), now_ts()).expect("vote two");

    vote_two.received_hash = vec![0; 32];
    vote_two.vote_hash = compute_vote_hash(&vote_two);
    vote_two.signature.clear();
    let vote_bytes = vote_two.encode_to_vec();
    vote_two.signature = voter_two
        .sign_message_sync(&vote_bytes)
        .expect("sign corrupted vote")
        .as_bytes()
        .to_vec();

    let mut invalid = proposal.clone();
    invalid.votes.push(vote_one);
    invalid.votes.push(vote_two);

    let err = validate_proposal::<EthereumConsensusSigner>(&invalid, now_ts())
        .expect_err("should fail chain validation");
    assert!(
        matches!(err, ConsensusError::ReceivedHashMismatch),
        "error: {err:?}"
    );
}

#[test]
fn test_validate_proposal_rejects_empty_vote_owner() {
    let service = make_service();
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
            now_ts(),
        )
        .expect("proposal");

    let mut vote = build_vote(&proposal, VOTE_YES, &wrap(proposal_owner), now_ts()).expect("vote");
    vote.vote_owner.clear();

    let mut invalid = proposal;
    invalid.votes.push(vote);

    let err = validate_proposal::<EthereumConsensusSigner>(&invalid, now_ts())
        .expect_err("empty vote owner should fail");
    assert!(matches!(err, ConsensusError::EmptyVoteOwner));
}

#[test]
fn test_validate_proposal_rejects_empty_vote_hash() {
    let service = make_service();
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
            now_ts(),
        )
        .expect("proposal");

    let mut vote = build_vote(&proposal, VOTE_YES, &wrap(proposal_owner), now_ts()).expect("vote");
    vote.vote_hash.clear();

    let mut invalid = proposal;
    invalid.votes.push(vote);

    let err = validate_proposal::<EthereumConsensusSigner>(&invalid, now_ts())
        .expect_err("empty vote hash should fail");
    assert!(matches!(err, ConsensusError::EmptyVoteHash));
}

#[test]
fn test_validate_proposal_rejects_empty_signature() {
    let service = make_service();
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
            now_ts(),
        )
        .expect("proposal");

    let mut vote = build_vote(&proposal, VOTE_YES, &wrap(proposal_owner), now_ts()).expect("vote");
    vote.signature.clear();

    let mut invalid = proposal;
    invalid.votes.push(vote);

    let err = validate_proposal::<EthereumConsensusSigner>(&invalid, now_ts())
        .expect_err("empty signature should fail");
    assert!(matches!(err, ConsensusError::EmptySignature));
}

#[test]
fn test_validate_proposal_rejects_mismatched_signature_length() {
    let service = make_service();
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
            now_ts(),
        )
        .expect("proposal");

    let mut vote = build_vote(&proposal, VOTE_YES, &wrap(proposal_owner), now_ts()).expect("vote");
    vote.signature = vec![7; 64];

    let mut invalid = proposal;
    invalid.votes.push(vote);

    let err = validate_proposal::<EthereumConsensusSigner>(&invalid, now_ts())
        .expect_err("invalid signature length should fail");
    // Signature-length checks now live in the scheme and surface as a
    // SignatureScheme error rather than a protocol-level MismatchedLength variant.
    assert!(matches!(err, ConsensusError::SignatureScheme(_)));
}

#[test]
fn test_vote_chain_validation_rejects_bad_parent_hash_owner_mismatch() {
    let service = make_service();
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
            now_ts(),
        )
        .expect("proposal");

    let voter_one = PrivateKeySigner::random();
    let voter_two = PrivateKeySigner::random();

    let vote_one = build_vote(&proposal, VOTE_YES, &wrap(voter_one), now_ts()).expect("vote one");
    let mut vote_two =
        build_vote(&proposal, VOTE_NO, &wrap(voter_two.clone()), now_ts()).expect("vote two");

    // parent_hash points to another owner's vote, which should fail RFC parent-chain checks.
    vote_two.parent_hash = vote_one.vote_hash.clone();
    resign_vote(&mut vote_two, &voter_two);

    let mut invalid = proposal;
    invalid.votes.push(vote_one);
    invalid.votes.push(vote_two);

    let err = validate_proposal::<EthereumConsensusSigner>(&invalid, now_ts())
        .expect_err("parent hash owner mismatch should fail");
    assert!(matches!(err, ConsensusError::ParentHashMismatch));
}
