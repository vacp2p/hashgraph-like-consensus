use futures::StreamExt;

use hashgraph_like_consensus::{
    scope::ScopeID,
    session::{ConsensusConfig, ConsensusSession},
    storage::{ConsensusStorage, InMemoryConsensusStorage},
    types::CreateProposalRequest,
};

const SCOPE: &str = "stream_scope";
const MISSING_SCOPE: &str = "missing_stream_scope";
const PROPOSAL_PAYLOAD: &str = "";
const EXPIRATION: u64 = 120;
const EXPECTED_VOTERS_COUNT: u32 = 3;

fn make_session(name: &str) -> ConsensusSession {
    let proposal = CreateProposalRequest::new(
        name.to_string(),
        PROPOSAL_PAYLOAD.to_string(),
        vec![1, 2, 3], // arbitrary "owner" bytes; signature not needed for storage tests
        EXPECTED_VOTERS_COUNT,
        EXPIRATION,
        true,
    )
    .expect("valid proposal request")
    .into_proposal()
    .expect("proposal");

    let (session, _) =
        ConsensusSession::from_proposal(proposal, ConsensusConfig::gossipsub()).expect("session");
    session
}

#[tokio::test]
async fn test_stream_scope_sessions_yields_all_sessions_in_scope() {
    let storage: InMemoryConsensusStorage<ScopeID> = InMemoryConsensusStorage::new();
    let scope = ScopeID::from(SCOPE);

    let session1 = make_session("p1");
    let session2 = make_session("p2");

    storage
        .save_session(&scope, session1.clone())
        .await
        .expect("save session1");
    storage
        .save_session(&scope, session2.clone())
        .await
        .expect("save session2");

    let mut got_ids: Vec<u32> = storage
        .stream_scope_sessions(&scope)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|r| r.expect("stream item").proposal.proposal_id)
        .collect();
    got_ids.sort_unstable();

    let mut expected_ids = vec![session1.proposal.proposal_id, session2.proposal.proposal_id];
    expected_ids.sort_unstable();

    assert_eq!(got_ids, expected_ids);
}

#[tokio::test]
async fn test_stream_scope_sessions_missing_scope_is_empty() {
    let storage: InMemoryConsensusStorage<ScopeID> = InMemoryConsensusStorage::new();
    let missing_scope = ScopeID::from(MISSING_SCOPE);

    let items: Vec<_> = storage
        .stream_scope_sessions(&missing_scope)
        .collect::<Vec<_>>()
        .await;
    assert!(items.is_empty());
}
