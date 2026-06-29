//! End-to-end happy path: init a market, submit a mix of maker/taker buy+sell
//! orders that feed both the bid and the ask auction, accumulate the whole
//! slab, finalize, settle every order, and verify the conservation invariants
//! (Σ fills per auction <= matched volume, residual <= dust) plus the phase
//! transitions Collect -> Accumulating -> Discovered -> Settling.
//!
//! Maker liquidity now comes from the on-chain MakerQuote book (§1.3):
//! `submit_order` is taker-only. So the maker-buy and maker-sell legs are posted
//! via `post_maker_order` / folded with `process_maker_quote` / settled with
//! `settle_maker_quote_clearing`; only the taker legs live in the order slab.

use tempo_integration_tests::*;

/// Per-auction tally of demand-side and supply-side fills. Each side is matched
/// against the *single* crossing volume V, so each side independently must
/// satisfy `Σ side fills <= V` (summing both sides would double-count V).
#[derive(Default)]
struct AuctionTally {
    demand_fills: u64,
    supply_fills: u64,
    residual: u64,
}

#[test]
fn happy_path_full_loop() {
    let mut ctx = TestContext::new();
    let tick = 10u64;
    let pdas = ctx.init_market(tick, 16, 64);

    assert_eq!(ctx.market(&pdas).phase, PHASE_COLLECT, "starts in Collect");

    // Traders.
    let mb = ctx.new_funded_signer(); // maker buyer  -> bid demand (quote book)
    let ts = ctx.new_funded_signer(); // taker seller -> bid supply (slab)
    let tb = ctx.new_funded_signer(); // taker buyer  -> ask demand (slab)
    let ms = ctx.new_funded_signer(); // maker seller -> ask supply (quote book)

    // Makers need a Position (no-margin market → no collateral needed).
    ctx.init_position(&pdas, &mb);
    ctx.init_position(&pdas, &ms);

    // Bid auction: maker-buy 20 (quote) vs taker-sell 20 (slab), both at price 40
    // (tick 3) so they cross exactly at a populated marginal bucket and fill fully.
    ctx.post_maker_order(&pdas, &mb, SIDE_BUY, 4 * tick, 20);
    let o_ts = ctx.submit_order(&pdas, &ts, SIDE_SELL, 4 * tick, 20);
    // Ask auction: taker-buy 30 (slab) vs maker-sell 30 (quote), both at price 50.
    let o_tb = ctx.submit_order(&pdas, &tb, SIDE_BUY, 5 * tick, 30);
    ctx.post_maker_order(&pdas, &ms, SIDE_SELL, 5 * tick, 30);

    // Only the two takers rest in the slab now; makers live in the quote book.
    // (PERF-1: the slab header count is the authoritative live-order count.)
    assert_eq!(ctx.order_slab(&pdas).count, 2);

    // Phase 1 ACCUMULATE — fold the slab (takers) then the maker quotes.
    ctx.process_chunk(&pdas, 0, 64);
    let m = ctx.market(&pdas);
    assert_eq!(
        m.phase, PHASE_ACCUMULATING,
        "first chunk transitions to Accumulating"
    );
    assert_eq!(
        ctx.histogram(&pdas).accumulated_count,
        2,
        "two takers accumulated"
    );
    ctx.process_maker_quote(&pdas, &mb.pubkey());
    ctx.process_maker_quote(&pdas, &ms.pubkey());

    // Phase 2 DISCOVER.
    ctx.finalize_clear(&pdas);
    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_DISCOVERED,
        "finalize -> Discovered"
    );
    let cr = ctx.clearing(&pdas).expect("clearing result published");

    // Both auctions crossed (matched volume == 20 / 30 resp.).
    assert_eq!(cr.bid_matched_volume, 20, "bid auction matched 20");
    assert_eq!(cr.ask_matched_volume, 30, "ask auction matched 30");

    // Phase 3 SETTLE — settle every order/quote, accumulating fills per auction.
    let mut bid = AuctionTally::default();
    let mut ask = AuctionTally::default();

    // Taker-sell -> bid supply.
    {
        let (_, fill) = ctx.settle_fill(&pdas, o_ts);
        bid.supply_fills += fill;
        bid.residual += 20 - fill;
    }
    // Taker-buy -> ask demand.
    {
        let (_, fill) = ctx.settle_fill(&pdas, o_tb);
        ask.demand_fills += fill;
        ask.residual += 30 - fill;
    }
    // Maker-buy -> bid demand (read the fill back off the maker's position size).
    {
        ctx.settle_maker_quote_clearing(&pdas, &mb.pubkey());
        let (mbpos, _) = ctx.position_pda(&pdas, &mb.pubkey());
        let fill = ctx.position(&mbpos).size as u64; // long
        bid.demand_fills += fill;
        bid.residual += 20 - fill;
    }
    // Maker-sell -> ask supply.
    {
        ctx.settle_maker_quote_clearing(&pdas, &ms.pubkey());
        let (mspos, _) = ctx.position_pda(&pdas, &ms.pubkey());
        let fill = (-ctx.position(&mspos).size) as u64; // short
        ask.supply_fills += fill;
        ask.residual += 30 - fill;
    }
    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_SETTLING,
        "settling phase reached"
    );

    // Conservation: fills on each *side* of each auction never exceed that
    // auction's single matched volume V (rationing rounds against the user).
    assert!(
        bid.demand_fills <= cr.bid_matched_volume,
        "bid demand {} <= {}",
        bid.demand_fills,
        cr.bid_matched_volume
    );
    assert!(
        bid.supply_fills <= cr.bid_matched_volume,
        "bid supply {} <= {}",
        bid.supply_fills,
        cr.bid_matched_volume
    );
    assert!(
        ask.demand_fills <= cr.ask_matched_volume,
        "ask demand {} <= {}",
        ask.demand_fills,
        cr.ask_matched_volume
    );
    assert!(
        ask.supply_fills <= cr.ask_matched_volume,
        "ask supply {} <= {}",
        ask.supply_fills,
        cr.ask_matched_volume
    );

    // This clean book crosses exactly at a populated marginal bucket on both
    // auctions, so each side fills the full matched volume (no dust).
    assert_eq!(
        bid.demand_fills, cr.bid_matched_volume,
        "bid demand fully matched"
    );
    assert_eq!(
        bid.supply_fills, cr.bid_matched_volume,
        "bid supply fully matched"
    );
    assert_eq!(
        ask.demand_fills, cr.ask_matched_volume,
        "ask demand fully matched"
    );
    assert_eq!(
        ask.supply_fills, cr.ask_matched_volume,
        "ask supply fully matched"
    );

    // Residual (unfilled remainder) is dust-bounded: balanced books fill cleanly.
    assert!(bid.residual <= 2, "bid residual dust {}", bid.residual);
    assert!(ask.residual <= 2, "ask residual dust {}", ask.residual);

    // Every taker order is now consumed.
    for o in ctx.orders(&pdas) {
        assert_eq!(o.status, STATUS_CONSUMED, "order {} consumed", o.order_id);
    }
}
