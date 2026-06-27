//! Plan task 1.2.7 — event emission via self-CPI.
//!
//! Every state-changing instruction emits its event through a self-CPI to the
//! program's `EmitEvent` handler (discriminator 228), signed by the
//! `event_authority` PDA. That inner instruction is visible in the transaction
//! logs as `Program <tempo> invoke [2]` and in the `inner_instructions` list.
//! Here we capture a `finalize_clear` transaction and assert the
//! `ClearingFinalized` event CPI actually happened.

use tempo_integration_tests::*;

#[test]
fn finalize_clear_emits_clearing_finalized_event_via_cpi() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    let buyer = ctx.new_funded_signer(); // maker buyer -> bid demand (quote book)
    let seller = ctx.new_funded_signer(); // taker seller -> bid supply (slab)
    ctx.init_position(&pdas, &buyer);
    ctx.post_maker_order(&pdas, &buyer, SIDE_BUY, 40, 10);
    ctx.submit_order(&pdas, &seller, SIDE_SELL, 40, 10);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &buyer.pubkey());

    let meta = ctx.finalize_clear(&pdas);

    // The self-CPI event emission appears as a depth-2 invocation of the program.
    // (We assert on the event CPI itself, not on any program log — processors no
    // longer log on the hot/clearing paths; the event carries the data.)
    let self_cpis = count_self_cpi_invocations(&meta.logs);
    assert!(
        self_cpis >= 1,
        "expected at least one self-CPI (event emission) at depth 2, logs: {:#?}",
        meta.logs
    );

    // The inner-instruction list must record the emitted event instruction.
    let inner_total: usize = meta.inner_instructions.iter().map(|v| v.len()).sum();
    assert!(
        inner_total >= 1,
        "expected at least one inner instruction (the event CPI), got {inner_total}"
    );
}

#[test]
fn submit_order_also_emits_an_event_cpi() {
    // Confirm the self-CPI machinery fires on a simpler instruction too.
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    let trader = ctx.new_funded_signer();
    let meta = ctx
        .try_submit_order(&pdas, &trader, SIDE_BUY, 40, 10)
        .expect("submit ok");

    assert!(
        count_self_cpi_invocations(&meta.logs) >= 1,
        "submit_order should emit an event via self-CPI"
    );
}
