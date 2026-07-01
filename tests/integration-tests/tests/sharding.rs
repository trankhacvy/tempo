//! Stage A sharding (plan §2): the OrderSlab is split into `num_slab_shards`
//! independent shards that fold into the single histogram in parallel. These tests
//! prove the adversarial invariants survive sharding, and pin the fixes from the
//! Stage-A code review:
//!   * cross-shard fold — orders in different shards fold into the same histogram
//!     (commutative addition);
//!   * completeness — `finalize_clear` refuses while any shard STILL HOLDS an unfolded
//!     order, but empty shards never block (the `shards_pending` aggregate counts only
//!     shards with unfolded orders — review bug 2);
//!   * a submit+cancel can never wedge the market (cancel decrements `resting_count` —
//!     review bug 1);
//!   * a multi-shard `force_reset` clears EVERY shard and bumps the auction id exactly
//!     once, so the next roll still succeeds (review bugs 3 & 4).

use tempo_integration_tests::*;

/// Orders submitted to different shards each land in their own slab account and fold
/// into the one shared histogram — `accumulated_count` sums across shards.
#[test]
fn orders_route_to_distinct_shards_and_fold() {
    let mut ctx = TestContext::new();
    ctx.market_num_slab_shards = 2;
    let pdas = ctx.init_market(10, 64, 16);
    assert_eq!(pdas.num_slab_shards, 2);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    // Taker buys (no maker sells → no cross), one into each shard.
    let _ = ctx.submit_order_to_shard(&pdas, &a, SIDE_BUY, 40, 5, 0);
    let _ = ctx.submit_order_to_shard(&pdas, &b, SIDE_BUY, 40, 7, 1);

    // Each order rests in its own shard account.
    assert_eq!(ctx.order_slab_shard(&pdas, 0).unwrap().count, 1, "shard 0");
    assert_eq!(ctx.order_slab_shard(&pdas, 1).unwrap().count, 1, "shard 1");

    // Fold both shards into the single histogram.
    ctx.process_chunk_shard(&pdas, 0, 0, 16);
    ctx.process_chunk_shard(&pdas, 1, 0, 16);

    // Commutative fold: the one histogram counts both shards' orders.
    assert_eq!(
        ctx.histogram(&pdas).accumulated_count,
        2,
        "both shards fold into the single histogram"
    );

    // Every shard with orders folded ⇒ completeness satisfied ⇒ finalize is accepted.
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "finalize succeeds once all non-empty shards are folded"
    );
}

/// Review bug 2: empty shards must NOT impose a per-shard crank obligation. A market with
/// orders only in shard 0 clears after folding ONLY shard 0 — the empty shards 1 & 2 are
/// never counted in `shards_pending`, so `finalize_clear` is accepted without cranking them.
#[test]
fn empty_shards_do_not_block_finalize() {
    let mut ctx = TestContext::new();
    ctx.market_num_slab_shards = 3;
    let pdas = ctx.init_market(10, 64, 16);

    // Orders only in shard 0; shards 1 and 2 stay empty.
    let t = ctx.new_funded_signer();
    ctx.submit_order_to_shard(&pdas, &t, SIDE_BUY, 40, 5, 0);

    // Fold shard 0 (the only shard with orders). The empty shards were never counted, so
    // completeness is already satisfied — no need to crank shards 1 & 2.
    ctx.process_chunk_shard(&pdas, 0, 0, 16);
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "finalize succeeds after folding only the non-empty shard (empties don't block)",
    );
}

/// The censorship guarantee still holds: a NON-EMPTY shard left unfolded blocks finalize.
/// Orders in both shards; folding only shard 0 must be refused until shard 1 is folded too.
#[test]
fn nonempty_unfolded_shard_still_blocks_finalize() {
    let mut ctx = TestContext::new();
    ctx.market_num_slab_shards = 2;
    let pdas = ctx.init_market(10, 64, 16);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    ctx.submit_order_to_shard(&pdas, &a, SIDE_BUY, 40, 5, 0);
    ctx.submit_order_to_shard(&pdas, &b, SIDE_BUY, 40, 6, 1);

    // Fold shard 0 only → shard 1 still holds an unfolded order → finalize refused.
    ctx.process_chunk_shard(&pdas, 0, 0, 16);
    assert!(
        ctx.try_finalize_clear(&pdas).is_err(),
        "finalize must refuse while shard 1 holds an unfolded order",
    );

    // Fold shard 1 → completeness satisfied → finalize accepted.
    ctx.process_chunk_shard(&pdas, 1, 0, 16);
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "finalize succeeds once every non-empty shard is folded",
    );
}

/// Review bug 1: a submit followed by a cancel must not leave a phantom `resting_count`
/// that wedges clearing forever. After a submit+cancel the shard is empty again; a fresh
/// order then folds and clears normally.
#[test]
fn submit_then_cancel_does_not_wedge_clearing() {
    let mut ctx = TestContext::new();
    // Single shard (default) so the wedge, if present, cannot be masked by other shards.
    let pdas = ctx.init_market(10, 64, 16);
    let t = ctx.new_funded_signer();

    // Submit then cancel — the cancelled order must drop out of the completeness aggregate.
    let a = ctx.submit_order(&pdas, &t, SIDE_BUY, 40, 5);
    ctx.cancel_order(&pdas, &t, a);
    assert_eq!(
        ctx.order_slab_shard(&pdas, 0).unwrap().count,
        0,
        "shard empty after cancel"
    );

    // A fresh order in the same round: it is the ONLY unfolded order. With the bug, the
    // cancelled order's stale count would keep `shards_pending > 0` and finalize would
    // revert forever. With the fix, folding the shard drives it to 0 and finalize succeeds.
    let b = ctx.submit_order(&pdas, &t, SIDE_BUY, 40, 7);
    ctx.process_chunk_shard(&pdas, 0, 0, 16);
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "finalize succeeds after submit+cancel+fresh-order (no phantom pending count)",
    );

    // And the round drains + rolls cleanly.
    let (_m, fill) = ctx.settle_fill(&pdas, b);
    assert_eq!(fill, 0, "non-crossing buy → zero fill");
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).phase, PHASE_COLLECT, "reopened Collect");
}

/// A full multi-shard round: fold every non-empty shard, finalize, settle, then roll —
/// which `reset_shard`s all shards before `start_auction` (the `shards_ready` gate).
#[test]
fn multi_shard_round_rolls_after_all_shards_reset() {
    let mut ctx = TestContext::new();
    ctx.market_num_slab_shards = 3;
    let pdas = ctx.init_market(10, 64, 16);
    let id0 = ctx.market(&pdas).current_auction_id;

    // Two non-crossing taker buys in shard 0 (no maker sells → zero fills, so no
    // Position/collateral is required to settle them).
    let t = ctx.new_funded_signer();
    let o1 = ctx.submit_order(&pdas, &t, SIDE_BUY, 40, 5);
    let o2 = ctx.submit_order(&pdas, &t, SIDE_BUY, 50, 3);

    // Fold shard 0 (the only non-empty shard). Empty shards 1 & 2 need no crank.
    ctx.process_chunk_shard(&pdas, 0, 0, 16);
    ctx.finalize_clear(&pdas);
    let cr = ctx.clearing(&pdas).expect("cleared");
    assert_eq!(cr.ask_matched_volume, 0, "no maker sells → no cross");

    // Settle both orders (zero fill → simply consumed).
    let (_m1, f1) = ctx.settle_fill(&pdas, o1);
    let (_m2, f2) = ctx.settle_fill(&pdas, o2);
    assert_eq!(f1, 0);
    assert_eq!(f2, 0);
    assert_eq!(
        ctx.order_slab_shard(&pdas, 0).unwrap().count,
        0,
        "shard drained"
    );

    // Roll: start_auction resets all 3 shards (shards_ready == num_slab_shards) then
    // bumps the round.
    ctx.start_auction(&pdas);
    let m = ctx.market(&pdas);
    assert_eq!(m.current_auction_id, id0 + 1, "auction id bumped");
    assert_eq!(m.phase, PHASE_COLLECT, "reopened Collect");
    for shard in 0..pdas.num_slab_shards {
        let s = ctx.order_slab_shard(&pdas, shard).unwrap();
        assert_eq!(s.count, 0, "shard {shard} empty after roll");
        assert_eq!(
            s.auction_id,
            id0 + 1,
            "shard {shard} re-armed to the new round"
        );
    }
}

/// Review bugs 3 & 4: a multi-shard `force_reset` must clear EVERY shard and bump the
/// auction id EXACTLY ONCE, leaving all shards at the same new id — otherwise stale orders
/// bleed into the next round (bug 3) or the shards desync and the next roll wedges (bug 4).
#[test]
fn multi_shard_force_reset_recovers_and_next_round_rolls() {
    let mut ctx = TestContext::new();
    ctx.market_num_slab_shards = 3;
    let pdas = ctx.init_market(10, 64, 16);
    let id0 = ctx.market(&pdas).current_auction_id;

    // Wedge the round: unsettled orders resting in TWO different shards.
    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    ctx.submit_order_to_shard(&pdas, &a, SIDE_BUY, 40, 5, 0);
    ctx.submit_order_to_shard(&pdas, &b, SIDE_BUY, 40, 6, 1);
    assert_eq!(ctx.order_slab_shard(&pdas, 0).unwrap().count, 1);
    assert_eq!(ctx.order_slab_shard(&pdas, 1).unwrap().count, 1);

    // One atomic force_reset clears ALL shards and bumps the id exactly once.
    ctx.force_reset(&pdas);
    let m = ctx.market(&pdas);
    assert_eq!(
        m.current_auction_id,
        id0 + 1,
        "auction id bumped exactly once (not once per shard — bug 4)"
    );
    assert_eq!(m.phase, PHASE_COLLECT, "reopened Collect");
    for shard in 0..pdas.num_slab_shards {
        let s = ctx.order_slab_shard(&pdas, shard).unwrap();
        assert_eq!(
            s.count, 0,
            "shard {shard} cleared (no stale orders — bug 3)"
        );
        assert_eq!(
            s.auction_id,
            id0 + 1,
            "shard {shard} at the SAME new id (no desync — bug 4)"
        );
    }

    // The market is fully recovered: a clean round now runs AND rolls again. If the shards
    // had desynced ids, reset_shard's id check would fail here and start_auction would wedge.
    let t = ctx.new_funded_signer();
    let o = ctx.submit_order(&pdas, &t, SIDE_BUY, 40, 5);
    ctx.process_chunk_shard(&pdas, 0, 0, 16);
    ctx.finalize_clear(&pdas);
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(fill, 0);
    ctx.start_auction(&pdas);
    assert_eq!(
        ctx.market(&pdas).current_auction_id,
        id0 + 2,
        "next roll succeeds after force_reset recovery",
    );
}
