use futures::StreamExt;

use hashgraph_like_consensus::{
    scope::ScopeID,
    session::{ConsensusConfig, ConsensusSession},
    storage::{ConsensusStorage, InMemoryConsensusStorage},
    types::CreateProposalRequest,
};

const SCOPE: &str = "stream_scope";
const MISSING_SCOPE: &str = "missing_stream_scope";
const PROPOSAL_PAYLOAD: Vec<u8> = vec![];
const EXPIRATION: u64 = 120;
const EXPECTED_VOTERS_COUNT: u32 = 3;

fn make_session(name: &str) -> ConsensusSession {
    let proposal = CreateProposalRequest::new(
        name.to_string(),
        PROPOSAL_PAYLOAD,
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

#[tokio::test]
async fn test_remove_list_scopes_and_replace_scope_sessions() {
    let storage: InMemoryConsensusStorage<ScopeID> = InMemoryConsensusStorage::new();
    let scope = ScopeID::from("remove_replace_scope");

    // Empty storage should return None for scopes and scope sessions.
    assert!(storage.list_scopes().await.expect("list scopes").is_none());
    assert!(
        storage
            .list_scope_sessions(&scope)
            .await
            .expect("list scope sessions")
            .is_none()
    );

    // Save one session and ensure scope appears.
    let session = make_session("remove-target");
    let proposal_id = session.proposal.proposal_id;
    storage
        .save_session(&scope, session.clone())
        .await
        .expect("save session");

    let scopes = storage
        .list_scopes()
        .await
        .expect("list scopes")
        .expect("should have one scope");
    assert_eq!(scopes, vec![scope.clone()]);

    // Remove existing and then missing session.
    let removed = storage
        .remove_session(&scope, proposal_id)
        .await
        .expect("remove existing");
    assert!(removed.is_some());

    let removed_missing = storage
        .remove_session(&scope, proposal_id)
        .await
        .expect("remove missing");
    assert!(removed_missing.is_none());

    // Replace scope sessions with two entries and verify retrieval.
    let replacement1 = make_session("replacement1");
    let replacement2 = make_session("replacement2");
    storage
        .replace_scope_sessions(&scope, vec![replacement1.clone(), replacement2.clone()])
        .await
        .expect("replace scope sessions");

    let listed = storage
        .list_scope_sessions(&scope)
        .await
        .expect("list after replace")
        .expect("scope must exist after replace");
    assert_eq!(listed.len(), 2);
}

#[tokio::test]
async fn test_update_session_and_update_scope_sessions_error_and_cleanup_paths() {
    let storage: InMemoryConsensusStorage<ScopeID> = InMemoryConsensusStorage::new();
    let scope = ScopeID::from("update_scope_sessions_scope");

    let session = make_session("updatable");
    let proposal_id = session.proposal.proposal_id;
    storage
        .save_session(&scope, session)
        .await
        .expect("save session");

    // update_session success path.
    let updated_name = storage
        .update_session(&scope, proposal_id, |session| {
            session.proposal.name = "mutated".to_string();
            Ok(session.proposal.name.clone())
        })
        .await
        .expect("update session");
    assert_eq!(updated_name, "mutated");

    // update_session missing path.
    let err = storage
        .update_session(&scope, u32::MAX, |_session| Ok(()))
        .await
        .expect_err("missing session should fail");
    assert!(matches!(
        err,
        hashgraph_like_consensus::error::ConsensusError::SessionNotFound
    ));

    // update_scope_sessions mutator error path should surface error.
    let err = storage
        .update_scope_sessions(&scope, |_sessions| {
            Err(hashgraph_like_consensus::error::ConsensusError::ConsensusFailed)
        })
        .await
        .expect_err("mutator error should bubble up");
    assert!(matches!(
        err,
        hashgraph_like_consensus::error::ConsensusError::ConsensusFailed
    ));

    // update_scope_sessions empty vector path should remove scope entry.
    storage
        .update_scope_sessions(&scope, |sessions| {
            sessions.clear();
            Ok(())
        })
        .await
        .expect("cleanup update");

    assert!(
        storage
            .list_scope_sessions(&scope)
            .await
            .expect("list after cleanup")
            .is_none()
    );
}

#[tokio::test]
async fn test_scope_config_storage_validation_and_updates() {
    use hashgraph_like_consensus::{
        error::ConsensusError,
        scope_config::{NetworkType, ScopeConfig},
    };

    let storage: InMemoryConsensusStorage<ScopeID> = InMemoryConsensusStorage::new();
    let scope = ScopeID::from("scope_config_storage_scope");

    assert!(
        storage
            .get_scope_config(&scope)
            .await
            .expect("get config")
            .is_none()
    );

    // set_scope_config validation failure path.
    let invalid = ScopeConfig {
        network_type: NetworkType::Gossipsub,
        default_consensus_threshold: 2.0 / 3.0,
        default_timeout: std::time::Duration::from_secs(60),
        default_liveness_criteria_yes: true,
        max_rounds_override: Some(0),
    };
    let err = storage
        .set_scope_config(&scope, invalid)
        .await
        .expect_err("invalid config should fail");
    assert!(matches!(err, ConsensusError::InvalidMaxRounds));

    // update_scope_config creates default when missing and persists valid change.
    storage
        .update_scope_config(&scope, |config| {
            config.network_type = NetworkType::P2P;
            config.max_rounds_override = Some(0);
            Ok(())
        })
        .await
        .expect("update scope config");

    let cfg = storage
        .get_scope_config(&scope)
        .await
        .expect("get updated config")
        .expect("config should exist");
    assert_eq!(cfg.network_type, NetworkType::P2P);
    assert_eq!(cfg.max_rounds_override, Some(0));

    // updater error path.
    let err = storage
        .update_scope_config(&scope, |_config| Err(ConsensusError::ConsensusFailed))
        .await
        .expect_err("updater error should fail");
    assert!(matches!(err, ConsensusError::ConsensusFailed));

    // post-update validation failure path.
    let err = storage
        .update_scope_config(&scope, |config| {
            config.network_type = NetworkType::Gossipsub;
            config.max_rounds_override = Some(0);
            Ok(())
        })
        .await
        .expect_err("invalid updated config should fail validation");
    assert!(matches!(err, ConsensusError::InvalidMaxRounds));
}
