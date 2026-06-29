//! Plan task 1.3.5 — two consecutive auctions on a single market.
//!
//! Run the full Collect -> Accumulate -> Discover -> Settle loop, then
//! `start_auction` to roll into the next round and run the loop again. This
//! proves the slot/bucket zeroing works: a second round on the same accounts
//! clears correctly, the auction id increments, and round-2 state does not leak
//! from round 1.

use solana_sdk::signature::Keypair;
use tempo_integration_tests::*;

/// Drive one full round on `pdas` with a simple crossing book, settling every
/// order so the round ends fully consumed (a precondition for `start_auction`).
/// Returns the round's clearing result and the two maker keypairs (whose quotes
/// persist across rounds and must be retired before the next round opens).
fn run_round(
    ctx: &mut TestContext,
    pdas: &MarketPdas,
    bid_qty: u64,
    ask_qty: u64,
) -> (ClearingState, Keypair, Keypair) {
    // Bid auction: maker-buy (quote book) vs taker-sell at the same tick
    // (price 40 -> tick 3). submit_order is taker-only (§1.3), so maker
    // liquidity comes from the MakerQuote book.
    let mb = ctx.new_funded_signer();
    let ts = ctx.new_funded_signer();
    ctx.post_maker_order(pdas, &mb, SIDE_BUY, 40, bid_qty);
    ctx.submit_order(pdas, &ts, SIDE_SELL, 40, bid_qty);
    // Ask auction: taker-buy vs maker-sell (quote book) at price 50 -> tick 4.
    let tb = ctx.new_funded_signer();
    let ms = ctx.new_funded_signer();
    ctx.submit_order(pdas, &tb, SIDE_BUY, 50, ask_qty);
    ctx.post_maker_order(pdas, &ms, SIDE_SELL, 50, ask_qty);

    ctx.process_chunk(pdas, 0, 64);
    ctx.process_maker_quote(pdas, &mb.pubkey());
    ctx.process_maker_quote(pdas, &ms.pubkey());
    ctx.finalize_clear(pdas);
    let cr = ctx.clearing(pdas).expect("round cleared");

    // Settle every taker order (slab) plus both maker quotes so the slab count
    // returns to zero (a precondition for start_auction).
    let ids: Vec<u64> = ctx.orders(pdas).iter().map(|o| o.order_id).collect();
    for id in ids {
        ctx.settle_fill(pdas, id);
    }
    ctx.settle_maker_quote_clearing(pdas, &mb.pubkey());
    ctx.settle_maker_quote_clearing(pdas, &ms.pubkey());
    assert_eq!(ctx.order_slab(pdas).count, 0, "all taker orders settled");
    (cr, mb, ms)
}

#[test]
fn two_consecutive_auctions_on_one_market() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    assert_eq!(ctx.market(&pdas).current_auction_id, 0);

    // --- Round 0 ---
    let (cr0, mb0, ms0) = run_round(&mut ctx, &pdas, 20, 30);
    assert_eq!(cr0.auction_id, 0);
    assert_eq!(cr0.bid_matched_volume, 20);
    assert_eq!(cr0.ask_matched_volume, 30);
    assert_eq!(ctx.market(&pdas).phase, PHASE_SETTLING);

    // --- Roll into the next round. ---
    ctx.start_auction(&pdas);
    // Maker quotes persist across rounds; retire round-0's two quotes now that
    // Collect has reopened (clear_maker_quote is Collect-only, strictly-increasing
    // sequence — post_maker_order used sequence 1, so clear at 2). This drops the
    // active maker-quote count back to 0 so round 1 starts from a clean book.
    ctx.clear_maker_quote(&pdas, &mb0, 2);
    ctx.clear_maker_quote(&pdas, &ms0, 2);
    let m = ctx.market(&pdas);
    assert_eq!(m.current_auction_id, 1, "auction id incremented");
    assert_eq!(m.phase, PHASE_COLLECT, "reopened for collection");
    // Histogram + slab were re-stamped with the new round and emptied; their counts
    // are the authoritative reset (PERF-1 removed the market mirrors).
    assert_eq!(ctx.histogram(&pdas).auction_id, 1);
    assert_eq!(ctx.histogram(&pdas).accumulated_count, 0, "counters reset");
    assert_eq!(ctx.order_slab(&pdas).auction_id, 1);
    assert_eq!(ctx.order_slab(&pdas).count, 0);
    assert_eq!(ctx.orders(&pdas).len(), 0, "no orders leak from round 0");

    // --- Round 1 with different volumes, proving buckets/slab were zeroed. ---
    // Bid auction: maker-buy (quote book) vs taker-sell @ price 40 (tick 3), qty 15.
    let mb = ctx.new_funded_signer();
    let ts = ctx.new_funded_signer();
    ctx.post_maker_order(&pdas, &mb, SIDE_BUY, 40, 15);
    ctx.submit_order(&pdas, &ts, SIDE_SELL, 40, 15);
    // Ask auction: taker-buy vs maker-sell (quote book) @ price 50 (tick 4), qty 25.
    let tb = ctx.new_funded_signer();
    let ms = ctx.new_funded_signer();
    ctx.submit_order(&pdas, &tb, SIDE_BUY, 50, 25);
    ctx.post_maker_order(&pdas, &ms, SIDE_SELL, 50, 25);

    // Only the two takers rest in the slab now (makers live in the quote book),
    // so the slab live count is 2, not 4.
    assert_eq!(
        ctx.order_slab(&pdas).count,
        2,
        "only round-1 taker orders are active"
    );
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &mb.pubkey());
    ctx.process_maker_quote(&pdas, &ms.pubkey());
    assert_eq!(
        ctx.histogram(&pdas).accumulated_count,
        2,
        "round-1 accumulates only its own 2 taker orders"
    );
    // The histogram carries the new round id and only round-1's taker
    // accumulation — proof that StartAuction zeroed both the buckets and the slab.
    assert_eq!(ctx.histogram(&pdas).auction_id, 1);
    assert_eq!(ctx.histogram(&pdas).accumulated_count, 2);

    // Round 1 finalizes by *reusing* the persistent ClearingResult PDA
    // (finalize_clear uses idempotent creation), overwriting round 0's result
    // with round 1's. This is the round-reuse model the docs describe.
    ctx.finalize_clear(&pdas);
    let cr1 = ctx.clearing(&pdas).expect("round 1 cleared");
    assert_eq!(cr1.auction_id, 1, "result re-stamped with the new round");
    assert_eq!(
        cr1.bid_matched_volume, 15,
        "round-1 bid volume, not round-0's 20"
    );
    assert_eq!(
        cr1.ask_matched_volume, 25,
        "round-1 ask volume, not round-0's 30"
    );

    // Settle round 1 fully (takers + both maker quotes), proving the whole loop
    // runs a second time.
    let ids: Vec<u64> = ctx.orders(&pdas).iter().map(|o| o.order_id).collect();
    for id in ids {
        ctx.settle_fill(&pdas, id);
    }
    ctx.settle_maker_quote_clearing(&pdas, &mb.pubkey());
    ctx.settle_maker_quote_clearing(&pdas, &ms.pubkey());
    assert_eq!(ctx.order_slab(&pdas).count, 0, "round 1 fully settled");
}
