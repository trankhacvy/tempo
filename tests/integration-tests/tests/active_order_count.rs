//! Plan task 0.3 — after a sequence of submits and cancels, the authoritative
//! live-order count `OrderSlabHeader.count` must equal the number of orders still
//! resting in the slab. (PERF-1 removed the redundant `Market.active_order_count`
//! mirror; the slab header count is now the single source of truth — known-issues §2.1.)

use tempo_integration_tests::*;

#[test]
fn counts_track_resting_orders_through_submits_and_cancels() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 32);

    let alice = ctx.new_funded_signer();
    let bob = ctx.new_funded_signer();
    let carol = ctx.new_funded_signer();

    // Submit 5 orders.
    let a0 = ctx.submit_order(&pdas, &alice, SIDE_BUY, 30, 10);
    let _a1 = ctx.submit_order(&pdas, &alice, SIDE_SELL, 40, 5);
    let b0 = ctx.submit_order(&pdas, &bob, SIDE_BUY, 50, 7);
    let _c0 = ctx.submit_order(&pdas, &carol, SIDE_SELL, 30, 3);
    let b1 = ctx.submit_order(&pdas, &bob, SIDE_BUY, 20, 4);

    assert_eq!(ctx.order_slab(&pdas).count, 5);
    assert_eq!(ctx.orders(&pdas).len(), 5);

    // Cancel 2 (each by its owner).
    ctx.cancel_order(&pdas, &alice, a0);
    ctx.cancel_order(&pdas, &bob, b0);

    assert_eq!(ctx.order_slab(&pdas).count, 3);

    // Submit 2 more (ids keep climbing monotonically; next_order_id == 5,6).
    let d0 = ctx.submit_order(&pdas, &carol, SIDE_BUY, 60, 2);
    let _d1 = ctx.submit_order(&pdas, &alice, SIDE_SELL, 60, 9);

    // Cancel one of the fresh ones and one of the survivors.
    ctx.cancel_order(&pdas, &carol, d0);
    ctx.cancel_order(&pdas, &bob, b1);

    // Resting now: a1, c0, d1  => 3 orders.
    let resting = ctx.orders(&pdas);
    assert_eq!(resting.len(), 3, "three orders still resting");
    for o in &resting {
        assert_eq!(o.status, STATUS_RESTING);
    }

    // The authoritative slab header count must agree with the actual resting set.
    assert_eq!(ctx.order_slab(&pdas).count, resting.len() as u32);
}
