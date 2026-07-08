//! `update_funding` advances the market funding index.
//!
//! Binds the market to a crafted Pyth oracle, runs one clearing round so the
//! market records non-zero last-fill prices (these feed the mark), then sets the
//! oracle *below* the cleared mark so the mark sits above the index → longs pay
//! → the monotonic funding index advances upward.

use tempo_integration_tests::*;

#[test]
fn update_funding_moves_index() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();

    // A realistic wall-clock so funding accrual has a non-zero elapsed window
    // (market.last_funding_ts starts at 0) and the crafted oracle reads fresh.
    ctx.set_clock_ts(1_700_000_000);

    let pdas = ctx.init_market_with_oracle(10, 16, 64, oracle);

    // Run a clearing round at price 40 so the market records last_*_fill_price.
    let buyer = ctx.new_funded_signer();
    let seller = ctx.new_funded_signer();
    // Maker-buy comes from the quote book (submit_order is taker-only §1.3); the
    // taker-sell crosses it in the bid auction.
    ctx.post_maker_order(&pdas, &buyer, SIDE_BUY, 40, 50);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 40, 50);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &buyer.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote_clearing(&pdas, &buyer.pubkey());
    let _ = ctx.settle_fill(&pdas, sell_id);

    // Before: index is the genesis 0 and ts is 0.
    let (idx0, ts0) = ctx.market_funding(&pdas);
    assert_eq!(idx0, 0, "fresh market funding index is 0");
    assert_eq!(ts0, 0, "fresh market last_funding_ts is 0");

    // Oracle below the cleared mark: price 30 (expo -8 → price_1e8 = 30). The
    // mark (cleared at 40) clamps to the +5% band edge 31 > 30 → positive gap →
    // funding index moves up.
    ctx.set_oracle(&oracle, 30, -8);

    ctx.update_funding(&pdas, &oracle);

    let (idx1, ts1) = ctx.market_funding(&pdas);
    assert!(
        idx1 > idx0,
        "funding index advanced upward (mark>oracle), got {idx1}"
    );
    assert_eq!(
        ts1, 1_700_000_000,
        "last_funding_ts set to the clock timestamp"
    );
}

/// M7 — `update_funding` halts when the oracle confidence interval is too wide.
#[test]
fn update_funding_halts_on_wide_oracle_confidence() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    let pdas = ctx.init_market_with_oracle(10, 16, 64, oracle);

    // Run a round so the market records last-fill prices (feeds the mark).
    let buyer = ctx.new_funded_signer();
    let seller = ctx.new_funded_signer();
    ctx.post_maker_order(&pdas, &buyer, SIDE_BUY, 40, 50);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 40, 50);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &buyer.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote_clearing(&pdas, &buyer.pubkey());
    let _ = ctx.settle_fill(&pdas, sell_id);

    // Oracle price 30 with a confidence of 3 raw units → 3/30 = 1000 bps > 500 →
    // the funding update must be rejected (the index is left untouched).
    ctx.set_oracle_with_conf(&oracle, 30, -8, 3);
    assert!(
        ctx.try_update_funding(&pdas, &oracle).is_err(),
        "wide-confidence oracle must halt funding"
    );
    let (idx, _) = ctx.market_funding(&pdas);
    assert_eq!(idx, 0, "funding index unchanged after a halted update");

    // A confident print (conf 0) at the same price updates normally.
    ctx.set_oracle(&oracle, 30, -8);
    ctx.update_funding(&pdas, &oracle);
}

/// A freshly created position seeds its funding checkpoint from the
/// market's live funding index, not 0.
#[test]
fn init_position_seeds_live_funding_index() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    let pdas = ctx.init_market_with_oracle(10, 16, 64, oracle);

    // Run a round, then advance the funding index above 0.
    let buyer = ctx.new_funded_signer();
    let seller = ctx.new_funded_signer();
    ctx.post_maker_order(&pdas, &buyer, SIDE_BUY, 40, 50);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 40, 50);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &buyer.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote_clearing(&pdas, &buyer.pubkey());
    let _ = ctx.settle_fill(&pdas, sell_id);
    ctx.set_oracle(&oracle, 30, -8);
    ctx.update_funding(&pdas, &oracle);

    let (idx, _) = ctx.market_funding(&pdas);
    assert!(idx > 0, "market funding index advanced above 0");

    // A brand-new owner's position must open at the live index, not 0.
    let newcomer = ctx.new_funded_signer();
    let position = ctx.init_position(&pdas, &newcomer);
    assert_eq!(
        ctx.position(&position).last_funding_index,
        idx,
        "new position seeds last_funding_index from the market",
    );
}

/// The effective-price meltdown brake walks toward the oracle by at most
/// the per-slot cap, instead of jumping to a spike in one step.
#[test]
fn effective_price_brake_walks_by_cap() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_max_price_move_bps = 500; // 5% per slot
    let pdas = ctx.init_market_with_oracle(1, 200, 8, oracle);

    // Bootstrap: a cold-start effective price jumps to the first fresh oracle.
    ctx.set_oracle(&oracle, 100, -8);
    ctx.update_funding(&pdas, &oracle);
    let m = ctx.market(&pdas);
    assert_eq!(m.effective_price_1e8, 100, "bootstrapped to oracle");
    let base_slot = m.last_good_oracle_slot;

    // Crash the oracle to 50; after 1 slot the effective price may only fall 5%
    // of 100 = 5 → 95 (the spike is not recognised all at once).
    ctx.set_oracle(&oracle, 50, -8);
    ctx.warp_slot(base_slot + 1);
    ctx.update_funding(&pdas, &oracle);
    assert_eq!(
        ctx.market(&pdas).effective_price_1e8,
        95,
        "1-slot move capped at 5"
    );

    // After enough slots the effective price catches up to the oracle.
    ctx.warp_slot(base_slot + 100);
    ctx.update_funding(&pdas, &oracle);
    assert_eq!(
        ctx.market(&pdas).effective_price_1e8,
        50,
        "effective price catches up over many slots"
    );
}

/// P5.4 (missing-features §5.1): the funding rate's INDEX side is the oracle
/// EMA, not spot. With mark == spot the spot-priced gap is zero, so the index
/// only moves if the program reads the divergent EMA. Solvency's use of raw
/// spot is pinned separately (`oracle::test_solvency_mark_prefers_fresh_raw_oracle`
/// + the divergent-ema reader unit tests in both the program and tempo-math).
#[test]
fn funding_rate_prices_off_the_ema_not_spot() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    let pdas = ctx.init_market_with_oracle(10, 16, 64, oracle);

    // A cleared round at 40 records the last-fill prices that feed the mark.
    let buyer = ctx.new_funded_signer();
    let seller = ctx.new_funded_signer();
    ctx.post_maker_order(&pdas, &buyer, SIDE_BUY, 40, 50);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 40, 50);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &buyer.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote_clearing(&pdas, &buyer.pubkey());
    let _ = ctx.settle_fill(&pdas, sell_id);

    // Control: spot 40, NO ema (fallback → index side = spot 40 = mark 40 →
    // zero gap → the index does not move).
    ctx.set_oracle_with_ema(&oracle, 40, -8, 0);
    ctx.update_funding(&pdas, &oracle);
    let (idx_control, _) = ctx.market_funding(&pdas);
    assert_eq!(idx_control, 0, "spot == mark → zero rate → index unmoved");

    // Divergent EMA: spot stays 40 (mark 40, band centered on spot), EMA 38.
    // If funding still priced off spot the gap would remain zero; reading the
    // EMA makes the gap (40 − 38) > 0 → the index must advance.
    ctx.set_clock_ts(1_700_003_600); // one full funding interval later
    ctx.set_oracle_with_ema(&oracle, 40, -8, 38);
    ctx.update_funding(&pdas, &oracle);
    let (idx_ema, _) = ctx.market_funding(&pdas);
    assert!(
        idx_ema > 0,
        "divergent EMA moved the funding index (spot alone could not), got {idx_ema}"
    );
}
