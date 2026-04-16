use std::collections::HashMap;

use hashgraph_like_consensus::{
    protos::consensus::v1::Vote,
    utils::{calculate_consensus_result, has_sufficient_votes},
};

#[test]
fn test_two_thirds_threshold_rounding() {
    let threshold = 2.0 / 3.0;

    // 1 voter: needs 1
    assert!(has_sufficient_votes(1, 1, threshold));

    // 2 voters: need both
    assert!(!has_sufficient_votes(1, 2, threshold));
    assert!(has_sufficient_votes(2, 2, threshold));

    // 3 voters: ceil(2/3 * 3) = 2
    assert!(!has_sufficient_votes(1, 3, threshold));
    assert!(has_sufficient_votes(2, 3, threshold));

    // 4 voters: ceil(2/3 * 4) = 3
    assert!(!has_sufficient_votes(2, 4, threshold));
    assert!(has_sufficient_votes(3, 4, threshold));

    // 5 voters: ceil(2/3 * 5) = 4
    assert!(!has_sufficient_votes(3, 5, threshold));
    assert!(has_sufficient_votes(4, 5, threshold));

    // 6 voters: ceil(2/3 * 6) = 4
    assert!(!has_sufficient_votes(3, 6, threshold));
    assert!(has_sufficient_votes(4, 6, threshold));

    // 100 voters: ceil(2/3 * 100) = 67
    assert!(!has_sufficient_votes(66, 100, threshold));
    assert!(has_sufficient_votes(67, 100, threshold));
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
    assert_eq!(
        calculate_consensus_result(&votes, 3, 2.0 / 3.0, false, false),
        Some(true)
    );

    // Majority no
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], no_vote(2));
    votes.insert(vec![3], no_vote(3));
    assert_eq!(
        calculate_consensus_result(&votes, 3, 2.0 / 3.0, true, false),
        Some(false)
    );

    // Tie decided by liveness when threshold satisfied
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], no_vote(2));
    assert_eq!(
        calculate_consensus_result(&votes, 2, 2.0 / 3.0, true, false),
        Some(false)
    );
    assert_eq!(
        calculate_consensus_result(&votes, 2, 2.0 / 3.0, false, false),
        Some(false)
    );

    // Strict threshold requires more yes votes
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    votes.insert(vec![3], yes_vote(3));
    votes.insert(vec![4], no_vote(4));
    votes.insert(vec![5], no_vote(5));
    assert_eq!(
        calculate_consensus_result(&votes, 5, 0.9, true, false),
        None
    );

    // Fast threshold resolves early
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    votes.insert(vec![3], no_vote(3));
    assert_eq!(
        calculate_consensus_result(&votes, 5, 0.5, true, false),
        Some(true)
    );

    // ── Timeout path: n<=2 is unaffected by is_timeout ──

    // n=2 at timeout with only 1 vote — still None (n<=2 requires all votes)
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    assert_eq!(
        calculate_consensus_result(&votes, 2, 2.0 / 3.0, true, true),
        None
    );

    // ── Timeout path: silent peers count toward quorum (n>2) ──

    // 2 of 4 voted YES, 2 silent — normal path: None (quorum not met)
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    assert_eq!(
        calculate_consensus_result(&votes, 4, 2.0 / 3.0, true, false),
        None
    );

    // Same at timeout with liveness=true: silent as YES → 4 YES → Some(true)
    assert_eq!(
        calculate_consensus_result(&votes, 4, 2.0 / 3.0, true, true),
        Some(true)
    );

    // Timeout with liveness=false: silent as NO → 2 YES, 2 NO → no majority → None
    assert_eq!(
        calculate_consensus_result(&votes, 4, 2.0 / 3.0, false, true),
        None
    );

    // Timeout: 1 YES, 1 NO, 2 silent, liveness=true → 3 YES, 1 NO → Some(true)
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], no_vote(2));
    assert_eq!(
        calculate_consensus_result(&votes, 4, 2.0 / 3.0, true, true),
        Some(true)
    );

    // Timeout: 1 YES, 2 NO, 1 silent, liveness=true → 2 YES, 2 NO → tied → None
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], no_vote(2));
    votes.insert(vec![3], no_vote(3));
    assert_eq!(
        calculate_consensus_result(&votes, 4, 2.0 / 3.0, true, true),
        None
    );
}
