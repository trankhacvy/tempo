//! The circuit breaker (missing-features §3.2, plan.md §2.2).
//!
//! The design contract under test: a pause blocks INTAKE only. The in-flight
//! round always drains (cranks + settles run), and users can always exit
//! (cancel, withdraw, liquidate) — a pause can never trap funds. `PAUSE_ROLL`
//! additionally parks the market quiescent after the round settles.

use tempo_integration_tests::*;

const PAUSE_INTAKE: u8 = 1;
const PAUSE_ROLL: u8 = 2;

#[test]
fn pause_intake_blocks_submits_and_quote_writes_but_never_exits() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let trader = ctx.new_funded_signer();
    let maker = ctx.new_funded_signer();

    // Pre-pause: a resting order exists (we prove cancel still works later).
    let o = ctx.submit_order(&pdas, &trader, SIDE_BUY, 40, 5);

    ctx.set_pause(&pdas, PAUSE_INTAKE);

    // Intake rejects: submit + new quote both fail MarketPaused (Custom 2).
    assert!(
        ctx.try_submit_order(&pdas, &trader, SIDE_BUY, 50, 5)
            .is_err(),
        "submit_order must reject while intake is paused"
    );
    assert!(
        ctx.try_init_maker_quote(&pdas, &maker).is_err(),
        "init_maker_quote must reject while intake is paused"
    );

    // Exits never pause: the pre-pause order cancels fine (margin path is a
    // no-money market here, so cancel is the exit that matters).
    ctx.cancel_order(&pdas, &trader, o);
    assert_eq!(ctx.order_slab(&pdas).count, 0, "cancel worked while paused");

    // The (now empty) round still cranks + rolls: pause_intake ≠ pause_roll.
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).phase, PHASE_COLLECT, "round rolled");

    // Resume: intake works again.
    ctx.set_pause(&pdas, 0);
    ctx.submit_order(&pdas, &trader, SIDE_BUY, 40, 5);
    assert_eq!(ctx.order_slab(&pdas).count, 1, "submit works after resume");
}

#[test]
fn pause_roll_drains_the_round_then_parks_quiescent() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let trader = ctx.new_funded_signer();
    let o = ctx.submit_order(&pdas, &trader, SIDE_BUY, 40, 5);

    // Pause BOTH bits mid-round: the round must still drain to fully settled.
    ctx.set_pause(&pdas, PAUSE_INTAKE | PAUSE_ROLL);
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    let (_m, fill) = ctx.settle_fill(&pdas, o);
    assert_eq!(fill, 0, "round drained (zero-fill settle ran while paused)");

    // ...but the roll is parked: start_auction rejects with MarketPaused.
    assert!(
        ctx.try_start_auction(&pdas).is_err(),
        "start_auction must reject while PAUSE_ROLL is set"
    );

    // Resume the roll bit only → the market rolls.
    ctx.set_pause(&pdas, 0);
    ctx.start_auction(&pdas);
    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_COLLECT,
        "rolled after resume"
    );
}

#[test]
fn set_pause_rejects_stranger_and_unknown_bits() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);

    // A non-authority signer is rejected (InvalidAuthority).
    let stranger = ctx.new_funded_signer();
    assert!(
        ctx.try_set_pause_signed(&pdas, &stranger, PAUSE_INTAKE)
            .is_err(),
        "only the market authority may pause"
    );

    // Unknown bits are rejected at parse (MarketConfigOutOfRange) so a future
    // flag can never be set accidentally by an old client.
    let authority_err = {
        let authority = ctx
            .market_authority_keypair(&pdas)
            .expect("authority recorded");
        ctx.try_set_pause_signed(&pdas, &authority, 0b100)
    };
    assert!(
        authority_err.is_err(),
        "unknown pause bits must be rejected"
    );

    // The market is still unpaused after both rejections.
    let trader = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &trader, SIDE_BUY, 40, 5);
}
