//! Phase 2: drive the reference market maker's pure quoting strategy
//! (`tempo_mm_bot::strategy::build_quote`) against the REAL program in LiteSVM.
//! Proves the strategy's ladder encoding + window-bounds clamp match the
//! on-chain `update_maker_quote_levels` parser and that the posted bid actually
//! folds and fills against a crossing taker — the end-to-end MM → clear path.

use tempo_integration_tests::*;

use tempo_mm_bot::strategy::{build_quote, MmStrategyConfig};
use tempo_sdk::accounts::MarketView;
use tempo_sdk::ix::Level;

/// Levels → the `(offset, size)` pairs the harness encoder takes.
fn pairs(levels: &[Level]) -> Vec<(u16, u64)> {
    levels.iter().map(|l| (l.offset, l.size)).collect()
}

#[test]
fn mm_strategy_quote_folds_and_fills_against_taker() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500; // margin-enabled money path
    let mint = ctx.create_mint();
    ctx.market_collateral_mint = Some(mint);
    let pdas = ctx.init_market(1, 32, 8);

    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let maker = ctx.new_funded_signer();
    let taker = ctx.new_funded_signer();
    for t in [&maker, &taker] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 10_000);
        ctx.deposit(t, &vault_ta, &ta, 10_000);
        ctx.init_position(&pdas, t);
    }
    let cranker = ctx.new_funded_signer();
    ctx.init_collateral(&cranker);

    // The reference strategy builds the ladder from the real market view. A
    // single rung keeps the assertion exact; the window-bounds + encoding are
    // what we're proving carry across to the on-chain parser.
    let market =
        MarketView::decode(&ctx.raw_account(&pdas.market).expect("market")).expect("decode");
    let cfg = MmStrategyConfig {
        levels: 1,
        inner_spread_ticks: 1,
        tick_step: 1,
        base_size: 10,
        size_growth_num: 1,
        size_growth_den: 1,
        max_inventory: 1_000_000,
        skew_ticks_max: 0,
    };
    let quote = build_quote(&market, None, 1_000_000, &cfg).expect("a two-sided quote");
    assert_eq!(quote.mid_tick, 16, "num_ticks 32 → mid 16");
    let bid = quote.bids[0];
    let bid_tick = quote.mid_tick - bid.offset as u32;
    let bid_price = market.window_floor_price + bid_tick as u64 * market.tick_size;

    // Post the strategy's ladder exactly as the bot would, then a taker sells
    // into the bid so the bid auction (maker-buys vs taker-sells) crosses.
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    ctx.update_maker_quote_levels(
        &pdas,
        &maker,
        1,
        quote.mid_tick,
        &pairs(&quote.bids),
        &pairs(&quote.asks),
    );
    let sell = ctx.submit_order(&pdas, &taker, SIDE_SELL, bid_price, bid.size);

    // Crank the round to completion.
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    ctx.finalize_clear_with_fee(&pdas, &cranker);
    ctx.settle_fill_with_margin(&pdas, sell, &taker.pubkey());
    ctx.settle_maker_quote(&pdas, &maker.pubkey());

    // The maker bought exactly its quoted bid size; the taker is short the same.
    let (mpos, _) = ctx.position_pda(&pdas, &maker.pubkey());
    let (tpos, _) = ctx.position_pda(&pdas, &taker.pubkey());
    let mp = ctx.position(&mpos);
    let tp = ctx.position(&tpos);
    assert_eq!(mp.size, bid.size as i64, "maker filled its quoted bid");
    assert_eq!(mp.entry_price, bid_price);
    assert_eq!(tp.size, -(bid.size as i64), "taker is short the same");

    // Conservation: nothing minted or lost.
    let mb = ctx.user_collateral(&maker.pubkey()).balance;
    let tb = ctx.user_collateral(&taker.pubkey()).balance;
    let cb = ctx.user_collateral(&cranker.pubkey()).balance;
    let ins = ctx.vault().insurance_balance;
    assert_eq!(mb + tb + cb + ins, ctx.token_balance(&vault_ta));
    assert_eq!(mb + tb + cb + ins, 20_000);
}
