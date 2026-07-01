//! Stage B — resting orders (plan §3). An unfilled / partially-filled order is not
//! discarded at settle: it re-arms as `Resting` and carries to the next round, so a
//! trader places once and the order lives until it is fully filled, expires, or is
//! cancelled. These tests pin the Stage-B-specific invariants:
//!   * a carried Resting order must be re-folded before the next round can finalize
//!     (the DDR-1 re-review trigger: the authoritative per-shard finalize scan still
//!     gates completeness after orders re-fold each round);
//!   * the roll gate is "no order still `Accumulated`" (fully SETTLED), not the old
//!     drain-to-`count == 0` — a folded-but-unsettled order blocks the roll, but a
//!     re-armed Resting survivor does not, and it carries across the roll;
//!   * `expires_at_auction` consumes a leftover instead of re-arming it;
//!   * conservation — a partial fill that rests then completes next round fills EXACTLY
//!     its original quantity (Σ fills across rounds == original qty).

use tempo_integration_tests::*;

const STATUS_RESTING: u8 = 1;
const STATUS_CONSUMED: u8 = 3;

/// DDR-1 re-review trigger. A non-crossing taker buy fills nothing, so at settle it is
/// re-armed `Resting` (not `Consumed`) and carries to the next round. In that next round
/// it is unfolded again, so `finalize_clear` MUST refuse until it is re-folded — proving
/// the authoritative per-shard completeness scan still holds with resting orders.
#[test]
fn carried_resting_order_blocks_finalize_next_round() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let id0 = ctx.market(&pdas).current_auction_id;

    // A lone taker buy — no maker sells, so it crosses nothing (zero fill).
    let t = ctx.new_funded_signer();
    let o = ctx.submit_order(&pdas, &t, SIDE_BUY, 40, 5);

    // Round 1: fold, finalize, settle. Zero fill + still live ⇒ re-armed Resting.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(fill, 0, "non-crossing buy fills nothing");
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_RESTING,
        "zero-fill live order re-armed Resting"
    );
    assert_eq!(rec.remaining, 5, "remaining unchanged");
    assert_eq!(
        ctx.order_slab(&pdas).count,
        1,
        "order stays in the book (count kept)"
    );

    // Roll — the Resting survivor carries (it is not Accumulated, so the roll gate passes).
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).current_auction_id, id0 + 1, "rolled");
    assert_eq!(
        ctx.order_slab(&pdas).count,
        1,
        "carried into the next round"
    );

    // Round 2: enter Accumulating by folding a slot range that SKIPS the carried order
    // (it rests in slot 0; start at slot 1 folds nothing). The carried order is thus still
    // Resting (unfolded), so finalize must be refused — the censorship guarantee.
    ctx.process_chunk(&pdas, 1, 64);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);
    assert!(
        ctx.try_finalize_clear(&pdas).is_err(),
        "a carried Resting order not re-folded must block finalize",
    );

    // Re-fold it → completeness satisfied → finalize succeeds.
    ctx.process_chunk(&pdas, 0, 64);
    assert!(
        ctx.try_finalize_clear(&pdas).is_ok(),
        "once re-folded, the round finalizes",
    );
}

/// The Stage-B roll gate is "no order still `Accumulated`" (fully settled), not the old
/// `count == 0`. A folded-but-unsettled order must block `reset_shard`; once settled (here
/// re-armed Resting on a zero fill), the shard resets and the survivor carries.
#[test]
fn roll_gate_rejects_unsettled_then_carries_resting() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let id0 = ctx.market(&pdas).current_auction_id;

    let t = ctx.new_funded_signer();
    let o = ctx.submit_order(&pdas, &t, SIDE_BUY, 40, 5);

    // Fold (→ Accumulated) and finalize, but do NOT settle yet.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);

    // The order is Accumulated (folded, unsettled) → reset_shard must refuse, even though
    // it would satisfy the old count==0 gate only after settle. (count is still 1 here.)
    assert!(
        ctx.try_reset_shard(&pdas, 0).is_err(),
        "roll must refuse a shard with a folded-but-unsettled (Accumulated) order",
    );

    // Settle it (zero fill → re-armed Resting). Now no order is Accumulated.
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(fill, 0);

    // Roll now succeeds and the Resting survivor carries with count preserved.
    ctx.start_auction(&pdas);
    let m = ctx.market(&pdas);
    assert_eq!(m.current_auction_id, id0 + 1, "rolled once settled");
    assert_eq!(m.phase, PHASE_COLLECT, "reopened Collect");
    let s = ctx.order_slab_shard(&pdas, 0).unwrap();
    assert_eq!(s.count, 1, "survivor carried");
    assert_eq!(s.auction_id, id0 + 1, "shard re-armed to the new round");
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.status, STATUS_RESTING, "carried as Resting");
}

/// `expires_at_auction` bounds how long an order rests. An order live in the round it is
/// submitted but past its expiry a round later is `Consumed` at settle (leaves the book),
/// not re-armed — even though it filled nothing.
#[test]
fn expired_resting_order_is_consumed_not_rearmed() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let id0 = ctx.market(&pdas).current_auction_id; // 0

    // Expire at auction id0+1: still live in round id0, expired when settled in round id0+1.
    let t = ctx.new_funded_signer();
    let o = ctx.submit_order_expiring(&pdas, &t, SIDE_BUY, 40, 5, id0 + 1);

    // Round id0: zero fill, expiry (id0+1) > current (id0) ⇒ NOT expired ⇒ re-armed Resting.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, o);
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.status, STATUS_RESTING, "not yet expired → carries");
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).current_auction_id, id0 + 1);

    // Round id0+1: expiry (id0+1) <= current (id0+1) ⇒ expired ⇒ Consumed, leaves the book.
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.settle_fill(&pdas, o);
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_CONSUMED,
        "expired order is consumed, not re-armed"
    );
    assert_eq!(
        ctx.order_slab(&pdas).count,
        0,
        "expired order left the book (count decremented)"
    );
}

/// Conservation across rounds: a taker sell that only PARTIALLY fills rests with its
/// leftover, then completes against fresh liquidity next round — filling EXACTLY its
/// original quantity over the two rounds and no more (Σ fills == original qty).
#[test]
fn partial_fill_rests_then_completes_conserving() {
    let mut ctx = TestContext::new();
    let tick = 10u64;
    let pdas = ctx.init_market(tick, 16, 64);
    let mid_tick = 3u32; // price 40 = tick 3 (price/tick_size - 1)

    let mb = ctx.new_funded_signer(); // maker buyer (bid demand, quote book)
    let ts = ctx.new_funded_signer(); // taker seller (bid supply, slab)
    ctx.init_position(&pdas, &mb);

    // Round 1: maker buys 12, taker sells 20 at price 40 → cross 12. Taker partially fills.
    ctx.post_maker_order(&pdas, &mb, SIDE_BUY, 4 * tick, 12);
    let o = ctx.submit_order(&pdas, &ts, SIDE_SELL, 4 * tick, 20);

    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &mb.pubkey());
    ctx.finalize_clear(&pdas);
    assert_eq!(
        ctx.clearing(&pdas).unwrap().bid_matched_volume,
        12,
        "bid auction matched the maker's 12"
    );

    let (_m, cum1) = ctx.settle_fill(&pdas, o);
    assert_eq!(cum1, 12, "round-1 fill is the crossed 12");
    ctx.settle_maker_quote_clearing(&pdas, &mb.pubkey());
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.status, STATUS_RESTING, "partial fill re-arms Resting");
    assert_eq!(rec.remaining, 8, "leftover 20 - 12 = 8 carries");

    // Roll — the partially-filled taker carries into round 2.
    ctx.start_auction(&pdas);
    assert_eq!(ctx.order_slab(&pdas).count, 1, "leftover carried");

    // Round 2: re-arm the maker to buy the remaining 8, then clear.
    ctx.update_maker_quote_levels(&pdas, &mb, 2, mid_tick, &[(0, 8)], &[]);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &mb.pubkey());
    ctx.finalize_clear(&pdas);
    assert_eq!(
        ctx.clearing(&pdas).unwrap().bid_matched_volume,
        8,
        "round-2 cross is the remaining 8"
    );

    let (_m2, cum2) = ctx.settle_fill(&pdas, o);
    // `order_fill` is cumulative (quantity - remaining): after round 2 the order is fully
    // filled, so this reads the full original quantity — conservation.
    assert_eq!(
        cum2, 20,
        "Σ fills across rounds == original 20 (no over-fill)"
    );
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(
        rec.status, STATUS_CONSUMED,
        "fully filled → leaves the book"
    );
    assert_eq!(rec.remaining, 0, "nothing left to fill");
}
