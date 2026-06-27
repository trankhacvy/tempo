//! Anti-spam: a single trader cannot flood the order slab. The per-trader
//! cap (8 resting orders per auction) bounds how many slots one account holds,
//! so dust-spam can't lock legitimate traders out for tx-fee-only cost.

use tempo_integration_tests::*;

#[test]
fn per_trader_order_cap_enforced() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64); // cap 64 >> per-trader cap

    let spammer = ctx.new_funded_signer();
    // Up to the per-trader cap (8) is allowed.
    for _ in 0..8 {
        ctx.submit_order(&pdas, &spammer, SIDE_BUY, 40, 5);
    }
    // The 9th from the same trader is rejected (cap, not slab-full).
    assert!(
        ctx.try_submit_order(&pdas, &spammer, SIDE_BUY, 40, 5)
            .is_err(),
        "9th order from one trader must hit the per-trader cap"
    );

    // A different trader is unaffected — the slab still has room.
    let other = ctx.new_funded_signer();
    let _ = ctx.submit_order(&pdas, &other, SIDE_BUY, 40, 5);
    assert_eq!(ctx.market(&pdas).active_order_count, 9, "8 + 1 active");
}
