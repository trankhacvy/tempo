//! Plan task 2.3 — liveness.
//!
//! A crank stops mid-accumulation; a *different* signer resumes `process_chunk`
//! and completes the round. The clearing result must equal the one produced by
//! a single uninterrupted crank on an identical book. The liveness failure mode
//! is delay, not loss: any honest party can finish the round.

use solana_sdk::pubkey::Pubkey;
use tempo_integration_tests::*;

// Maker rows (is_maker==1) now flow through the MakerQuote book; takers rest in
// the slab. The economic book is unchanged, so the clearing math is identical.
const BOOK: &[(u8, u8, u64, u64)] = &[
    (SIDE_BUY, 1, 50, 20),  // maker
    (SIDE_SELL, 0, 30, 20), // taker
    (SIDE_BUY, 0, 60, 12),  // taker
    (SIDE_SELL, 1, 40, 12), // maker
    (SIDE_BUY, 1, 40, 8),   // maker
    (SIDE_SELL, 0, 40, 5),  // taker
];

/// Number of taker rows (the only orders that rest in the slab and are folded by
/// process_chunk; maker quotes are folded separately).
fn taker_count() -> u64 {
    BOOK.iter().filter(|&&(_, m, _, _)| m == 0).count() as u64
}

fn clearing_math(c: &ClearingState) -> &[u8] {
    &c.raw[2..2 + (8 * 9 + 4 * 2)]
}

/// Build the book: makers via the quote book (distinct keypair each), takers into
/// the slab. Returns the maker pubkeys so the caller can fold each before finalize.
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
fn interrupted_crank_resumed_by_other_signer_matches_uninterrupted() {
    let mut ctx = TestContext::new();

    // --- Reference: one uninterrupted crank. ---
    let r = ctx.init_market(10, 16, 16);
    let makers_r = build_book(&mut ctx, &r);
    ctx.process_chunk(&r, 0, 16);
    for mk in &makers_r {
        ctx.process_maker_quote(&r, mk);
    }
    ctx.finalize_clear(&r);
    let cr_ref = ctx.clearing(&r).expect("reference cleared");

    // --- Interrupted: first cranker does part, stops; second cranker resumes. ---
    let m = ctx.init_market(10, 16, 16);
    let makers_m = build_book(&mut ctx, &m);

    let cranker1 = ctx.new_funded_signer();
    let cranker2 = ctx.new_funded_signer();

    // cranker1 folds the first slab slot, then "stops" (does nothing more). Only
    // takers rest in the slab; active_order_count counts those.
    ctx.process_chunk_by(&m, &cranker1, 0, 1);
    let mid = ctx.market(&m);
    assert_eq!(mid.phase, PHASE_ACCUMULATING);
    assert_eq!(mid.accumulated_order_count, 1, "partial progress");
    assert!(
        mid.accumulated_order_count < mid.active_order_count,
        "round not complete"
    );

    // A finalize attempt here must fail (incomplete) — proves the round is stuck
    // until someone resumes.
    assert!(
        ctx.try_finalize_clear(&m).is_err(),
        "cannot finalize a stuck round"
    );

    // cranker2 (a *different* signer) resumes and finishes the slab, then the
    // maker quotes are folded (finalize blocks until every active quote is folded).
    ctx.process_chunk_by(&m, &cranker2, 1, 15);
    assert_eq!(ctx.market(&m).accumulated_order_count, taker_count());
    for mk in &makers_m {
        ctx.process_maker_quote(&m, mk);
    }

    ctx.finalize_clear(&m);
    let cr_resumed = ctx.clearing(&m).expect("resumed round cleared");

    assert_eq!(
        clearing_math(&cr_ref),
        clearing_math(&cr_resumed),
        "resumed-after-interruption result must equal the uninterrupted result"
    );
}
