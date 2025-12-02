use std::collections::HashMap;

use hashgraph_like_consensus::{
    protos::consensus::v1::Vote,
    utils::{calculate_consensus_result, check_sufficient_votes},
};

#[test]
fn test_two_thirds_threshold_rounding() {
    let threshold = 2.0 / 3.0;

    // 1 voter: needs 1
    assert!(check_sufficient_votes(1, 1, threshold));

    // 2 voters: need both
    assert!(!check_sufficient_votes(1, 2, threshold));
    assert!(check_sufficient_votes(2, 2, threshold));

    // 3 voters: ceil(2/3 * 3) = 2
    assert!(!check_sufficient_votes(1, 3, threshold));
    assert!(check_sufficient_votes(2, 3, threshold));

    // 4 voters: ceil(2/3 * 4) = 3
    assert!(!check_sufficient_votes(2, 4, threshold));
    assert!(check_sufficient_votes(3, 4, threshold));

    // 5 voters: ceil(2/3 * 5) = 4
    assert!(!check_sufficient_votes(3, 5, threshold));
    assert!(check_sufficient_votes(4, 5, threshold));

    // 6 voters: ceil(2/3 * 6) = 4
    assert!(!check_sufficient_votes(3, 6, threshold));
    assert!(check_sufficient_votes(4, 6, threshold));

    // 100 voters: ceil(2/3 * 100) = 67
    assert!(!check_sufficient_votes(66, 100, threshold));
    assert!(check_sufficient_votes(67, 100, threshold));
}

#[test]
fn test_calculate_consensus_result_variants() {
    let yes_vote = |id: u32| Vote {
        vote_id: id,
        vote_owner: vec![id as u8],
        proposal_id: 1,
        timestamp: 0,
        vote: true,
        parent_hash: vec![],
        received_hash: vec![],
        vote_hash: vec![id as u8],
        signature: vec![],
    };
    let no_vote = |id: u32| Vote {
        vote: false,
        vote_hash: vec![id as u8],
        ..yes_vote(id)
    };

    // Majority yes
    let mut votes: HashMap<Vec<u8>, Vote> = HashMap::new();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    votes.insert(vec![3], no_vote(3));
    assert!(calculate_consensus_result(&votes, false));

    // Majority no
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], no_vote(2));
    votes.insert(vec![3], no_vote(3));
    assert!(!calculate_consensus_result(&votes, true));

    // Tie decided by liveness
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], no_vote(2));
    assert!(calculate_consensus_result(&votes, true));
    assert!(!calculate_consensus_result(&votes, false));
}
