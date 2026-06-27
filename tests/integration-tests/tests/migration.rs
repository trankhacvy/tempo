//! Account layout migration (compat): a deployed VERSION-4 `Market` is upgraded to
//! v5, and a VERSION-2 `Position` is upgraded to v3 (appending `margin_mode`).
//!
//! Both layout bumps are pure *appends*, so the existing bytes keep their meaning;
//! migration grows the account, zero-inits the new tail, sets the version byte, and
//! (for the market) the two admin-chosen risk-config fields. The position v2→v3
//! append defaults `margin_mode` to 0 (isolated) and leaves market OI untouched.

use tempo_integration_tests::*;

/// A v4 market upgrades to v5: prefix fields are preserved, the new risk block is
/// zeroed, the admin config is applied, and the market is still fully usable.
#[test]
fn migrate_market_v4_to_v5_preserves_prefix_and_works() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(7, 64, 8); // tick_size 7, distinctive prefix values

    // Capture stable prefix fields + the full v5 length before downgrading.
    let before = ctx.market(&pdas);
    let v5_len = ctx.account_raw(&pdas.market).len();
    let authority_before = before.authority;

    // Simulate a real on-chain v4 account, then migrate it.
    ctx.downgrade_market_to_v4(&pdas.market);
    assert_eq!(
        ctx.account_raw(&pdas.market).len(),
        v5_len - 124,
        "downgraded account is the v4 size (risk block + window floor + v8 risk config dropped)"
    );
    ctx.migrate_market(&pdas, 250, 100); // max_price_move_bps=250, soft_stale=100

    // Grown back to the current size, version byte bumped to the current VERSION.
    let raw = ctx.account_raw(&pdas.market);
    assert_eq!(raw.len(), v5_len, "regrown to current size");
    assert_eq!(raw[1], 8, "version byte bumped to current VERSION");

    let after = ctx.market(&pdas);
    // Prefix fields untouched.
    assert_eq!(after.authority, authority_before, "authority preserved");
    assert_eq!(after.tick_size, 7, "tick_size preserved");
    assert_eq!(after.num_ticks, 64, "num_ticks preserved");
    // New risk block zeroed.
    assert_eq!(after.oi_long, 0);
    assert_eq!(after.oi_short, 0);
    assert_eq!(after.effective_price_1e8, 0);
    // Trailing config (current): ... max_price_move(2), soft_stale(8), window_floor(8),
    // initial_margin_bps(2), max_position_notional(16). The older three now sit 18
    // bytes before the account end; the v8 tail is left zero by migrate.
    let n = raw.len();
    assert_eq!(
        u16::from_le_bytes(raw[n - 36..n - 34].try_into().unwrap()),
        250,
        "max_price_move_bps_per_slot set"
    );
    assert_eq!(
        u64::from_le_bytes(raw[n - 34..n - 26].try_into().unwrap()),
        100,
        "soft_stale_slots set"
    );
    // Window floor seeded to the genesis default (tick_size = 7) by migrate (§2.7).
    assert_eq!(
        u64::from_le_bytes(raw[n - 26..n - 18].try_into().unwrap()),
        7,
        "window_floor_price seeded to tick_size"
    );
    // v8 pre-trade risk config left zero (initial falls back to maintenance; cap off).
    assert_eq!(
        u16::from_le_bytes(raw[n - 18..n - 16].try_into().unwrap()),
        0,
        "initial_margin_bps defaults to 0 (→ maintenance)"
    );
    assert_eq!(
        u128::from_le_bytes(raw[n - 16..n].try_into().unwrap()),
        0,
        "max_position_notional defaults to 0 (disabled)"
    );

    // The migrated market is fully functional: it still accepts orders. On a
    // money-path market (maint > 0) a submit reserves worst-case margin
    // (missing-features §1.1), so the trader needs a funded ledger + position.
    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);
    let trader = ctx.new_funded_signer();
    ctx.init_collateral(&trader);
    let t_ta = ctx.create_token_account(&mint, &trader.pubkey());
    ctx.mint_to(&mint, &t_ta, 1000);
    ctx.deposit(&trader, &vault_ta, &t_ta, 1000);
    ctx.init_position(&pdas, &trader);
    let _ = ctx.submit_order(&pdas, &trader, SIDE_BUY, 7, 3);
}

/// A second migration of an already-v5 market is rejected (idempotency / safety).
#[test]
fn migrate_market_twice_is_rejected() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 32, 8);
    ctx.downgrade_market_to_v4(&pdas.market);
    ctx.migrate_market(&pdas, 0, 0);
    // Already v5 → NotMigratable.
    assert!(
        ctx.try_migrate_market(&pdas, 0, 0).is_err(),
        "re-migrating a v5 market must fail"
    );
}

/// A v2 `Position` upgrades to v3 in place: the `margin_mode` byte is appended
/// (defaulting to 0 = isolated), the version is bumped, prior fields are
/// preserved, and the market's open interest is untouched (v2→v3 is a pure append).
#[test]
fn migrate_position_v2_to_v3_appends_isolated_margin_mode() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // Open a long(A)/short(B) pair of 10 @ 20 → market OI 10/10.
    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    for t in [&a, &b] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }
    // Maker-buy (a) opens its long via the quote book; taker-sell (b) crosses it.
    // submit_order is taker-only (§1.3). OI is unchanged: the maker still takes
    // the long via its quote.
    ctx.post_maker_order(&pdas, &a, SIDE_BUY, 20, 10);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, b_sell, &b.pubkey());
    assert_eq!(ctx.market(&pdas).oi_long, 10);
    assert_eq!(ctx.market(&pdas).oi_short, 10);

    let pos_a = ctx.position_pda(&pdas, &a.pubkey()).0;
    let pos_b = ctx.position_pda(&pdas, &b.pubkey()).0;

    // Roll the positions back to the v2 layout, then migrate v2 → v3.
    ctx.downgrade_position_to_v2(&pos_a);
    ctx.downgrade_position_to_v2(&pos_b);
    ctx.migrate_position(&pdas, &a, &pos_a);
    ctx.migrate_position(&pdas, &b, &pos_b);

    // Version bumped to 3, margin_mode appended as 0 (isolated), size preserved.
    for p in [&pos_a, &pos_b] {
        let raw = ctx.account_raw(p);
        assert_eq!(raw[1], 3, "version byte bumped to 3");
        assert_eq!(raw[raw.len() - 1], 0, "margin_mode defaults to isolated");
    }
    assert_eq!(ctx.position(&pos_a).size, 10, "long size preserved");
    assert_eq!(ctx.position(&pos_b).size, -10, "short size preserved");

    // v2 → v3 is a pure append: market OI is untouched.
    assert_eq!(ctx.market(&pdas).oi_long, 10, "OI unchanged by migrate");
    assert_eq!(ctx.market(&pdas).oi_short, 10);

    // Re-migrating a v3 position is rejected (idempotency / safety).
    ctx.downgrade_position_to_v2(&pos_a);
    ctx.migrate_position(&pdas, &a, &pos_a);
    assert!(
        ctx.try_migrate_position(&pdas, &a, &pos_a).is_err(),
        "re-migrating a v3 position must fail"
    );
}

/// A v1 `Position` upgrades straight to v3: the `last_social_index` + `margin_mode`
/// tail is appended (both zero), and — as in the original v1 path — the market OI
/// that `migrate_market` reset to 0 is rebuilt from each migrated position's size.
#[test]
fn migrate_positions_v1_to_v3_rebuild_market_oi() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    for t in [&a, &b] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }
    // Maker-buy (a) opens its long via the quote book; taker-sell (b) crosses it.
    // submit_order is taker-only (§1.3). OI is unchanged: the maker still takes
    // the long via its quote.
    ctx.post_maker_order(&pdas, &a, SIDE_BUY, 20, 10);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, b_sell, &b.pubkey());
    assert_eq!(ctx.market(&pdas).oi_long, 10);
    assert_eq!(ctx.market(&pdas).oi_short, 10);

    let pos_a = ctx.position_pda(&pdas, &a.pubkey()).0;
    let pos_b = ctx.position_pda(&pdas, &b.pubkey()).0;

    // Roll back to the original v4 market + v1 positions, then migrate.
    ctx.downgrade_market_to_v4(&pdas.market);
    ctx.downgrade_position_to_v1(&pos_a);
    ctx.downgrade_position_to_v1(&pos_b);
    ctx.migrate_market(&pdas, 0, 0);
    assert_eq!(ctx.market(&pdas).oi_long, 0, "market OI reset on migrate");
    assert_eq!(ctx.market(&pdas).oi_short, 0);

    ctx.migrate_position(&pdas, &a, &pos_a);
    assert_eq!(ctx.market(&pdas).oi_long, 10, "long OI rebuilt");
    ctx.migrate_position(&pdas, &b, &pos_b);
    assert_eq!(ctx.market(&pdas).oi_short, 10, "short OI rebuilt");

    // Upgraded to v3 with isolated margin_mode, sizes preserved.
    for p in [&pos_a, &pos_b] {
        let raw = ctx.account_raw(p);
        assert_eq!(raw[1], 3, "version byte bumped to 3");
        assert_eq!(raw[raw.len() - 1], 0, "margin_mode defaults to isolated");
    }
    assert_eq!(ctx.position(&pos_a).size, 10);
    assert_eq!(ctx.position(&pos_b).size, -10);
}
