use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use hashgraph_like_consensus::signing::EthereumConsensusSigner;
use prost::Message;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use hashgraph_like_consensus::{
    error::ConsensusError,
    events::ConsensusEventBus,
    scope::ScopeID,
    service::DefaultConsensusService,
    session::ConsensusConfig,
    storage::ConsensusStorage,
    types::{ConsensusEvent, CreateProposalRequest},
    utils::{build_vote, compute_vote_hash},
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
    let vote = build_vote(&proposal, choice, signer)?;
    service.process_incoming_vote(scope, vote.clone())?;
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

fn setup_proposal(
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
        .expect("proposal should be created")
}

fn cast_vote_or_panic(
    service: &DefaultConsensusService,
    scope: &ScopeID,
    proposal_id: u32,
    choice: bool,
    signer: PrivateKeySigner,
    msg: &str,
) {
    cast_remote_vote(service, scope, proposal_id, choice, &wrap(signer)).expect(msg);
}

#[test]
fn test_basic_consensus_flow() {
    let service = make_service();
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
        .expect("proposal should be created");

    let proposal = cast_remote_vote_and_get_proposal(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("proposal_owner vote should succeed");

    {
        let active = service.storage().get_active_proposals(&scope).unwrap();
        assert_eq!(active.len(), 1);
    }
    let stats = service.get_scope_stats(&scope);
    assert_eq!(stats.total_sessions, 1);

    // Not enough votes yet -- consensus should not be reached
    let result = service
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id);
    assert!(
        matches!(result, Err(ConsensusError::ConsensusNotReached)),
        "should not have reached consensus with only 1 vote"
    );

    let voter_two = PrivateKeySigner::random();
    cast_remote_vote(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(voter_two),
    )
    .expect("second vote should succeed");

    let voter_three = PrivateKeySigner::random();
    cast_remote_vote(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(voter_three),
    )
    .expect("third vote should succeed");

    // Now consensus should be reached
    let result = service
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id);
    assert!(
        result.is_ok(),
        "consensus should be reached with 3 YES votes"
    );
}

#[test]
fn test_multi_scope_isolation() {
    let service = DefaultConsensusService::new_with_max_sessions(
        EthereumConsensusSigner::new(PrivateKeySigner::random()),
        5,
    );
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
        .expect("scope1 proposal");

    cast_remote_vote_and_get_proposal(
        &service,
        &scope1,
        proposal_1.proposal_id,
        VOTE_YES,
        &wrap(signer1),
    )
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
        .expect("scope2 proposal");

    cast_remote_vote_and_get_proposal(
        &service,
        &scope2,
        proposal_2.proposal_id,
        VOTE_YES,
        &wrap(signer2),
    )
    .expect("scope2 proposal_owner vote");

    {
        let active = service.storage().get_active_proposals(&scope1).unwrap();
        assert_eq!(active.len(), 1);
    }
    {
        let active = service.storage().get_active_proposals(&scope2).unwrap();
        assert_eq!(active.len(), 0); // scope2 reached consensus
    }

    let stats1 = service.get_scope_stats(&scope1);
    assert_eq!(stats1.total_sessions, 1);
    assert_eq!(stats1.active_sessions, 1);

    let stats2 = service.get_scope_stats(&scope2);
    assert_eq!(stats2.total_sessions, 1);
    assert_eq!(stats2.active_sessions, 0);
}

#[test]
fn test_consensus_threshold_emits_event() {
    let service = make_service();
    let events = service.event_bus().subscribe();
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
        .expect("proposal should be created");

    cast_remote_vote(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("proposal_owner vote");

    for _ in 0..3 {
        let signer = PrivateKeySigner::random();
        cast_remote_vote(
            &service,
            &scope,
            proposal.proposal_id,
            VOTE_YES,
            &wrap(signer),
        )
        .expect("additional vote");
    }

    let proposal_id = proposal.proposal_id;
    let result = (|| {
        while let Ok((event_scope, event)) = events.recv_timeout(Duration::from_secs(5)) {
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
    })()
    .expect("consensus event missing");

    assert!(result);
}

#[test]
fn test_handle_consensus_timeout_already_reached() {
    let service = make_service();
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
    );

    // Cast votes to reach consensus
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    );

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    );

    // Wait a bit to ensure consensus is reached
    std::thread::sleep(Duration::from_millis(100));

    // Now call handle_consensus_timeout - should return the already reached consensus
    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect("should return consensus result");

    assert!(result, "should return true (YES consensus)");
}

#[test]
fn test_handle_consensus_timeout_reaches_consensus() {
    let service = make_service();
    let events = service.event_bus().subscribe();
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
    );

    // Cast 2 YES votes (need 2 for threshold with 3 expected voters)
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    );

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    );

    // Call handle_consensus_timeout - should calculate consensus and reach it
    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect("should reach consensus");

    assert!(result, "should return true (YES consensus)");

    // Verify event was emitted
    let event_received = (|| {
        while let Ok((event_scope, event)) = events.recv_timeout(Duration::from_secs(1)) {
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
    })()
    .expect("consensus event should be emitted");

    assert!(event_received, "event should indicate YES consensus");
}

#[test]
fn test_handle_consensus_timeout_reaches_no_consensus_with_multiple_votes() {
    let service = make_service();
    let events = service.event_bus().subscribe();
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
    );

    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        true,
        yes_voter,
        "YES vote should succeed",
    );

    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        false,
        no_voter_1,
        "first NO vote should succeed",
    );

    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        false,
        no_voter_2,
        "second NO vote should succeed",
    );

    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect("should reach consensus at timeout");

    assert!(!result, "should return false (NO consensus)");

    let event_result = (|| {
        while let Ok((event_scope, event)) = events.recv_timeout(Duration::from_secs(1)) {
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
    })()
    .expect("consensus event should be emitted");

    assert!(!event_result, "event should indicate NO consensus");
}

#[test]
fn test_handle_consensus_timeout_resolves_with_liveness_yes() {
    let service = make_service();
    let events = service.event_bus().subscribe();
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
    );

    // Cast 1 YES vote (3 silent peers counted as YES at timeout → 4 YES total)
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    );

    // At timeout with liveness=true, silent peers count as YES → consensus reached
    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect("should reach consensus with silent peers as YES");

    assert!(result, "should return true (YES consensus)");

    // Verify ConsensusReached event was emitted
    let event_received = (|| {
        while let Ok((event_scope, event)) = events.recv_timeout(Duration::from_secs(1)) {
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
    })()
    .expect("ConsensusReached event should be emitted");

    assert!(event_received, "event should indicate YES consensus");

    // Verify get_consensus_result returns Ok(true)
    let consensus_result = service
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id)
        .expect("consensus result should be available");
    assert!(consensus_result, "consensus result should be true");
}

#[test]
fn test_handle_consensus_timeout_insufficient_votes() {
    let service = make_service();
    let events = service.event_bus().subscribe();
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
    );

    // Cast 2 YES votes (2 silent count as NO → 2 YES, 2 NO → tied → no consensus)
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    );

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    );

    // Call handle_consensus_timeout - should fail (tied votes, no majority)
    let err = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect_err("should fail with insufficient votes");

    assert!(
        matches!(err, ConsensusError::InsufficientVotesAtTimeout),
        "should return InsufficientVotesAtTimeout error"
    );

    // Verify ConsensusFailed event was emitted
    let event_received = (|| {
        while let Ok((event_scope, event)) = events.recv_timeout(Duration::from_secs(1)) {
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
    })();

    assert!(event_received, "ConsensusFailed event should be emitted");

    // Verify session is marked as Failed via get_consensus_result
    let result = service
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id);
    assert!(
        matches!(result, Err(ConsensusError::ConsensusFailed)),
        "session should be in Failed state"
    );

    {
        let active = service.storage().get_active_proposals(&scope).unwrap();
        assert!(
            active.is_empty(),
            "proposal should not be in active proposals"
        );
    }
}

#[test]
fn test_handle_consensus_timeout_no_votes_liveness_true() {
    let service = make_service();
    let events = service.event_bus().subscribe();
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
    );

    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect("should reach consensus with silent peers as YES");

    assert!(result, "should return true (all silent peers as YES)");

    // Verify ConsensusReached event was emitted
    let event_received = (|| {
        while let Ok((event_scope, event)) = events.recv_timeout(Duration::from_secs(1)) {
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
    })()
    .expect("ConsensusReached event should be emitted");

    assert!(event_received, "event should indicate YES consensus");
}

#[test]
fn test_handle_consensus_timeout_no_votes_liveness_false() {
    let service = make_service();
    let events = service.event_bus().subscribe();
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
    );

    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect("should reach NO consensus with silent peers as NO");

    assert!(!result, "should return false (all silent peers as NO)");

    // Verify ConsensusReached event was emitted
    let event_received = (|| {
        while let Ok((event_scope, event)) = events.recv_timeout(Duration::from_secs(1)) {
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
    })()
    .expect("ConsensusReached event should be emitted");

    assert!(!event_received, "event should indicate NO consensus");
}

#[test]
fn test_handle_consensus_timeout_reaches_consensus_p2p() {
    let service = make_service();
    let events = service.event_bus().subscribe();
    let scope = ScopeID::from("scope_p2p_reaches_consensus");
    let proposal_owner = PrivateKeySigner::random();

    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_3,
        true,
        ConsensusConfig::p2p(),
    );

    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    );

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    );

    let result = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect("should reach consensus");

    assert!(result, "should return true (YES consensus)");

    let event_received = (|| {
        while let Ok((event_scope, event)) = events.recv_timeout(Duration::from_secs(1)) {
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
    })()
    .expect("consensus event should be emitted");

    assert!(event_received, "event should indicate YES consensus");
}

#[test]
fn test_handle_consensus_timeout_insufficient_votes_p2p() {
    let service = make_service();
    let events = service.event_bus().subscribe();
    let scope = ScopeID::from("scope_p2p_insufficient_votes");
    let proposal_owner = PrivateKeySigner::random();

    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_4,
        false,
        ConsensusConfig::p2p(),
    );

    // Cast 2 YES votes (2 silent count as NO → 2 YES, 2 NO → tied → no consensus)
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        proposal_owner,
        "first vote",
    );

    let voter2 = PrivateKeySigner::random();
    cast_vote_or_panic(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        voter2,
        "second vote",
    );

    let err = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect_err("should fail with insufficient votes");

    assert!(
        matches!(err, ConsensusError::InsufficientVotesAtTimeout),
        "should return InsufficientVotesAtTimeout error"
    );

    let event_received = (|| {
        while let Ok((event_scope, event)) = events.recv_timeout(Duration::from_secs(1)) {
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
    })();

    assert!(event_received, "ConsensusFailed event should be emitted");
}

#[test]
fn test_cast_vote_rejects_same_voter_twice() {
    // Service-side dedup: cast_vote pre-checks the held signer's identity
    // against the session's votes map and returns UserAlreadyVoted.
    let proposal_owner = PrivateKeySigner::random();
    let signer = wrap(proposal_owner.clone());
    let service = DefaultConsensusService::new(signer.clone());
    let scope = ScopeID::from(SCOPE1_NAME);

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
        .expect("proposal should be created");

    service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES)
        .expect("first vote should succeed");

    let err = service
        .cast_vote(&scope, proposal.proposal_id, VOTE_YES)
        .expect_err("second vote from same voter should fail");

    assert!(
        matches!(err, ConsensusError::UserAlreadyVoted),
        "should return UserAlreadyVoted"
    );
}

#[test]
fn test_process_incoming_proposal_rejects_duplicate_proposal() {
    let service = make_service();
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
        .expect("proposal should be created");

    let err = service
        .process_incoming_proposal(&scope, proposal)
        .expect_err("duplicate proposal should be rejected");

    assert!(
        matches!(err, ConsensusError::ProposalAlreadyExist),
        "should return ProposalAlreadyExist"
    );
}

#[test]
fn test_process_incoming_vote_rejects_unknown_session() {
    let service = make_service();
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
        .expect("proposal should be created");

    let unknown_proposal_id = proposal.proposal_id.wrapping_add(1);
    let vote_owner = PrivateKeySigner::random();
    let vote = cast_remote_vote(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(vote_owner.clone()),
    )
    .expect("valid signed vote");

    let mut invalid_vote = vote;
    invalid_vote.proposal_id = unknown_proposal_id;
    invalid_vote.vote_hash = compute_vote_hash(&invalid_vote);
    invalid_vote.signature.clear();
    let vote_bytes = invalid_vote.encode_to_vec();
    invalid_vote.signature = vote_owner
        .sign_message_sync(&vote_bytes)
        .expect("vote should be resignable")
        .as_bytes()
        .to_vec();

    let err = service
        .process_incoming_vote(&scope, invalid_vote)
        .expect_err("vote for unknown proposal should fail");

    assert!(
        matches!(err, ConsensusError::SessionNotFound),
        "should return SessionNotFound"
    );
}

#[test]
fn test_handle_consensus_timeout_rejects_unknown_session() {
    let service = make_service();
    let scope = ScopeID::from(SCOPE1_NAME);

    let err = service
        .handle_consensus_timeout(&scope, u32::MAX)
        .expect_err("timeout handling for unknown proposal should fail");

    assert!(
        matches!(err, ConsensusError::SessionNotFound),
        "should return SessionNotFound"
    );
}

#[test]
fn test_process_incoming_proposal_rejects_expired_proposal() {
    let service = make_service();
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

    std::thread::sleep(Duration::from_secs(2));

    let err = service
        .process_incoming_proposal(&scope, proposal)
        .expect_err("expired incoming proposal should fail");

    assert!(
        matches!(err, ConsensusError::ProposalExpired),
        "should return ProposalExpired"
    );
}

#[test]
fn test_process_incoming_vote_rejects_invalid_vote_hash() {
    let service = make_service();
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
        .expect("proposal should be created");

    let proposal = cast_remote_vote_and_get_proposal(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("first vote should succeed");

    let voter = PrivateKeySigner::random();
    let mut vote =
        build_vote(&proposal, VOTE_YES, &wrap(voter)).expect("valid vote should be built");
    vote.vote_hash = vec![1; 32];

    let err = service
        .process_incoming_vote(&scope, vote)
        .expect_err("tampered vote hash should fail");

    assert!(
        matches!(err, ConsensusError::InvalidVoteHash),
        "should return InvalidVoteHash"
    );
}

#[test]
fn test_process_incoming_vote_rejects_invalid_vote_signature() {
    let service = make_service();
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
        .expect("proposal should be created");

    let proposal = cast_remote_vote_and_get_proposal(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner.clone()),
    )
    .expect("first vote should succeed");

    let voter = PrivateKeySigner::random();
    let mut vote =
        build_vote(&proposal, VOTE_YES, &wrap(voter)).expect("valid vote should be built");
    let wrong_signer = PrivateKeySigner::random();
    let vote_bytes = vote.encode_to_vec();
    let wrong_sig = wrong_signer
        .sign_message_sync(&vote_bytes)
        .expect("wrong signer should sign");
    vote.signature = wrong_sig.as_bytes().to_vec();

    let err = service
        .process_incoming_vote(&scope, vote)
        .expect_err("tampered signature should fail");

    assert!(
        matches!(err, ConsensusError::InvalidVoteSignature),
        "should return InvalidVoteSignature"
    );
}

#[test]
fn test_process_incoming_vote_rejects_duplicate_vote_owner() {
    let service = make_service();
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
        .expect("proposal should be created");

    let proposal = cast_remote_vote_and_get_proposal(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner.clone()),
    )
    .expect("first owner vote should succeed");

    let duplicate_vote = build_vote(&proposal, VOTE_YES, &wrap(proposal_owner))
        .expect("duplicate vote should be signable");

    let err = service
        .process_incoming_vote(&scope, duplicate_vote)
        .expect_err("duplicate vote owner should fail");

    assert!(
        matches!(err, ConsensusError::DuplicateVote),
        "should return DuplicateVote"
    );
}

#[test]
fn test_process_incoming_vote_rejects_expired_vote_timestamp() {
    let service = make_service();
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
        .expect("proposal should be created");

    let proposal = cast_remote_vote_and_get_proposal(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("first vote should succeed");

    let voter = PrivateKeySigner::random();
    let mut vote =
        build_vote(&proposal, VOTE_YES, &wrap(voter.clone())).expect("valid vote should be built");
    vote.timestamp = proposal.expiration_timestamp.saturating_add(1);
    vote.vote_hash = compute_vote_hash(&vote);
    vote.signature.clear();
    let vote_bytes = vote.encode_to_vec();
    vote.signature = voter
        .sign_message_sync(&vote_bytes)
        .expect("vote should be resignable")
        .as_bytes()
        .to_vec();

    let err = service
        .process_incoming_vote(&scope, vote)
        .expect_err("expired vote timestamp should fail");

    assert!(
        matches!(err, ConsensusError::VoteExpired),
        "should return VoteExpired"
    );
}

#[test]
fn test_handle_consensus_timeout_is_idempotent_for_failed_session() {
    let service = make_service();
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
        .expect("proposal should be created");

    cast_remote_vote(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("first vote should succeed");

    let voter2 = PrivateKeySigner::random();
    cast_remote_vote(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(voter2),
    )
    .expect("second vote should succeed");

    let err_first = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect_err("first timeout should fail consensus");
    assert!(matches!(
        err_first,
        ConsensusError::InsufficientVotesAtTimeout
    ));

    let err_second = service
        .handle_consensus_timeout(&scope, proposal.proposal_id)
        .expect_err("second timeout should keep failed consensus");
    assert!(matches!(
        err_second,
        ConsensusError::InsufficientVotesAtTimeout
    ));

    let result = service
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id);
    assert!(matches!(result, Err(ConsensusError::ConsensusFailed)));
}

#[test]
fn test_handle_consensus_timeout_rejects_unknown_scope() {
    let service = make_service();
    let unknown_scope = ScopeID::from("unknown_scope");

    let err = service
        .handle_consensus_timeout(&unknown_scope, 1)
        .expect_err("unknown scope timeout handling should fail");

    assert!(
        matches!(err, ConsensusError::SessionNotFound),
        "should return SessionNotFound for unknown scope"
    );
}

#[test]
fn test_cast_vote_still_active_does_not_emit_consensus_event() {
    let service = make_service();
    let events = service.event_bus().subscribe();
    let scope = ScopeID::from("still_active_no_event_scope");
    let proposal_owner = PrivateKeySigner::random();

    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_4,
        true,
        ConsensusConfig::gossipsub(),
    );

    // One vote is insufficient for n=4; transition should remain StillActive.
    cast_remote_vote(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("vote should succeed");

    let no_event = events.recv_timeout(Duration::from_millis(150));
    assert!(
        no_event.is_err(),
        "no terminal consensus event should be emitted while session is still active"
    );
}

#[test]
fn test_process_incoming_proposal_resolve_config_uses_base_timeout_when_expiration_not_after_timestamp()
 {
    let service = make_service();
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
        .expect("incoming proposal should be accepted");

    let resolved = service
        .storage()
        .get_proposal_config(&scope, incoming.proposal_id)
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

#[test]
fn test_get_reached_proposals_with_consensus() {
    let service = make_service();
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
        .expect("proposal should be created");

    // Cast vote to reach consensus
    cast_remote_vote(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("vote should succeed");

    // Get reached proposals
    let reached_map = service.storage().get_reached_proposals(&scope).unwrap();

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

#[test]
fn test_get_reached_proposals_no_consensus() {
    let service = make_service();
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
        .expect("proposal should be created");

    // Don't cast enough votes - proposal remains active

    // Get reached proposals
    let reached = service.storage().get_reached_proposals(&scope).unwrap();

    assert!(
        reached.is_empty(),
        "should return empty when no proposals have reached consensus"
    );
}

#[test]
fn test_get_reached_proposals_mixed_states() {
    let service = make_service();
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
        .expect("proposal should be created");

    cast_remote_vote(
        &service,
        &scope,
        proposal1.proposal_id,
        true,
        &wrap(proposal_owner1),
    )
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
        .expect("proposal should be created");

    cast_remote_vote(
        &service,
        &scope,
        proposal2.proposal_id,
        false,
        &wrap(proposal_owner2),
    )
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
        .expect("proposal should be created");

    // Don't cast enough votes for proposal3

    // Get reached proposals
    let reached_map = service.storage().get_reached_proposals(&scope).unwrap();

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

#[test]
fn test_get_reached_proposals_nonexistent_scope() {
    let service = make_service();
    let nonexistent_scope = ScopeID::from("nonexistent");

    // Get reached proposals for non-existent scope
    let reached = service
        .storage()
        .get_reached_proposals(&nonexistent_scope)
        .unwrap();

    assert!(
        reached.is_empty(),
        "should return empty map for non-existent scope"
    );
}

#[test]
fn test_unknown_scope_queries_stats_and_active_proposals() {
    let service = make_service();
    let unknown_scope = ScopeID::from("unknown_scope");

    let stats = service.get_scope_stats(&unknown_scope);
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

    let active = service
        .storage()
        .get_active_proposals(&unknown_scope)
        .unwrap();
    assert!(
        active.is_empty(),
        "get_active_proposals should return empty vec for unknown scope"
    );
}

#[test]
fn test_delete_scope_cleans_up_all_state() {
    let service = make_service();
    let scope = ScopeID::from("delete_scope_test");
    let proposal_owner = PrivateKeySigner::random();

    // 1. Create proposal and reach consensus
    let proposal = setup_proposal(
        &service,
        &scope,
        &proposal_owner,
        EXPECTED_VOTERS_COUNT_1,
        true,
        ConsensusConfig::gossipsub(),
    );

    cast_remote_vote(
        &service,
        &scope,
        proposal.proposal_id,
        VOTE_YES,
        &wrap(proposal_owner),
    )
    .expect("vote should succeed");

    // Verify state exists
    let consensus_result = service
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id);
    assert!(consensus_result.is_ok(), "consensus should be reached");

    {
        let reached = service.storage().get_reached_proposals(&scope).unwrap();
        assert!(
            !reached.is_empty(),
            "should have reached proposals before delete"
        );
    }

    // 2. Delete scope
    service
        .storage()
        .delete_scope(&scope)
        .expect("delete_scope should succeed");

    // 3. Verify all state is gone
    let active = service.storage().get_active_proposals(&scope).unwrap();
    assert!(
        active.is_empty(),
        "get_active_proposals should return empty vec after delete"
    );

    let reached = service.storage().get_reached_proposals(&scope).unwrap();
    assert!(
        reached.is_empty(),
        "get_reached_proposals should return empty map after delete"
    );

    let result = service
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id);
    assert!(
        matches!(result, Err(ConsensusError::SessionNotFound)),
        "get_consensus_result should return SessionNotFound after delete"
    );

    // 4. Verify scope can be reused (fresh start)
    let new_owner = PrivateKeySigner::random();
    let new_proposal = setup_proposal(
        &service,
        &scope,
        &new_owner,
        EXPECTED_VOTERS_COUNT_1,
        true,
        ConsensusConfig::gossipsub(),
    );

    cast_remote_vote(
        &service,
        &scope,
        new_proposal.proposal_id,
        VOTE_YES,
        &wrap(new_owner),
    )
    .expect("vote on new proposal should succeed");

    let new_result = service
        .storage()
        .get_consensus_result(&scope, new_proposal.proposal_id)
        .expect("new consensus should be reachable");
    assert!(new_result, "new proposal should reach YES consensus");
}

#[test]
fn test_delete_scope_on_unknown_scope_is_ok() {
    let service = make_service();
    let unknown_scope = ScopeID::from("never_existed");

    // Deleting a scope that was never created should not error
    let result = service.storage().delete_scope(&unknown_scope);
    assert!(
        result.is_ok(),
        "delete_scope on unknown scope should succeed"
    );
}
