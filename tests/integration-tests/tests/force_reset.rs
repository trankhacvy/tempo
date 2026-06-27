//! Authority-gated force-reset: an escape hatch that abandons a wedged
//! round and reopens Collect, regardless of phase or unsettled orders.

use tempo_integration_tests::*;

#[test]
fn force_reset_recovers_a_wedged_round() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let id0 = ctx.market(&pdas).current_auction_id;

    // Take the round mid-flight: submit + accumulate (now in Accumulating with a
    // resting order that normal flow would require finalize+settle to clear).
    let t = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &t, SIDE_BUY, 40, 10);
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);

    // A non-authority signer cannot force-reset.
    let stranger = ctx.new_funded_signer();
    assert!(
        ctx.try_force_reset_by(&pdas, &stranger).is_err(),
        "force_reset must reject a non-authority signer",
    );

    // The authority can: the round rolls to Collect, the id bumps, slab empties.
    ctx.force_reset(&pdas);
    let m = ctx.market(&pdas);
    assert_eq!(m.phase, PHASE_COLLECT, "back to Collect");
    assert_eq!(m.current_auction_id, id0 + 1, "auction id bumped");
    assert_eq!(m.active_order_count, 0);
    assert_eq!(m.accumulated_order_count, 0);
    assert_eq!(ctx.order_slab(&pdas).count, 0, "slab emptied");

    // A fresh round works end-to-end after the reset.
    let t2 = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &t2, SIDE_BUY, 40, 5);
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);
    assert_eq!(ctx.market(&pdas).accumulated_order_count, 1);
}
