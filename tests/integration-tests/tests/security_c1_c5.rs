//! Regression tests for the two settlement-path criticals from the security
//! review:
//!
//! * A non-zero fill must never be silently discarded. Settling an
//!   order that matched volume while omitting the trader's Position is rejected
//!   (`MissingSettleAccounts`); the fill survives and settles correctly once the
//!   Position is supplied.
//! * `finalize_clear` derives the ClearingResult bump canonically and
//!   rejects an attacker-supplied non-canonical bump, so a market can never be
//!   wedged in `Discovered` with the result written at an off-canonical PDA.

use solana_sdk::pubkey::Pubkey;
use tempo_integration_tests::*;

/// Cross a simple bid book (maker-buy 20 via the quote book vs taker-sell 20 @
/// price 40) and run the auction up to `Discovered`. The C1 guard under test is on
/// the *slab order* settle path (`settle_fill`), so the order it returns is the
/// taker-sell (`sell_id`) — the maker liquidity now lives in the quote book
/// (§1.3) and settles via its own path. Returns `(pdas, sell_id, maker_pubkey)`.
fn cross_and_discover(ctx: &mut TestContext) -> (MarketPdas, u64, Pubkey) {
    let pdas = ctx.init_market(10, 16, 64);
    let buyer = ctx.new_funded_signer();
    let seller = ctx.new_funded_signer();
    ctx.post_maker_order(&pdas, &buyer, SIDE_BUY, 40, 20);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 40, 20);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &buyer.pubkey());
    ctx.finalize_clear(&pdas);
    (pdas, sell_id, buyer.pubkey())
}

#[test]
fn nonzero_fill_settle_requires_position() {
    let mut ctx = TestContext::new();
    let (pdas, sell_id, maker) = cross_and_discover(&mut ctx);

    let cr = ctx.clearing(&pdas).expect("cleared");
    assert_eq!(cr.bid_matched_volume, 20, "the book crossed for 20");

    // Settling the matched slab order (the taker-sell) with the raw 6-account form
    // (no Position) must be rejected — otherwise the fill would be consumed and
    // discarded. This is the C1 guard on the slab-order settle path.
    assert!(
        ctx.try_settle_fill_no_position(&pdas, sell_id).is_err(),
        "a non-zero fill cannot be settled without the owner's Position"
    );

    // The order is untouched (still Accumulated, full remaining) — nothing lost.
    let order = ctx
        .orders(&pdas)
        .into_iter()
        .find(|o| o.order_id == sell_id)
        .unwrap();
    assert_eq!(order.status, STATUS_ACCUMULATED, "order not consumed");
    assert_eq!(order.remaining, 20, "remaining intact");

    // Supplying the Position (the harness attaches it) settles the fill properly.
    let (_m, fill) = ctx.settle_fill(&pdas, sell_id);
    assert_eq!(fill, 20, "the full matched fill is recorded");
    let order = ctx
        .orders(&pdas)
        .into_iter()
        .find(|o| o.order_id == sell_id)
        .unwrap();
    assert_eq!(order.status, STATUS_CONSUMED, "now consumed");

    // The maker counterparty settles the matched fill via the quote-settle path.
    ctx.settle_maker_quote_clearing(&pdas, &maker);
    let (mpos, _) = ctx.position_pda(&pdas, &maker);
    assert_eq!(
        ctx.position(&mpos).size,
        20,
        "maker is long 20 (the crossed counterparty fill)"
    );
}

#[test]
fn non_canonical_clearing_bump_rejected() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);
    let buyer = ctx.new_funded_signer();
    let seller = ctx.new_funded_signer();
    // Maker-buy 20 via the quote book (§1.3) vs taker-sell 20 — one crossing book.
    ctx.post_maker_order(&pdas, &buyer, SIDE_BUY, 40, 20);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 40, 20);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &buyer.pubkey());

    // A bump that is not the canonical one for the clearing PDA must be rejected
    // by finalize_clear (it derives the canonical bump itself).
    let bad_bump = pdas.clearing_bump.wrapping_sub(1);
    assert_ne!(bad_bump, pdas.clearing_bump);
    assert!(
        ctx.try_finalize_clear_with_bump(&pdas, bad_bump).is_err(),
        "finalize must reject a non-canonical clearing bump"
    );

    // The market is NOT bricked: it is still in Accumulating and a correct
    // finalize succeeds, after which settlement proceeds normally.
    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_ACCUMULATING,
        "phase unchanged"
    );
    ctx.finalize_clear(&pdas);
    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_DISCOVERED,
        "finalized cleanly"
    );

    let (_m, fill) = ctx.settle_fill(&pdas, sell_id);
    assert_eq!(fill, 20, "settlement works against the canonical result");
    ctx.settle_maker_quote_clearing(&pdas, &buyer.pubkey());
}
