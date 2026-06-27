//! `settle_fill` applies the fill to a trader's `Position`.
//!
//! Proves the optional-Position path end-to-end on-chain: init a Position,
//! clear a crossing book, settle the owner's order *with* the position account,
//! and assert the position reflects the fill (size + VWAP entry).

use tempo_integration_tests::*;

#[test]
fn settle_applies_fill_to_position() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    // The maker buyer gets a Position; the taker seller settles without one.
    let maker_buyer = ctx.new_funded_signer();
    let taker_seller = ctx.new_funded_signer();
    let position = ctx.init_position(&pdas, &maker_buyer);

    // Bid auction crosses at tick 3 (price 40): maker-buy 50 (quote book) vs
    // taker-sell 50 (submit_order). Maker liquidity comes from the MakerQuote
    // book — submit_order is taker-only (§1.3).
    ctx.post_maker_order(&pdas, &maker_buyer, SIDE_BUY, 40, 50);
    let sell_id = ctx.submit_order(&pdas, &taker_seller, SIDE_SELL, 40, 50);

    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &maker_buyer.pubkey());
    ctx.finalize_clear(&pdas);
    let cr = ctx.clearing(&pdas).expect("cleared");
    assert_eq!(cr.bid_clearing_price, 40);
    assert_eq!(cr.bid_matched_volume, 50);

    // Settle the maker quote into its position; the taker-sell plainly.
    ctx.settle_maker_quote_clearing(&pdas, &maker_buyer.pubkey());
    let (_m, sell_fill) = ctx.settle_fill(&pdas, sell_id);
    assert_eq!(sell_fill, 50, "taker seller fully filled");

    // The position now holds a +50 long opened at the clearing price.
    let p = ctx.position(&position);
    assert_eq!(p.owner, maker_buyer.pubkey());
    assert_eq!(p.market, pdas.market);
    assert_eq!(p.size, 50, "long of the filled quantity");
    assert_eq!(p.entry_price, 40, "entry at the clearing price");
    assert_eq!(p.realized_pnl, 0, "nothing realized on an opening fill");
}
