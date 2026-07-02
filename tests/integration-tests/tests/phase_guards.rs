//! Phase-machine guards:
//!  - `submit_order` is ALWAYS OPEN (Stage C1 / DDR-4): a submit after the market has
//!    left Collect succeeds and is armed for the NEXT round — it does not fold into or
//!    block the current round.
//!  - `finalize_clear` fails the completeness check when a chunk is skipped so
//!    not every active order has been accumulated.

use tempo_integration_tests::*;

#[test]
fn submit_after_accumulating_arms_next_round() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &a, SIDE_BUY, 30, 10);
    ctx.submit_order(&pdas, &b, SIDE_SELL, 30, 10);

    // Enter Accumulating and fold the two Collect-phase orders.
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);
    assert_eq!(ctx.histogram(&pdas).accumulated_count, 2);

    // Stage C1 / DDR-4 (always-open submission): a submit mid-round now SUCCEEDS and is
    // tagged for the NEXT round (`arm_auction_id = current + 1`) instead of being rejected.
    let c = ctx.new_funded_signer();
    let c_id = ctx.submit_order(&pdas, &c, SIDE_BUY, 30, 5);
    assert_eq!(
        ctx.order_slab(&pdas).count,
        3,
        "the deferred order was added"
    );

    // It is armed for the next round, so a further chunk does NOT fold it this round
    // (the folded count stays at the two Collect orders)...
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(
        ctx.histogram(&pdas).accumulated_count,
        2,
        "a next-round-armed order must not fold into the current round"
    );

    // ...and it does NOT block the current round's finalize (the completeness gate
    // exempts an order armed for a later round — DDR-4).
    ctx.finalize_clear(&pdas);
    assert_eq!(ctx.market(&pdas).phase, PHASE_DISCOVERED);

    // The deferred order is still Resting, carried to fold next round.
    let c_rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|o| o.order_id == c_id)
        .expect("deferred order still present");
    assert_eq!(c_rec.status, STATUS_RESTING);
}

#[test]
fn cancel_is_always_open_symmetric_with_submit() {
    // Stage C1 follow-up: submit is always-open, so cancel must be too — otherwise a
    // trader who submits mid-round cannot reclaim that order (and its locked margin)
    // until the next Collect. Cancelling a still-Resting order is safe in any phase
    // (it was never folded into the histogram).
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &a, SIDE_BUY, 30, 10);
    ctx.submit_order(&pdas, &b, SIDE_SELL, 30, 10);

    // Leave Collect.
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);

    // Submit a deferred order mid-round, then cancel it in the SAME (non-Collect) phase.
    let c = ctx.new_funded_signer();
    let c_id = ctx.submit_order(&pdas, &c, SIDE_BUY, 30, 5);
    assert_eq!(ctx.order_slab(&pdas).count, 3);

    ctx.cancel_order(&pdas, &c, c_id); // must succeed outside Collect now
    assert_eq!(
        ctx.order_slab(&pdas).count,
        2,
        "deferred order was cancelled"
    );
    assert!(
        ctx.orders(&pdas).into_iter().all(|o| o.order_id != c_id),
        "cancelled order is gone from the slab"
    );
}

#[test]
fn finalize_fails_when_chunk_skipped() {
    let mut ctx = TestContext::new();
    // capacity 8 so orders land in distinct, predictable slots.
    let pdas = ctx.init_market(10, 16, 8);

    // Submit 4 orders -> slots 0..=3.
    let t0 = ctx.new_funded_signer();
    let t1 = ctx.new_funded_signer();
    let t2 = ctx.new_funded_signer();
    let t3 = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &t0, SIDE_BUY, 40, 10);
    ctx.submit_order(&pdas, &t1, SIDE_SELL, 40, 10);
    ctx.submit_order(&pdas, &t2, SIDE_BUY, 50, 10);
    ctx.submit_order(&pdas, &t3, SIDE_SELL, 50, 10);
    assert_eq!(ctx.order_slab(&pdas).count, 4);

    // Only accumulate the first 3 slots, deliberately skipping slot 3.
    ctx.process_chunk(&pdas, 0, 3);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);
    // Authoritative completeness (PERF-1): the histogram's folded count is below the
    // slab's live count, so one order is still un-accumulated.
    let folded = ctx.histogram(&pdas).accumulated_count;
    let live = ctx.order_slab(&pdas).count as u64;
    assert_eq!(folded, 3, "one order left un-accumulated");
    assert!(folded != live, "incomplete");

    // Completeness check must reject finalize.
    let res = ctx.try_finalize_clear(&pdas);
    assert!(
        res.is_err(),
        "finalize must fail when not all orders accumulated"
    );

    // Once the skipped slot is folded, finalize succeeds.
    ctx.process_chunk(&pdas, 3, 1);
    assert_eq!(ctx.histogram(&pdas).accumulated_count, 4);
    ctx.finalize_clear(&pdas);
    assert_eq!(ctx.market(&pdas).phase, PHASE_DISCOVERED);
}
