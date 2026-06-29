//! Collection-window enforcement: process_chunk must refuse to start
//! accumulating until the market's `phase_deadline_slot` is reached, so orders
//! submitted anywhere in the window land in the same batch.

use tempo_integration_tests::*;

#[test]
fn collect_window_blocks_early_accumulation() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);

    // A single taker order exercises the window mechanics (no cross needed); it
    // rests in the slab and is folded by process_chunk once the window closes.
    let trader = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &trader, SIDE_BUY, 40, 10);

    let deadline = ctx.phase_deadline_slot(&pdas);

    // One slot before the deadline: accumulation is rejected, phase stays Collect.
    ctx.warp_slot(deadline.saturating_sub(1));
    assert!(
        ctx.try_process_chunk(&pdas, 0, 64).is_err(),
        "process_chunk must fail while the collection window is open",
    );
    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_COLLECT,
        "phase stays Collect when accumulation is blocked",
    );

    // At the deadline: accumulation proceeds and the phase advances.
    ctx.warp_slot(deadline);
    ctx.process_chunk(&pdas, 0, 64);
    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_ACCUMULATING,
        "process_chunk transitions to Accumulating once the window closes",
    );
    assert_eq!(ctx.histogram(&pdas).accumulated_count, 1);
}
