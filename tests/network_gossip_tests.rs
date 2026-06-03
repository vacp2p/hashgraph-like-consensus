use alloy::signers::local::PrivateKeySigner;
use std::thread;
use std::time::Duration;

use hashgraph_like_consensus::{
    error::ConsensusError, scope::ScopeID, service::DefaultConsensusService,
    session::ConsensusConfig, signing::EthereumConsensusSigner, storage::ConsensusStorage,
    types::CreateProposalRequest, utils::build_vote,
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

fn make_service() -> DefaultConsensusService {
    DefaultConsensusService::new(EthereumConsensusSigner::new(PrivateKeySigner::random()))
}

fn wrap(signer: PrivateKeySigner) -> EthereumConsensusSigner {
    EthereumConsensusSigner::new(signer)
}

const SCOPE: &str = "network_gossip_scope";
const PROPOSAL_NAME: &str = "Network Gossip Proposal";
const PROPOSAL_PAYLOAD: Vec<u8> = vec![];
const EXPIRATION: u64 = 120;

fn owner_bytes(signer: &PrivateKeySigner) -> Vec<u8> {
    signer.address().as_slice().to_vec()
}

/// Peer A creates a proposal, gossips it to peer B, both vote YES,
/// gossip votes back/forth, and both peers converge to Ok(true) consensus result.
#[test]
fn test_two_peers_gossip_reaches_unanimous_yes_for_n2() {
    let peer_a = make_service();
    let peer_b = make_service();
    let scope = ScopeID::from(SCOPE);

    let owner_a = PrivateKeySigner::random();
    let proposal = peer_a
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                owner_bytes(&owner_a),
                2, // n=2 => unanimous YES required
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .expect("peer_a proposal");

    // Gossip proposal to peer_b (decentralized: each peer stores locally).
    peer_b
        .process_incoming_proposal(&scope, proposal.clone())
        .expect("peer_b accepts proposal");

    // Peer A votes YES, gossip vote to peer B.
    let vote_a = cast_remote_vote(&peer_a, &scope, proposal.proposal_id, true, &wrap(owner_a))
        .expect("peer_a vote");
    peer_b
        .process_incoming_vote(&scope, vote_a)
        .expect("peer_b accepts peer_a vote");

    // Peer B votes YES, gossip vote to peer A.
    let owner_b = PrivateKeySigner::random();
    let vote_b = cast_remote_vote(&peer_b, &scope, proposal.proposal_id, true, &wrap(owner_b))
        .expect("peer_b vote");
    peer_a
        .process_incoming_vote(&scope, vote_b)
        .expect("peer_a accepts peer_b vote");

    // Both peers should converge to the same consensus result.
    let res_a = peer_a
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id)
        .expect("peer_a has consensus");
    let res_b = peer_b
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id)
        .expect("peer_b has consensus");

    assert!(res_a);
    assert!(res_b);
}

/// Three peers, and peer C receives votes in a different order,
/// but still converges to the same YES result.
#[test]
fn test_three_peers_gossip_converges_with_out_of_order_delivery() {
    let peer_a = make_service();
    let peer_b = make_service();
    let peer_c = make_service();
    let scope = ScopeID::from(format!("{SCOPE}_3p"));

    let owner_a = PrivateKeySigner::random();
    let proposal = peer_a
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                PROPOSAL_NAME.to_string(),
                PROPOSAL_PAYLOAD,
                owner_bytes(&owner_a),
                3,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .expect("peer_a proposal");

    // Gossip proposal to other peers
    peer_b
        .process_incoming_proposal(&scope, proposal.clone())
        .expect("peer_b accepts proposal");
    peer_c
        .process_incoming_proposal(&scope, proposal.clone())
        .expect("peer_c accepts proposal");

    // Two YES votes are sufficient for n=3 with threshold 2/3 and majority YES.
    let vote_a = cast_remote_vote(&peer_a, &scope, proposal.proposal_id, true, &wrap(owner_a))
        .expect("peer_a vote");
    let owner_b = PrivateKeySigner::random();
    let vote_b = cast_remote_vote(&peer_b, &scope, proposal.proposal_id, true, &wrap(owner_b))
        .expect("peer_b vote");

    // Deliver to peer_c out-of-order (vote_b then vote_a).
    peer_c
        .process_incoming_vote(&scope, vote_b.clone())
        .expect("peer_c accepts vote_b");
    peer_c
        .process_incoming_vote(&scope, vote_a.clone())
        .expect("peer_c accepts vote_a");

    // Deliver to peer_a and peer_b (in-order doesn't matter either).
    peer_a
        .process_incoming_vote(&scope, vote_b)
        .expect("peer_a accepts vote_b");
    peer_b
        .process_incoming_vote(&scope, vote_a)
        .expect("peer_b accepts vote_a");

    let res_a = peer_a
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id)
        .expect("peer_a has consensus");
    let res_b = peer_b
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id)
        .expect("peer_b has consensus");
    let res_c = peer_c
        .storage()
        .get_consensus_result(&scope, proposal.proposal_id)
        .expect("peer_c has consensus");

    assert!(res_a);
    assert!(res_b);
    assert!(res_c);
}

/// Multiple peers each schedule their own timeout finalization task.
/// All peers should converge to the same "failed" state.
/// With liveness=false and 2 YES votes out of 4, silent peers count as NO at timeout,
/// resulting in a 2 YES / 2 NO tie → no consensus.
#[test]
fn test_multi_peer_timeout_task_converges_to_failed() {
    let peer_a = make_service();
    let peer_b = make_service();
    let peer_c = make_service();
    let scope = ScopeID::from(format!("{SCOPE}_timeout"));

    // n=4, liveness=false, 2 YES votes → 2 YES + 2 silent(NO) = tied → fail.
    let owner_a = PrivateKeySigner::random();
    let voter_b = PrivateKeySigner::random();
    let proposal = peer_a
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                "Timeout Proposal".to_string(),
                PROPOSAL_PAYLOAD,
                owner_bytes(&owner_a),
                4,
                EXPIRATION,
                false,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .expect("peer_a proposal");

    peer_b
        .process_incoming_proposal(&scope, proposal.clone())
        .expect("peer_b accepts proposal");
    peer_c
        .process_incoming_proposal(&scope, proposal.clone())
        .expect("peer_c accepts proposal");

    // 2 YES votes total.
    let vote_a = cast_remote_vote(&peer_a, &scope, proposal.proposal_id, true, &wrap(owner_a))
        .expect("peer_a vote");
    peer_b
        .process_incoming_vote(&scope, vote_a.clone())
        .expect("peer_b accepts vote_a");
    peer_c
        .process_incoming_vote(&scope, vote_a)
        .expect("peer_c accepts vote_a");

    let vote_b = cast_remote_vote(&peer_b, &scope, proposal.proposal_id, true, &wrap(voter_b))
        .expect("peer_b vote");
    peer_a
        .process_incoming_vote(&scope, vote_b.clone())
        .expect("peer_a accepts vote_b");
    peer_c
        .process_incoming_vote(&scope, vote_b)
        .expect("peer_c accepts vote_b");

    // App-style scheduling: each peer runs its own timeout task.
    let proposal_id = proposal.proposal_id;
    let scope_a = scope.clone();
    let scope_b = scope.clone();
    let scope_c = scope.clone();
    let peer_a_task = peer_a.clone();
    let peer_b_task = peer_b.clone();
    let peer_c_task = peer_c.clone();
    let ha = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        let _ = peer_a_task.handle_consensus_timeout(&scope_a, proposal_id);
    });
    let hb = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        let _ = peer_b_task.handle_consensus_timeout(&scope_b, proposal_id);
    });
    let hc = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        let _ = peer_c_task.handle_consensus_timeout(&scope_c, proposal_id);
    });

    ha.join().expect("timeout task A");
    hb.join().expect("timeout task B");
    hc.join().expect("timeout task C");

    // Converge to "failed" state (no consensus result).
    let result_a = peer_a.storage().get_consensus_result(&scope, proposal_id);
    assert!(
        matches!(result_a, Err(ConsensusError::ConsensusFailed)),
        "peer_a should be in Failed state"
    );

    let result_b = peer_b.storage().get_consensus_result(&scope, proposal_id);
    assert!(
        matches!(result_b, Err(ConsensusError::ConsensusFailed)),
        "peer_b should be in Failed state"
    );

    let result_c = peer_c.storage().get_consensus_result(&scope, proposal_id);
    assert!(
        matches!(result_c, Err(ConsensusError::ConsensusFailed)),
        "peer_c should be in Failed state"
    );
}

/// Four peers, check that the proposal converges to YES by liveness criteria.
/// Result depends on the liveness criteria as we have 2 YES and 2 NO votes.
#[test]
fn test_multi_peer_timeout_task_resolves_tie_by_liveness_criteria_yes() {
    let peer_a = make_service();
    let peer_b = make_service();
    let peer_c = make_service();
    let peer_d = make_service();
    let scope = ScopeID::from(format!("{SCOPE}_timeout_tie"));

    // n=4, votes: 2 YES / 2 NO => tie. With liveness_criteria_yes=true, resolve to YES.
    let owner_a = PrivateKeySigner::random();
    let proposal = peer_a
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                "Timeout Tie Proposal".to_string(),
                PROPOSAL_PAYLOAD,
                owner_bytes(&owner_a),
                4,
                EXPIRATION,
                true, // liveness_criteria_yes
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .expect("peer_a proposal");

    for peer in [&peer_b, &peer_c, &peer_d] {
        peer.process_incoming_proposal(&scope, proposal.clone())
            .expect("peer accepts proposal");
    }

    // Cast votes sequentially, gossiping each vote to all peers before the next voter votes,
    // so that received_hash references match on every peer.
    let vote_a = cast_remote_vote(&peer_a, &scope, proposal.proposal_id, true, &wrap(owner_a))
        .expect("vote_a");
    for peer in [&peer_b, &peer_c, &peer_d] {
        peer.process_incoming_vote(&scope, vote_a.clone())
            .expect("peer accepts vote_a");
    }

    let voter_b = PrivateKeySigner::random();
    let vote_b = cast_remote_vote(&peer_b, &scope, proposal.proposal_id, true, &wrap(voter_b))
        .expect("vote_b");
    for peer in [&peer_a, &peer_c, &peer_d] {
        peer.process_incoming_vote(&scope, vote_b.clone())
            .expect("peer accepts vote_b");
    }

    let voter_c = PrivateKeySigner::random();
    let vote_c = cast_remote_vote(&peer_c, &scope, proposal.proposal_id, false, &wrap(voter_c))
        .expect("vote_c");
    for peer in [&peer_a, &peer_b, &peer_d] {
        peer.process_incoming_vote(&scope, vote_c.clone())
            .expect("peer accepts vote_c");
    }

    let voter_d = PrivateKeySigner::random();
    let vote_d = cast_remote_vote(&peer_d, &scope, proposal.proposal_id, false, &wrap(voter_d))
        .expect("vote_d");
    for peer in [&peer_a, &peer_b, &peer_c] {
        peer.process_incoming_vote(&scope, vote_d.clone())
            .expect("peer accepts vote_d");
    }

    // App-style scheduling: each peer runs a timeout finalization task.
    let proposal_id = proposal.proposal_id;
    let peer_a_task = peer_a.clone();
    let peer_b_task = peer_b.clone();
    let peer_c_task = peer_c.clone();
    let peer_d_task = peer_d.clone();
    let scope_a = scope.clone();
    let scope_b = scope.clone();
    let scope_c = scope.clone();
    let scope_d = scope.clone();

    let ha = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        let _ = peer_a_task.handle_consensus_timeout(&scope_a, proposal_id);
    });
    let hb = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        let _ = peer_b_task.handle_consensus_timeout(&scope_b, proposal_id);
    });
    let hc = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        let _ = peer_c_task.handle_consensus_timeout(&scope_c, proposal_id);
    });
    let hd = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        let _ = peer_d_task.handle_consensus_timeout(&scope_d, proposal_id);
    });

    ha.join().expect("timeout task A");
    hb.join().expect("timeout task B");
    hc.join().expect("timeout task C");
    hd.join().expect("timeout task D");

    let res_a = peer_a
        .storage()
        .get_consensus_result(&scope, proposal_id)
        .expect("peer_a has consensus");
    let res_b = peer_b
        .storage()
        .get_consensus_result(&scope, proposal_id)
        .expect("peer_b has consensus");
    let res_c = peer_c
        .storage()
        .get_consensus_result(&scope, proposal_id)
        .expect("peer_c has consensus");
    let res_d = peer_d
        .storage()
        .get_consensus_result(&scope, proposal_id)
        .expect("peer_d has consensus");

    assert!(res_a);
    assert!(res_b);
    assert!(res_c);
    assert!(res_d);
}
