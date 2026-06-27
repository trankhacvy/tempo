//! Phase-machine guards:
//!  - `submit_order` is rejected once the market has left Collect (Accumulating).
//!  - `finalize_clear` fails the completeness check when a chunk is skipped so
//!    not every active order has been accumulated.

use tempo_integration_tests::*;

#[test]
fn submit_order_rejected_after_accumulating() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &a, SIDE_BUY, 30, 10);
    ctx.submit_order(&pdas, &b, SIDE_SELL, 30, 10);

    // Transition into Accumulating by processing a chunk.
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);

    // A further submit must now be rejected (wrong phase).
    let c = ctx.new_funded_signer();
    let res = ctx.try_submit_order(&pdas, &c, SIDE_BUY, 30, 5);
    assert!(res.is_err(), "submit_order must fail once Accumulating");
    // active_order_count is unchanged.
    assert_eq!(ctx.market(&pdas).active_order_count, 2);
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
    assert_eq!(ctx.market(&pdas).active_order_count, 4);

    // Only accumulate the first 3 slots, deliberately skipping slot 3.
    ctx.process_chunk(&pdas, 0, 3);
    let m = ctx.market(&pdas);
    assert_eq!(m.phase, PHASE_ACCUMULATING);
    assert_eq!(
        m.accumulated_order_count, 3,
        "one order left un-accumulated"
    );
    assert!(
        m.accumulated_order_count != m.active_order_count,
        "incomplete"
    );

    // Completeness check must reject finalize.
    let res = ctx.try_finalize_clear(&pdas);
    assert!(
        res.is_err(),
        "finalize must fail when not all orders accumulated"
    );

    // Once the skipped slot is folded, finalize succeeds.
    ctx.process_chunk(&pdas, 3, 1);
    assert_eq!(ctx.market(&pdas).accumulated_order_count, 4);
    ctx.finalize_clear(&pdas);
    assert_eq!(ctx.market(&pdas).phase, PHASE_DISCOVERED);
}
