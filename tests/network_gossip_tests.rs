use alloy::signers::local::PrivateKeySigner;
use tokio::time::{Duration, sleep};

use hashgraph_like_consensus::{
    error::ConsensusError, scope::ScopeID, service::DefaultConsensusService,
    session::ConsensusConfig, types::CreateProposalRequest,
};

const SCOPE: &str = "network_gossip_scope";
const PROPOSAL_NAME: &str = "Network Gossip Proposal";
const PROPOSAL_PAYLOAD: Vec<u8> = vec![];
const EXPIRATION: u64 = 120;

fn owner_bytes(signer: &PrivateKeySigner) -> Vec<u8> {
    signer.address().as_slice().to_vec()
}

/// Peer A creates a proposal, gossips it to peer B, both vote YES,
/// gossip votes back/forth, and both peers converge to Ok(true) consensus result.
#[tokio::test]
async fn test_two_peers_gossip_reaches_unanimous_yes_for_n2() {
    let peer_a = DefaultConsensusService::default();
    let peer_b = DefaultConsensusService::default();
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
        .await
        .expect("peer_a proposal");

    // Gossip proposal to peer_b (decentralized: each peer stores locally).
    peer_b
        .process_incoming_proposal(&scope, proposal.clone())
        .await
        .expect("peer_b accepts proposal");

    // Peer A votes YES, gossip vote to peer B.
    let vote_a = peer_a
        .cast_vote(&scope, proposal.proposal_id, true, owner_a)
        .await
        .expect("peer_a vote");
    peer_b
        .process_incoming_vote(&scope, vote_a)
        .await
        .expect("peer_b accepts peer_a vote");

    // Peer B votes YES, gossip vote to peer A.
    let owner_b = PrivateKeySigner::random();
    let vote_b = peer_b
        .cast_vote(&scope, proposal.proposal_id, true, owner_b)
        .await
        .expect("peer_b vote");
    peer_a
        .process_incoming_vote(&scope, vote_b)
        .await
        .expect("peer_a accepts peer_b vote");

    // Both peers should converge to the same consensus result.
    let res_a = peer_a
        .get_consensus_result(&scope, proposal.proposal_id)
        .await
        .expect("peer_a has consensus");
    let res_b = peer_b
        .get_consensus_result(&scope, proposal.proposal_id)
        .await
        .expect("peer_b has consensus");

    assert!(res_a);
    assert!(res_b);
}

/// Three peers, and peer C receives votes in a different order,
/// but still converges to the same YES result.
#[tokio::test]
async fn test_three_peers_gossip_converges_with_out_of_order_delivery() {
    let peer_a = DefaultConsensusService::default();
    let peer_b = DefaultConsensusService::default();
    let peer_c = DefaultConsensusService::default();
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
        .await
        .expect("peer_a proposal");

    // Gossip proposal to other peers
    peer_b
        .process_incoming_proposal(&scope, proposal.clone())
        .await
        .expect("peer_b accepts proposal");
    peer_c
        .process_incoming_proposal(&scope, proposal.clone())
        .await
        .expect("peer_c accepts proposal");

    // Two YES votes are sufficient for n=3 with threshold 2/3 and majority YES.
    let vote_a = peer_a
        .cast_vote(&scope, proposal.proposal_id, true, owner_a)
        .await
        .expect("peer_a vote");
    let owner_b = PrivateKeySigner::random();
    let vote_b = peer_b
        .cast_vote(&scope, proposal.proposal_id, true, owner_b)
        .await
        .expect("peer_b vote");

    // Deliver to peer_c out-of-order (vote_b then vote_a).
    peer_c
        .process_incoming_vote(&scope, vote_b.clone())
        .await
        .expect("peer_c accepts vote_b");
    peer_c
        .process_incoming_vote(&scope, vote_a.clone())
        .await
        .expect("peer_c accepts vote_a");

    // Deliver to peer_a and peer_b (in-order doesn't matter either).
    peer_a
        .process_incoming_vote(&scope, vote_b)
        .await
        .expect("peer_a accepts vote_b");
    peer_b
        .process_incoming_vote(&scope, vote_a)
        .await
        .expect("peer_b accepts vote_a");

    let res_a = peer_a
        .get_consensus_result(&scope, proposal.proposal_id)
        .await
        .expect("peer_a has consensus");
    let res_b = peer_b
        .get_consensus_result(&scope, proposal.proposal_id)
        .await
        .expect("peer_b has consensus");
    let res_c = peer_c
        .get_consensus_result(&scope, proposal.proposal_id)
        .await
        .expect("peer_c has consensus");

    assert!(res_a);
    assert!(res_b);
    assert!(res_c);
}

/// Multiple peers each schedule their own timeout finalization task.
/// All peers should converge to the same "failed" state.
#[tokio::test]
async fn test_multi_peer_timeout_task_converges_to_failed() {
    let peer_a = DefaultConsensusService::default();
    let peer_b = DefaultConsensusService::default();
    let peer_c = DefaultConsensusService::default();
    let scope = ScopeID::from(format!("{SCOPE}_timeout"));

    // n=4, but we will only submit 1 vote -> should fail on timeout finalization.
    let owner_a = PrivateKeySigner::random();
    let proposal = peer_a
        .create_proposal_with_config(
            &scope,
            CreateProposalRequest::new(
                "Timeout Proposal".to_string(),
                PROPOSAL_PAYLOAD,
                owner_bytes(&owner_a),
                4,
                EXPIRATION,
                true,
            )
            .expect("valid proposal request"),
            Some(ConsensusConfig::gossipsub()),
        )
        .await
        .expect("peer_a proposal");

    peer_b
        .process_incoming_proposal(&scope, proposal.clone())
        .await
        .expect("peer_b accepts proposal");
    peer_c
        .process_incoming_proposal(&scope, proposal.clone())
        .await
        .expect("peer_c accepts proposal");

    // Only 1 vote total.
    let vote_a = peer_a
        .cast_vote(&scope, proposal.proposal_id, true, owner_a)
        .await
        .expect("peer_a vote");
    peer_b
        .process_incoming_vote(&scope, vote_a.clone())
        .await
        .expect("peer_b accepts vote");
    peer_c
        .process_incoming_vote(&scope, vote_a)
        .await
        .expect("peer_c accepts vote");

    // App-style scheduling: each peer runs its own timeout task.
    let proposal_id = proposal.proposal_id;
    let scope_a = scope.clone();
    let scope_b = scope.clone();
    let scope_c = scope.clone();
    let peer_a_task = peer_a.clone();
    let peer_b_task = peer_b.clone();
    let peer_c_task = peer_c.clone();
    let ha = tokio::spawn(async move {
        sleep(Duration::from_millis(25)).await;
        let _ = peer_a_task
            .handle_consensus_timeout(&scope_a, proposal_id)
            .await;
    });
    let hb = tokio::spawn(async move {
        sleep(Duration::from_millis(25)).await;
        let _ = peer_b_task
            .handle_consensus_timeout(&scope_b, proposal_id)
            .await;
    });
    let hc = tokio::spawn(async move {
        sleep(Duration::from_millis(25)).await;
        let _ = peer_c_task
            .handle_consensus_timeout(&scope_c, proposal_id)
            .await;
    });

    ha.await.expect("timeout task A");
    hb.await.expect("timeout task B");
    hc.await.expect("timeout task C");

    // Converge to "failed" state (no consensus result).
    assert!(matches!(
        peer_a.get_consensus_result(&scope, proposal_id).await,
        Err(ConsensusError::ConsensusFailed)
    ));
    assert!(matches!(
        peer_b.get_consensus_result(&scope, proposal_id).await,
        Err(ConsensusError::ConsensusFailed)
    ));
    assert!(matches!(
        peer_c.get_consensus_result(&scope, proposal_id).await,
        Err(ConsensusError::ConsensusFailed)
    ));
}

/// Four peers, check that the proposal converges to YES by liveness criteria.
/// Result depends on the liveness criteria as we have 2 YES and 2 NO votes.
#[tokio::test]
async fn test_multi_peer_timeout_task_resolves_tie_by_liveness_criteria_yes() {
    let peer_a = DefaultConsensusService::default();
    let peer_b = DefaultConsensusService::default();
    let peer_c = DefaultConsensusService::default();
    let peer_d = DefaultConsensusService::default();
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
        .await
        .expect("peer_a proposal");

    for peer in [&peer_b, &peer_c, &peer_d] {
        peer.process_incoming_proposal(&scope, proposal.clone())
            .await
            .expect("peer accepts proposal");
    }

    // Cast votes sequentially, gossiping each vote to all peers before the next voter votes,
    // so that received_hash references match on every peer.
    let vote_a = peer_a
        .cast_vote(&scope, proposal.proposal_id, true, owner_a)
        .await
        .expect("vote_a");
    for peer in [&peer_b, &peer_c, &peer_d] {
        peer.process_incoming_vote(&scope, vote_a.clone())
            .await
            .expect("peer accepts vote_a");
    }

    let voter_b = PrivateKeySigner::random();
    let vote_b = peer_b
        .cast_vote(&scope, proposal.proposal_id, true, voter_b)
        .await
        .expect("vote_b");
    for peer in [&peer_a, &peer_c, &peer_d] {
        peer.process_incoming_vote(&scope, vote_b.clone())
            .await
            .expect("peer accepts vote_b");
    }

    let voter_c = PrivateKeySigner::random();
    let vote_c = peer_c
        .cast_vote(&scope, proposal.proposal_id, false, voter_c)
        .await
        .expect("vote_c");
    for peer in [&peer_a, &peer_b, &peer_d] {
        peer.process_incoming_vote(&scope, vote_c.clone())
            .await
            .expect("peer accepts vote_c");
    }

    let voter_d = PrivateKeySigner::random();
    let vote_d = peer_d
        .cast_vote(&scope, proposal.proposal_id, false, voter_d)
        .await
        .expect("vote_d");
    for peer in [&peer_a, &peer_b, &peer_c] {
        peer.process_incoming_vote(&scope, vote_d.clone())
            .await
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

    let ha = tokio::spawn(async move {
        sleep(Duration::from_millis(10)).await;
        let _ = peer_a_task
            .handle_consensus_timeout(&scope_a, proposal_id)
            .await;
    });
    let hb = tokio::spawn(async move {
        sleep(Duration::from_millis(10)).await;
        let _ = peer_b_task
            .handle_consensus_timeout(&scope_b, proposal_id)
            .await;
    });
    let hc = tokio::spawn(async move {
        sleep(Duration::from_millis(10)).await;
        let _ = peer_c_task
            .handle_consensus_timeout(&scope_c, proposal_id)
            .await;
    });
    let hd = tokio::spawn(async move {
        sleep(Duration::from_millis(10)).await;
        let _ = peer_d_task
            .handle_consensus_timeout(&scope_d, proposal_id)
            .await;
    });

    ha.await.expect("timeout task A");
    hb.await.expect("timeout task B");
    hc.await.expect("timeout task C");
    hd.await.expect("timeout task D");

    let res_a = peer_a
        .get_consensus_result(&scope, proposal_id)
        .await
        .expect("peer_a has consensus");
    let res_b = peer_b
        .get_consensus_result(&scope, proposal_id)
        .await
        .expect("peer_b has consensus");
    let res_c = peer_c
        .get_consensus_result(&scope, proposal_id)
        .await
        .expect("peer_c has consensus");
    let res_d = peer_d
        .get_consensus_result(&scope, proposal_id)
        .await
        .expect("peer_d has consensus");

    assert!(res_a);
    assert!(res_b);
    assert!(res_c);
    assert!(res_d);
}
