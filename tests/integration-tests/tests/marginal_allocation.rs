//! Exact open-interest conservation at the marginal tick.
//!
//! Construct a bid auction that rations the *supply* side across several orders
//! whose pro-rata shares do NOT divide evenly: three taker-sells of 10 (=30)
//! against one maker-buy of 20 at the same price. Matched volume is 20, so the
//! supply side is rationed 20-of-30.
//!
//! Plain floor pro-rata would fill each sell `floor(10*20/30) = 6` → 18 total,
//! leaving a 2-unit imbalance against the demand side's 20 (the dust/OI leak the
//! review flagged). The cumulative-floor allocation in `settle_fill` fills
//! 6 + 7 + 7 = 20, so filled-supply == filled-demand == matched-volume exactly.
//!
//! NB: the rationed multi-order side must be the taker side (settled via
//! `settle_fill`'s cumulative-floor split). Multiple *distinct makers* sharing
//! the exact marginal tick is a documented follow-up in the maker-quote settle
//! path (each quote settles independently, so the cumulative-floor split isn't
//! shared) — so the scarce, fully-filled side is the maker here.

use tempo_integration_tests::*;

#[test]
fn marginal_tick_allocation_conserves_open_interest() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    // Bid auction: demand = maker-buy, supply = taker-sells, all at price 40.
    let buyer = ctx.new_funded_signer();
    let s0 = ctx.new_funded_signer();
    let s1 = ctx.new_funded_signer();
    let s2 = ctx.new_funded_signer();

    // Scarce demand = one maker-buy of 20 (via the quote book). Rationed supply =
    // three taker-sells of 10 at the same price 40.
    ctx.post_maker_order(&pdas, &buyer, SIDE_BUY, 40, 20);
    let sell0 = ctx.submit_order(&pdas, &s0, SIDE_SELL, 40, 10);
    let sell1 = ctx.submit_order(&pdas, &s1, SIDE_SELL, 40, 10);
    let sell2 = ctx.submit_order(&pdas, &s2, SIDE_SELL, 40, 10);

    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &buyer.pubkey());
    ctx.finalize_clear(&pdas);

    let cr = ctx.clearing(&pdas).expect("cleared");
    assert_eq!(cr.bid_clearing_price, 40, "cleared at the common price");
    assert_eq!(cr.bid_matched_volume, 20, "min(demand 20, supply 30)");

    // Settle the rationed supply side; the fills must sum to EXACTLY the matched
    // volume (no dust), and individually be the cumulative-floor split 6/7/7.
    let (_, f0) = ctx.settle_fill(&pdas, sell0);
    let (_, f1) = ctx.settle_fill(&pdas, sell1);
    let (_, f2) = ctx.settle_fill(&pdas, sell2);
    let supply_filled = f0 + f1 + f2;
    let mut sorted = [f0, f1, f2];
    sorted.sort_unstable();
    assert_eq!(sorted, [6, 7, 7], "cumulative-floor split, not 6/6/6");
    assert_eq!(
        supply_filled, 20,
        "supply fills sum to exactly the matched volume"
    );

    // The scarce demand side (the maker) fills fully.
    ctx.settle_maker_quote_clearing(&pdas, &buyer.pubkey());
    let demand_filled = ctx
        .position(&ctx.position_pda(&pdas, &buyer.pubkey()).0)
        .size as u64;
    assert_eq!(demand_filled, 20, "scarce demand fills fully");

    // The conservation invariant: filled supply == filled demand == matched.
    assert_eq!(
        supply_filled, demand_filled,
        "open interest conserved (no marginal-tick dust leak)"
    );
}
