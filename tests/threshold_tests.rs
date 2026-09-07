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

    // Strict threshold requires more yes votes (3 of 5 < ceil(5 * 0.9) = 5)
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

    // Low threshold (0.5, margin = 3 of 5): 2 YES is below the margin, and the two
    // outstanding peers are no longer counted as YES before the timeout — must wait.
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    votes.insert(vec![3], no_vote(3));
    assert_eq!(
        calculate_consensus_result(&votes, 5, 0.5, true, false),
        None
    );

    // Threshold at or below 1/2 (0.4, margin = 2 of 5): 2 YES meet the margin but a
    // 2-vote lead with 3 outstanding is flippable (another peer could see 2 NO first),
    // so the unflippable-lead guard keeps it undecided.
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    assert_eq!(
        calculate_consensus_result(&votes, 5, 0.4, true, false),
        None
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

    // ── Pre-timeout rule: silent peers are not counted; a side wins only with the
    // ceil(2n/3) margin AND a lead the outstanding votes cannot overturn. ──

    // n=3, YES+NO, 1 outstanding: 1-vote lead is flippable — must wait.
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], no_vote(2));
    assert_eq!(
        calculate_consensus_result(&votes, 3, 2.0 / 3.0, true, false),
        None
    );

    // n=3, YES+NO+NO, fully voted: NO's lead (2 vs 1) is unflippable.
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], no_vote(2));
    votes.insert(vec![3], no_vote(3));
    assert_eq!(
        calculate_consensus_result(&votes, 3, 2.0 / 3.0, true, false),
        Some(false)
    );

    // n=4, YES+YES+NO, 1 outstanding: YES leads by 1 but that is exactly the
    // number of outstanding votes — NO could still tie it, so still None.
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    votes.insert(vec![3], no_vote(3));
    assert_eq!(
        calculate_consensus_result(&votes, 4, 2.0 / 3.0, true, false),
        None
    );

    // n=7, 3 YES + 2 NO, 2 outstanding: a 1-vote lead with 2 outstanding could
    // still tie (both remaining go NO) — must wait.
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    votes.insert(vec![3], yes_vote(3));
    votes.insert(vec![4], no_vote(4));
    votes.insert(vec![5], no_vote(5));
    assert_eq!(
        calculate_consensus_result(&votes, 7, 2.0 / 3.0, true, false),
        None
    );

    // n=7, 4 YES + 2 NO, 1 outstanding: the lead is unflippable but 4 is below the
    // ceil(2n/3) = 5 winning margin — still waits (the margin is kept from 0.6.0).
    votes.insert(vec![6], yes_vote(6));
    assert_eq!(
        calculate_consensus_result(&votes, 7, 2.0 / 3.0, true, false),
        None
    );

    // n=7, 5 YES + 2 NO, fully voted: margin met and unflippable — YES.
    votes.insert(vec![7], yes_vote(7));
    assert_eq!(
        calculate_consensus_result(&votes, 7, 2.0 / 3.0, true, false),
        Some(true)
    );

    // n=5, 3 YES + 2 NO, fully voted: a majority below ceil(2n/3) = 4 does not
    // resolve, before or at the timeout (unchanged from 0.6.0; the application may
    // retry with a new proposal).
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    votes.insert(vec![3], yes_vote(3));
    votes.insert(vec![4], no_vote(4));
    votes.insert(vec![5], no_vote(5));
    assert_eq!(
        calculate_consensus_result(&votes, 5, 2.0 / 3.0, true, false),
        None
    );
    assert_eq!(
        calculate_consensus_result(&votes, 5, 2.0 / 3.0, true, true),
        None
    );

    // Timeout keeps the margin too: n=7, 4 YES, 3 silent counted as NO → 4 < 5 → None.
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    votes.insert(vec![3], yes_vote(3));
    votes.insert(vec![4], yes_vote(4));
    assert_eq!(
        calculate_consensus_result(&votes, 7, 2.0 / 3.0, false, true),
        None
    );
    // Same votes with silent peers counted as YES → 7 YES → Some(true).
    assert_eq!(
        calculate_consensus_result(&votes, 7, 2.0 / 3.0, true, true),
        Some(true)
    );

    // Pre-timeout ignores liveness_criteria_yes entirely: n=3, YES + NO stays
    // undecided for both settings.
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], no_vote(2));
    assert_eq!(
        calculate_consensus_result(&votes, 3, 2.0 / 3.0, false, false),
        None
    );

    // Tie consistency: a fully-voted exact tie resolves via liveness_criteria_yes
    // identically before and at the timeout (0.6.0 behaviour, kept deliberately).
    votes.clear();
    votes.insert(vec![1], yes_vote(1));
    votes.insert(vec![2], yes_vote(2));
    votes.insert(vec![3], no_vote(3));
    votes.insert(vec![4], no_vote(4));
    assert_eq!(
        calculate_consensus_result(&votes, 4, 2.0 / 3.0, true, true),
        Some(true)
    );
    assert_eq!(
        calculate_consensus_result(&votes, 4, 2.0 / 3.0, false, true),
        Some(false)
    );
}
