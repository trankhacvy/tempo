//! Known-issues §2.13 — the Stage-B **marketable-fill** half of DDR-3, end to end.
//!
//! The passive-park half (window moves AWAY → order parks, exempt from
//! completeness) is covered in `resting_orders.rs`. What was missing is the
//! *fill* half: when the oracle-anchored window recenters **through** a resting
//! order's fixed price, `classify_resting_fold` folds it at the boundary tick as
//! *marketable* — and it must then actually EXECUTE against a live counterparty
//! at the uniform clearing price, with exact margin/position/OI accounting.
//!
//! Both directions are pinned here on a full money-path market:
//!  * a resting SELL the window moved UP past (price < new floor) folds at tick 0
//!    and fills against a live maker BID — at a clearing price ≥ its limit;
//!  * a resting BUY the window moved DOWN past (price > new top) folds at the top
//!    tick and fills against a live maker ASK — at a clearing price ≤ its limit.

use solana_sdk::pubkey::Pubkey;
use tempo_integration_tests::*;

const STATUS_CONSUMED: u8 = 3;

fn position_key(pdas: &MarketPdas, owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"position", pdas.market.as_ref(), owner.as_ref()],
        &TEMPO_PROGRAM_ID,
    )
    .0
}

/// Full money-path setup: vault + two funded participants (collateral ledger,
/// token account, deposit, position). Returns (seller/buyer keypair, maker keypair).
fn money_setup(
    ctx: &mut TestContext,
    pdas: &MarketPdas,
) -> (solana_sdk::signature::Keypair, solana_sdk::signature::Keypair) {
    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let trader = ctx.new_funded_signer();
    ctx.init_collateral(&trader);
    let trader_ta = ctx.create_token_account(&mint, &trader.pubkey());
    ctx.mint_to(&mint, &trader_ta, 1_000_000_000);
    ctx.deposit(&trader, &vault_ta, &trader_ta, 1_000_000_000);
    ctx.init_position(pdas, &trader);

    let maker = ctx.new_funded_signer();
    ctx.init_collateral(&maker);
    let maker_ta = ctx.create_token_account(&mint, &maker.pubkey());
    ctx.mint_to(&mint, &maker_ta, 1_000_000_000);
    ctx.deposit(&maker, &vault_ta, &maker_ta, 1_000_000_000);
    ctx.init_position(pdas, &maker);

    (trader, maker)
}

/// Run one full empty-ish round so a lone resting order zero-fills and re-arms.
fn run_zero_fill_round(ctx: &mut TestContext, pdas: &MarketPdas, order: u64, owner: &Pubkey) {
    let d = ctx.phase_deadline_slot(pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(pdas, 0, 64);
    ctx.finalize_clear(pdas);
    let (_m, fill) = ctx.settle_fill_with_margin(pdas, order, owner);
    assert_eq!(fill, 0, "no counterparty in round 1 → zero fill, re-rests");
}

/// A resting SELL at 100_000 is gapped by an UP-recenter (new floor 199_680 >
/// its price) → marketable, folds at tick 0, and fills a live maker BID at the
/// uniform clearing price (the window floor — better than its limit).
#[test]
fn recentered_window_fills_marketable_resting_sell_against_live_buy() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500; // money market (initial margin falls back to 500)
    let oracle = Pubkey::new_unique();
    // Window centers on 100_000: floor 99_680, top 100_310 (tick 10, 64 ticks).
    ctx.set_oracle(&oracle, 100_000, -8);
    let pdas = ctx.init_market_with_oracle(10, 64, 16, oracle);
    let (seller, maker) = money_setup(&mut ctx, &pdas);

    // In-window resting SELL @ 100_000, qty 5, GTC. Reserves worst-case margin.
    let o = ctx.submit_order(&pdas, &seller, SIDE_SELL, 100_000, 5);
    assert!(
        ctx.user_collateral(&seller.pubkey()).locked > 0,
        "the resting sell locked worst-case margin at submit"
    );

    // Round 1: no counterparty → zero fill → re-armed Resting.
    run_zero_fill_round(&mut ctx, &pdas, o, &seller.pubkey());

    // The market moves THROUGH the sell: recenter to 200_000 → new window
    // [199_680, 200_310]. The sell's fixed 100_000 is below the floor →
    // Marketable(tick 0) per classify_resting_fold (DDR-3).
    ctx.set_oracle(&oracle, 200_000, -8);
    ctx.start_auction(&pdas);

    // Live counterparty: a maker BID at 200_000 (mid_tick 32 on the new window).
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 32, &[(0, 5)], &[]);

    // Round 2: fold (the sell folds at the boundary tick 0), clear, settle.
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    ctx.finalize_clear(&pdas);

    let cr = ctx.clearing(&pdas).unwrap();
    assert_eq!(cr.bid_matched_volume, 5, "bid auction crossed the full 5");
    // Clearing tick is the boundary tick 0 → price = the new window floor, which
    // is ≥ the sell's limit (it sells BETTER than it asked — the market moved
    // through it, never below its limit).
    assert_eq!(cr.bid_clearing_price, 199_680, "cleared at the window floor");
    assert!(
        cr.bid_clearing_price >= 100_000,
        "a marketable sell never fills below its limit"
    );

    let (_m, fill) = ctx.settle_fill_with_margin(&pdas, o, &seller.pubkey());
    assert_eq!(fill, 5, "the marketable sell filled fully");
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.status, STATUS_CONSUMED, "fully filled → leaves the book");
    assert_eq!(rec.remaining, 0);

    ctx.settle_maker_quote(&pdas, &maker.pubkey());

    // Positions: both sides booked at the SAME uniform price; OI conserves.
    let sp = ctx.position(&position_key(&pdas, &seller.pubkey()));
    let mp = ctx.position(&position_key(&pdas, &maker.pubkey()));
    assert_eq!(sp.size, -5, "seller is short 5");
    assert_eq!(mp.size, 5, "maker is long 5");
    assert_eq!(sp.size + mp.size, 0, "OI conserved (Σ signed sizes == 0)");
    assert_eq!(sp.entry_price, cr.bid_clearing_price);
    assert_eq!(mp.entry_price, cr.bid_clearing_price);

    // Margin exactness: the order's reservation is fully released; what remains
    // locked is EXACTLY the position's margin (no leaked/stranded lock). This is
    // the gap-through case — the fill price (199_680) exceeds the worst_price the
    // reservation was taken at (the OLD window top), so the settle re-lock used
    // the never-revert lock_up_to path and still balanced the ledger.
    let uc = ctx.user_collateral(&seller.pubkey());
    assert_eq!(
        uc.locked, sp.collateral,
        "locked == position margin: reservation exactly released"
    );
}

/// Mirror: a resting BUY at 100_000 is gapped by a DOWN-recenter (new top
/// 50_310 < its price) → marketable, folds at the top tick, and fills a live
/// maker ASK at the uniform clearing price (≤ its limit).
#[test]
fn recentered_window_fills_marketable_resting_buy_against_live_ask() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let oracle = Pubkey::new_unique();
    ctx.set_oracle(&oracle, 100_000, -8);
    let pdas = ctx.init_market_with_oracle(10, 64, 16, oracle);
    let (buyer, maker) = money_setup(&mut ctx, &pdas);

    // In-window resting BUY @ 100_000, qty 5, GTC.
    let o = ctx.submit_order(&pdas, &buyer, SIDE_BUY, 100_000, 5);
    let locked_at_submit = ctx.user_collateral(&buyer.pubkey()).locked;
    assert!(locked_at_submit > 0, "the resting buy locked margin at submit");

    // Round 1: zero fill → re-rests.
    run_zero_fill_round(&mut ctx, &pdas, o, &buyer.pubkey());

    // The market moves THROUGH the buy: recenter to 50_000 → new window
    // [49_680, 50_310]. The buy's fixed 100_000 is above the top →
    // Marketable(num_ticks - 1) per classify_resting_fold (DDR-3).
    ctx.set_oracle(&oracle, 50_000, -8);
    ctx.start_auction(&pdas);

    // Live counterparty: a maker ASK at 50_000 (mid_tick 32 on the new window).
    ctx.init_maker_quote(&pdas, &maker, None, 0);
    ctx.update_maker_quote_levels(&pdas, &maker, 1, 32, &[], &[(0, 5)]);

    // Round 2: fold (the buy folds at the top tick 63), clear, settle.
    let d = ctx.phase_deadline_slot(&pdas);
    ctx.warp_slot(d);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    ctx.finalize_clear(&pdas);

    let cr = ctx.clearing(&pdas).unwrap();
    assert_eq!(cr.ask_matched_volume, 5, "ask auction crossed the full 5");
    // Uniform price = the maker's tick (32): 49_680 + 320 = 50_000 — far below
    // the buy's 100_000 limit (it buys BETTER than it bid).
    assert_eq!(cr.ask_clearing_price, 50_000, "cleared at the maker's price");
    assert!(
        cr.ask_clearing_price <= 100_000,
        "a marketable buy never fills above its limit"
    );

    let (_m, fill) = ctx.settle_fill_with_margin(&pdas, o, &buyer.pubkey());
    assert_eq!(fill, 5, "the marketable buy filled fully");
    let rec = ctx
        .orders(&pdas)
        .into_iter()
        .find(|r| r.order_id == o)
        .unwrap();
    assert_eq!(rec.status, STATUS_CONSUMED, "fully filled → leaves the book");
    assert_eq!(rec.remaining, 0);

    ctx.settle_maker_quote(&pdas, &maker.pubkey());

    // Both sides at the same uniform price; OI conserves.
    let bp = ctx.position(&position_key(&pdas, &buyer.pubkey()));
    let mp = ctx.position(&position_key(&pdas, &maker.pubkey()));
    assert_eq!(bp.size, 5, "buyer is long 5");
    assert_eq!(mp.size, -5, "maker is short 5");
    assert_eq!(bp.size + mp.size, 0, "OI conserved");
    assert_eq!(bp.entry_price, cr.ask_clearing_price);
    assert_eq!(mp.entry_price, cr.ask_clearing_price);

    // Margin exactness (release direction here: the fill price 50_000 is BELOW
    // the buy's reservation price 100_000, so the reservation strictly covers
    // the position margin and the surplus must come back).
    let uc = ctx.user_collateral(&buyer.pubkey());
    assert_eq!(
        uc.locked, bp.collateral,
        "locked == position margin: reservation surplus released"
    );
    assert!(
        uc.locked < locked_at_submit,
        "cheaper fill than the worst case → some reservation released"
    );
}
