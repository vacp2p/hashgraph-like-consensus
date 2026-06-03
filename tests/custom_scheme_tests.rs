//! End-to-end test that exercises [`ConsensusService`] with a non-Ethereum
//! signature scheme. Proves the [`ConsensusSignatureScheme`] abstraction
//! holds for embedders integrating Ed25519, HSMs, libchat accounts, etc.
//!
//! The stub scheme used here is intentionally trivial — signatures are
//! `SHA256(identity || payload)` — which is enough to demonstrate that votes
//! signed by one peer and validated by another round-trip cleanly through the
//! service without any Ethereum-specific assumptions.

use hashgraph_like_consensus::{
    events::BroadcastEventBus,
    scope::ScopeID,
    service::ConsensusService,
    session::ConsensusConfig,
    signing::{ConsensusSchemeError, ConsensusSignatureScheme},
    storage::{ConsensusStorage, InMemoryConsensusStorage},
    types::CreateProposalRequest,
    utils::build_vote,
};
use sha2::{Digest, Sha256};

const STUB_IDENTITY_LEN: usize = 8;

/// Trivial signature scheme used to validate the generic plumbing.
///
/// **Not cryptographically secure** — any holder of the identity can forge
/// signatures. Only suitable as a test stub for verifying that
/// [`ConsensusService`] does not bake in Ethereum-specific assumptions.
#[derive(Debug, Clone)]
struct StubSigner {
    identity: [u8; STUB_IDENTITY_LEN],
}

impl StubSigner {
    fn new(identity: [u8; STUB_IDENTITY_LEN]) -> Self {
        Self { identity }
    }

    fn expected_signature(identity: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut h = Sha256::new();
        h.update(identity);
        h.update(payload);
        h.finalize().to_vec()
    }
}

impl ConsensusSignatureScheme for StubSigner {
    fn identity(&self) -> &[u8] {
        &self.identity
    }

    fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, ConsensusSchemeError> {
        Ok(Self::expected_signature(&self.identity, payload))
    }

    fn verify(
        identity: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, ConsensusSchemeError> {
        if identity.len() != STUB_IDENTITY_LEN {
            return Err(ConsensusSchemeError::Verify(format!(
                "stub identity must be {STUB_IDENTITY_LEN} bytes, got {}",
                identity.len()
            )));
        }
        Ok(signature == Self::expected_signature(identity, payload))
    }
}

type StubService = ConsensusService<
    ScopeID,
    InMemoryConsensusStorage<ScopeID>,
    BroadcastEventBus<ScopeID>,
    StubSigner,
>;

/// Build a per-peer service sharing storage and event bus.
fn peer_service(
    storage: &InMemoryConsensusStorage<ScopeID>,
    bus: &BroadcastEventBus<ScopeID>,
    signer: StubSigner,
) -> StubService {
    StubService::new_with_components(storage.clone(), bus.clone(), signer, 10)
}

#[test]
fn stub_scheme_reaches_consensus_without_ethereum_types() {
    let storage = InMemoryConsensusStorage::<ScopeID>::new();
    let bus = BroadcastEventBus::<ScopeID>::default();
    let scope = ScopeID::from("stub-scope");

    let owner = peer_service(&storage, &bus, StubSigner::new([1; STUB_IDENTITY_LEN]));
    let voter_two = peer_service(&storage, &bus, StubSigner::new([2; STUB_IDENTITY_LEN]));
    let voter_three = peer_service(&storage, &bus, StubSigner::new([3; STUB_IDENTITY_LEN]));

    let proposal = owner
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                "stub-proposal".into(),
                b"payload".to_vec(),
                owner.signer().identity().to_vec(),
                3,
                60,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .expect("proposal should be created");

    owner
        .cast_vote(&scope, proposal.proposal_id, true)
        .expect("owner vote");
    voter_two
        .cast_vote(&scope, proposal.proposal_id, true)
        .expect("voter two");
    voter_three
        .cast_vote(&scope, proposal.proposal_id, true)
        .expect("voter three");

    let session = owner
        .storage()
        .get_session(&scope, proposal.proposal_id)
        .expect("get session")
        .expect("session exists");
    assert!(
        session.get_consensus_result().expect("consensus reached"),
        "3 YES votes via stub scheme should reach consensus"
    );
}

#[test]
fn stub_scheme_rejects_forged_signature() {
    let storage = InMemoryConsensusStorage::<ScopeID>::new();
    let bus = BroadcastEventBus::<ScopeID>::default();
    let scope = ScopeID::from("stub-scope-forge");

    let owner = peer_service(&storage, &bus, StubSigner::new([9; STUB_IDENTITY_LEN]));
    let voter = StubSigner::new([10; STUB_IDENTITY_LEN]);

    let proposal = owner
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                "stub-proposal".into(),
                b"payload".to_vec(),
                owner.signer().identity().to_vec(),
                2,
                60,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .expect("proposal should be created");

    let mut vote = build_vote(&proposal, true, &voter).expect("vote");
    // Tamper with the signature so verify() returns false.
    vote.signature.iter_mut().for_each(|b| *b ^= 0xFF);

    let err = owner
        .process_incoming_vote(&scope, vote)
        .expect_err("forged signature must be rejected");
    assert!(
        matches!(
            err,
            hashgraph_like_consensus::error::ConsensusError::InvalidVoteSignature
        ),
        "expected InvalidVoteSignature, got {err:?}"
    );
}
