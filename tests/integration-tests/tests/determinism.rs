//! Plan task 2.2 — determinism / order-independence.
//!
//! The same logical book is cleared two ways on two independent markets:
//!   1. one big `process_chunk` over the whole slab, by the default cranker;
//!   2. many single-slot chunks in *reversed* slot order, by a *different*
//!      signer.
//! Commutativity of histogram folding (clearing-protocol §4.1) means both must
//! produce the identical `ClearingResult` math (every field except the
//! per-market `market` pubkey + bump, which necessarily differ between the two
//! markets).

use solana_sdk::pubkey::Pubkey;
use tempo_integration_tests::*;

/// (side, is_maker, price, qty) for a fixed book exercising both auctions and a
/// rationed marginal tick. Maker rows now flow through the MakerQuote book
/// (submit_order is taker-only, §1.3); takers rest in the slab. The *economic*
/// book is unchanged, so the clearing math is identical.
const BOOK: &[(u8, u8, u64, u64)] = &[
    (SIDE_BUY, 1, 50, 30),  // bid demand, tick 4 (maker)
    (SIDE_SELL, 0, 30, 40), // bid supply, tick 2 (taker)
    (SIDE_BUY, 1, 40, 15),  // bid demand, tick 3 (maker, marginal-ish)
    (SIDE_BUY, 0, 60, 25),  // ask demand, tick 5 (taker)
    (SIDE_SELL, 1, 40, 50), // ask supply, tick 3 (maker)
    (SIDE_SELL, 1, 50, 10), // ask supply, tick 4 (maker)
];

/// Number of taker rows in BOOK (these are the only orders that rest in the slab).
fn taker_count() -> u32 {
    BOOK.iter().filter(|&&(_, m, _, _)| m == 0).count() as u32
}

/// Math-relevant slice of the clearing account: everything between the 2-byte
/// prefix and the `market` pubkey (auction_id + prices + volumes + ticks).
fn clearing_math(c: &ClearingState) -> &[u8] {
    &c.raw[2..2 + (8 * 9 + 4 * 2)]
}

/// Build the book onto `pdas`: maker rows posted via the quote book (distinct
/// keypair each), taker rows submitted into the slab. Returns the maker pubkeys
/// (so the caller can fold each quote before finalize).
fn build_book(ctx: &mut TestContext, pdas: &MarketPdas) -> Vec<Pubkey> {
    let mut makers = Vec::new();
    for &(side, is_maker, price, qty) in BOOK {
        let t = ctx.new_funded_signer();
        if is_maker == 1 {
            ctx.post_maker_order(pdas, &t, side, price, qty);
            makers.push(t.pubkey());
        } else {
            ctx.submit_order(pdas, &t, side, price, qty);
        }
    }
    makers
}

#[test]
fn one_big_chunk_equals_many_reversed_chunks() {
    let mut ctx = TestContext::new();

    // --- Market A: a single big chunk by the default cranker. ---
    let a = ctx.init_market(10, 16, 16);
    let makers_a = build_book(&mut ctx, &a);
    ctx.process_chunk(&a, 0, 16);
    for m in &makers_a {
        ctx.process_maker_quote(&a, m);
    }
    ctx.finalize_clear(&a);
    let cr_a = ctx.clearing(&a).expect("A cleared");

    // --- Market B: same book, single-slot chunks in reversed slot order, by a
    //     different signer than the market authority/payer-cranker. Maker quotes
    //     are folded separately (folding is commutative, so order is irrelevant).
    let b = ctx.init_market(10, 16, 16);
    let makers_b = build_book(&mut ctx, &b);
    let alt_cranker = ctx.new_funded_signer();
    // Only takers rest in the slab; fold their slots one at a time, reversed.
    let slots = taker_count();
    for i in (0..slots).rev() {
        ctx.process_chunk_by(&b, &alt_cranker, i, 1);
    }
    for m in &makers_b {
        ctx.process_maker_quote(&b, m);
    }
    // finalize can be by anyone; use the default cranker here.
    ctx.finalize_clear(&b);
    let cr_b = ctx.clearing(&b).expect("B cleared");

    // The clearing math must be byte-identical regardless of chunking / order /
    // cranker identity.
    assert_eq!(
        clearing_math(&cr_a),
        clearing_math(&cr_b),
        "clearing math must be deterministic"
    );

    // Sanity: both auctions actually crossed (so the equality is meaningful).
    assert!(cr_a.bid_matched_volume > 0, "bid crossed");
    assert!(cr_a.ask_matched_volume > 0, "ask crossed");
    assert_eq!(cr_a.bid_clearing_price, cr_b.bid_clearing_price);
    assert_eq!(cr_a.ask_clearing_price, cr_b.ask_clearing_price);
    assert_eq!(cr_a.bid_matched_volume, cr_b.bid_matched_volume);
    assert_eq!(cr_a.ask_matched_volume, cr_b.ask_matched_volume);
}
