//! Oracle-anchored tick window (known-issues §2.7): the histogram price window is
//! re-snapped onto the market's oracle at `initialize_market` and at every
//! `start_auction`, so it tracks the live price instead of staying pinned at
//! genesis. A stale/missing oracle carries the previous window forward (the path
//! every dummy-oracle test already exercises); here we drive a *fresh* Pyth oracle
//! and assert the window actually moves.

use solana_sdk::pubkey::Pubkey;
use tempo_integration_tests::*;

/// The price at tick 0 = the `window_floor_price` field (§2.7). It sits 18 bytes
/// before the account end now that the v8 pre-trade risk config (`initial_margin_bps`
/// 2 + `max_position_notional` 16) is appended after it (missing-features §1.2).
fn window_floor(ctx: &TestContext, pdas: &MarketPdas) -> u64 {
    let raw = ctx.account_raw(&pdas.market);
    let n = raw.len();
    u64::from_le_bytes(raw[n - 26..n - 18].try_into().unwrap())
}

#[test]
fn init_market_centers_window_on_oracle() {
    let mut ctx = TestContext::new();
    let oracle = Pubkey::new_unique();
    // price_1e8 = 100_000 (raw 100_000, exponent -8). tick_size 10, num_ticks 64.
    ctx.set_oracle(&oracle, 100_000, -8);
    let pdas = ctx.init_market_with_oracle(10, 64, 16, oracle);

    // floor = oracle - (num_ticks/2)*tick_size = 100_000 - 320 = 99_680.
    assert_eq!(window_floor(&ctx, &pdas), 99_680);

    // An order at the centered price is accepted; a genesis-era price (10) is now
    // below the window floor and rejected — proving the window moved off genesis.
    let trader = ctx.new_funded_signer();
    let _ = ctx.submit_order(&pdas, &trader, SIDE_BUY, 100_000, 3);
    assert!(
        ctx.try_submit_order(&pdas, &trader, SIDE_BUY, 10, 3)
            .is_err(),
        "a price below the recentered window floor must be rejected"
    );
}

#[test]
fn dummy_oracle_keeps_genesis_window() {
    // init_market uses a throwaway (non-Pyth) oracle → recenter is skipped and the
    // genesis floor (tick_size) is kept (the carry-forward branch).
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    assert_eq!(window_floor(&ctx, &pdas), 10, "genesis floor == tick_size");
}

#[test]
fn start_auction_resnaps_window_on_oracle() {
    let mut ctx = TestContext::new();
    let oracle = Pubkey::new_unique();
    ctx.set_oracle(&oracle, 100_000, -8);
    let pdas = ctx.init_market_with_oracle(10, 64, 16, oracle);
    assert_eq!(window_floor(&ctx, &pdas), 99_680);

    // Roll an empty round to Discovered (no orders → never reaches Settling).
    let deadline = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(deadline);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    assert_eq!(ctx.market(&pdas).phase, PHASE_DISCOVERED);

    // The oracle moves up before the next round opens.
    ctx.set_oracle(&oracle, 200_000, -8);
    ctx.start_auction(&pdas);
    assert_eq!(ctx.market(&pdas).phase, PHASE_COLLECT);

    // New floor = 200_000 - 320 = 199_680: the window followed the oracle.
    assert_eq!(window_floor(&ctx, &pdas), 199_680);
}

#[test]
fn start_auction_carries_window_forward_when_oracle_stale() {
    // A market whose oracle account is non-Pyth (dummy) keeps its genesis window
    // across a round roll — the recenter is skipped, never blocking the roll.
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    assert_eq!(window_floor(&ctx, &pdas), 10);

    let deadline = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(deadline);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.finalize_clear(&pdas);
    ctx.start_auction(&pdas);

    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_COLLECT,
        "roll still succeeds"
    );
    assert_eq!(window_floor(&ctx, &pdas), 10, "window carried forward");
}

#[test]
fn cancel_order_slot_hint_o1_and_fallback_agree() {
    // The validated slot hint (known-issues §2.7): the correct slot takes the O(1)
    // path; a wrong/out-of-range hint falls back to the scan. Both must succeed and
    // remove the order — the hint is an optimization, never a trust input.
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 64, 16);
    let trader = ctx.new_funded_signer();

    // First order lands in slot 0; cancel it via the correct hint (O(1) path).
    ctx.submit_order(&pdas, &trader, SIDE_BUY, 40, 5);
    ctx.cancel_order_hinted(&pdas, &trader, 0, 0);
    assert_eq!(ctx.order_slab(&pdas).count, 0, "hinted cancel removed it");

    // A second order; cancel it with a deliberately wrong hint → scan fallback.
    ctx.submit_order(&pdas, &trader, SIDE_BUY, 40, 5);
    let id = ctx.orders(&pdas)[0].order_id;
    ctx.cancel_order_hinted(&pdas, &trader, id, 999);
    assert_eq!(ctx.order_slab(&pdas).count, 0, "fallback cancel removed it");
}
