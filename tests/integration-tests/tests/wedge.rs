//! Empty-round wedge: an order-less round reaches `Discovered` but never
//! `Settling` (no settle_fill to make the transition), so `start_auction` must be
//! able to roll it from `Discovered` when the slab is empty — otherwise the round
//! wedges forever and only the authority-gated force_reset escapes.

use tempo_integration_tests::*;

#[test]
fn start_auction_rolls_empty_discovered_round() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);

    // No orders submitted. Close the window, accumulate nothing, finalize.
    let deadline = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(deadline);
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);
    ctx.finalize_clear(&pdas);
    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_DISCOVERED,
        "an empty round finalizes into Discovered",
    );
    assert_eq!(ctx.order_slab(&pdas).count, 0);

    // Without this fix the round wedges (start_auction required Settling). It must
    // now roll the empty round forward.
    ctx.start_auction(&pdas);
    let m = ctx.market(&pdas);
    assert_eq!(
        m.phase, PHASE_COLLECT,
        "empty Discovered round rolls to Collect"
    );
    assert_eq!(m.current_auction_id, 1, "auction id bumped");
}

#[test]
fn start_auction_refuses_discovered_with_orders() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);

    // A single taker order keeps the slab populated (no cross needed here); the
    // unsettled order is what blocks start_auction.
    let trader = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &trader, SIDE_BUY, 40, 10);

    let deadline = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(deadline);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    assert_eq!(ctx.market(&pdas).phase, PHASE_DISCOVERED);
    assert_eq!(
        ctx.order_slab(&pdas).count,
        1,
        "the unsettled order keeps the slab populated",
    );

    // A Discovered round that still holds orders must NOT roll — those orders
    // have to settle first (the empty-slab precondition is the real gate).
    assert!(
        ctx.try_start_auction(&pdas).is_err(),
        "start_auction must refuse a Discovered round with unsettled orders",
    );
}
