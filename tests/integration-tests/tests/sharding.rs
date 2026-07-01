//! Stage A sharding (plan §2): the OrderSlab is split into `num_slab_shards`
//! independent shards that fold into the single histogram in parallel. These tests
//! prove the two adversarial invariants survive sharding:
//!   * cross-shard fold — orders in different shards fold into the same histogram
//!     (commutative addition), and
//!   * completeness — `finalize_clear` refuses until EVERY shard (even an empty one)
//!     has been folded (the `shards_pending` gate replacing the single-slab scan).

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

    // Every shard folded ⇒ completeness satisfied ⇒ finalize is accepted.
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "finalize succeeds once all shards are folded"
    );
}

/// The completeness gate is per-shard: `finalize_clear` must reject until EVERY shard
/// (including empty ones) has been folded, then accept — the censorship guarantee that
/// replaces the old single-slab scan.
#[test]
fn finalize_blocked_until_every_shard_folded() {
    let mut ctx = TestContext::new();
    ctx.market_num_slab_shards = 3;
    let pdas = ctx.init_market(10, 64, 16);

    // Orders only in shard 0; shards 1 and 2 stay empty but must still be folded.
    let t = ctx.new_funded_signer();
    ctx.submit_order_to_shard(&pdas, &t, SIDE_BUY, 40, 5, 0);

    // Fold shard 0 only → 2 shards still pending → finalize refused.
    ctx.process_chunk_shard(&pdas, 0, 0, 16);
    assert!(
        ctx.try_finalize_clear(&pdas).is_err(),
        "finalize must refuse while shards 1 & 2 are unfolded",
    );

    // Fold shard 1 → still one pending → still refused.
    ctx.process_chunk_shard(&pdas, 1, 0, 16);
    assert!(
        ctx.try_finalize_clear(&pdas).is_err(),
        "finalize must refuse while shard 2 is unfolded",
    );

    // Fold the last (empty) shard → completeness satisfied → finalize accepted.
    ctx.process_chunk_shard(&pdas, 2, 0, 16);
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "finalize succeeds once the last empty shard is folded",
    );
}

/// A full multi-shard round: fold every shard, finalize, settle, then roll — which
/// `reset_shard`s all shards before `start_auction` (the `shards_ready` gate).
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

    // Fold every shard (shard 0 has orders; 1 & 2 are empty but count for completeness).
    for shard in 0..pdas.num_slab_shards {
        ctx.process_chunk_shard(&pdas, shard, 0, 16);
    }
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
