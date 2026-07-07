//! The liquidation money path (headline test).
//!
//! Full flow: vault + collateral, an owner who deposits margin, opens a long via
//! a cleared auction (`settle_fill_with_margin` locks initial margin into the
//! position), then the oracle drops so the long is underwater-but-solvent. A
//! permissionless liquidator closes it: the position is zeroed, the liquidator is
//! paid the penalty, and the owner is credited the residual equity.
//!
//! Unit assumption (margin.rs): `collateral`, `pnl`, and `|size|·price` share
//! one base unit, so we keep `tick_size = 1`, the entry price (~30) and the
//! oracle `price_1e8` in the same small numeric scale.

use tempo_integration_tests::*;

const MAINT_BPS: u16 = 500; // 5%

#[test]
fn liquidate_underwater_long() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = MAINT_BPS;

    // tick_size = 1, num_ticks big enough for price 30 (tick 29), small slab cap.
    let pdas = ctx.init_market_with_oracle(1, 32, 8, oracle);

    // Vault + a token account owned by the vault-authority PDA.
    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // Owner: ledger + 100 deposited collateral + a Position.
    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 100);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 100);
    let position = ctx.init_position(&pdas, &owner);

    // Liquidator: just needs a collateral ledger to be paid the penalty into.
    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    // Counterparty seller so the owner's maker-buy crosses; on a margin market
    // the seller must also post margin (initial_margin(10, 30, 500) = 15).
    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1), released back at settle; fund above the limit-price margin so the submit
    // reservation lock succeeds (the position's locked margin stays the actual 15).
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    // Owner opens a long: maker-buy 10 @ 30 (quote book), taker-sell 10 @ 30.
    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);

    // Settle the owner's maker fill WITH margin → locks initial margin into the
    // position. initial_margin(10, 30, 500) = 10*30*5% = 15.
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    // §7.1: the owner's 10-lot ladder still stands after the fill (standing
    // reservation 10·window_top(32)·5% = 16). Roll and clear it so the
    // liquidation math below sees only the position margin.
    ctx.start_auction(&pdas);
    ctx.try_clear_maker_quote(&pdas, &owner, 2)
        .expect("clear releases the ladder reservation");

    let p = ctx.position(&position);
    assert_eq!(p.size, 10, "long 10");
    assert_eq!(p.entry_price, 30);
    assert_eq!(p.collateral, 15, "initial margin locked = 10*30*5%");

    let owner_uc = ctx.user_collateral(&owner.pubkey());
    assert_eq!(owner_uc.balance, 100);
    assert_eq!(owner_uc.locked, 15, "margin locked in the ledger");

    // --- healthy position cannot be liquidated ---
    // Oracle at 31 (above entry): equity 15 + 10*(31-30)=25 >> maintenance
    // (10*31*5% = 15) → NotLiquidatable.
    ctx.set_oracle(&oracle, 31, -8);
    let healthy = ctx.try_liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());
    assert!(
        healthy.is_err(),
        "a healthy position must not be liquidatable"
    );
    // Position untouched by the failed attempt.
    assert_eq!(ctx.position(&position).size, 10);

    // --- underwater-but-solvent: liquidate ---
    // Oracle drops to 29 (price_1e8 = 29): unrealized = 10*(29-30) = -10 →
    // equity = collateral(15) + (-10) = 5. maintenance = 10*29*5% = 14.
    // equity(5) < maintenance(14) → liquidatable.
    // penalty = |10|*29 * 1% = 2 (capped at equity 5); returned = 5-2 = 3; no bad debt.
    let insurance_before = ctx.vault().insurance_balance;
    ctx.set_oracle(&oracle, 29, -8);
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());

    // Position is fully zeroed.
    let p = ctx.position(&position);
    assert_eq!(p.size, 0, "position closed");
    assert_eq!(p.collateral, 0);
    assert_eq!(p.entry_price, 0);
    assert_eq!(p.realized_pnl, 0);

    // Liquidator was paid the 2-unit penalty into their ledger.
    assert_eq!(
        ctx.user_collateral(&liquidator.pubkey()).balance,
        2,
        "liquidator earns the penalty"
    );

    // Owner: margin released; the position's collateral (15) leaves `balance`
    // and only the residual equity (3) returns → 100 - 15 + 3 = 88. The 10-unit
    // loss + 2-unit penalty are borne by the owner (conservation).
    let owner_uc = ctx.user_collateral(&owner.pubkey());
    assert_eq!(owner_uc.locked, 0, "locked margin released");
    assert_eq!(
        owner_uc.balance, 88,
        "owner keeps deposit minus loss and penalty (100 - 15 + 3)"
    );

    // The owner's 10-unit realized loss accrues to insurance (the conserving close
    // routes it there to fund the counterparty's matching gain) — no bad debt.
    assert_eq!(
        ctx.vault().insurance_balance,
        insurance_before + 10,
        "owner's realized loss (10) flows to insurance"
    );
}

/// A liquidation with bad debt that insurance cannot fully cover socializes
/// the uncovered residual to the winning (counterparty) side via its social-loss
/// index, instead of silently logging it.
#[test]
fn liquidate_bad_debt_socializes_to_winning_side() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = MAINT_BPS;
    let pdas = ctx.init_market_with_oracle(1, 32, 8, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // Owner long 10 @ 30 (margin 15); seller short 10 @ 30 (margin 15).
    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 100);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 100);
    let position = ctx.init_position(&pdas, &owner);

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1), released back at settle; fund above the limit-price margin so the submit
    // reservation lock succeeds (the position's locked margin stays the actual 15).
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    // OI is 10/10 and insurance is empty before the crash.
    let m = ctx.market(&pdas);
    assert_eq!((m.oi_long, m.oi_short), (10, 10));
    assert_eq!(ctx.vault().insurance_balance, 0);

    // Oracle crashes to 28: unrealized = 10*(28-30) = -20, equity = 15-20 = -5 → bad
    // debt 5 (equity <= 0, no penalty/return).
    ctx.set_oracle(&oracle, 28, -8);
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());

    // Position closed; its long OI is gone, the short winner's OI remains.
    assert_eq!(ctx.position(&position).size, 0);
    let m = ctx.market(&pdas);
    assert_eq!(m.oi_long, 0);
    assert_eq!(m.oi_short, 10);

    // The seized collateral (15) is in insurance; the 5 bad debt exceeds the 0
    // pre-existing insurance, so all 5 is socialized to the short side:
    // index += 5 * FUNDING_SCALE / oi_short(10) = 5e8.
    assert_eq!(
        ctx.vault().insurance_balance,
        15,
        "seized collateral in pool"
    );
    assert_eq!(
        m.social_loss_index_short, 500_000_000,
        "bad debt 5 socialized to shorts over OI 10"
    );
    assert_eq!(m.social_loss_index_long, 0, "longs untouched");
}

/// Liquidation is permissionless and a redundant attempt is a clean
/// no-op. After one liquidator closes the position, a second liquidator's attempt
/// reverts (NotLiquidatable) and changes nothing — racing keepers are safe.
#[test]
fn redundant_liquidation_is_clean_no_op() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = MAINT_BPS;
    let pdas = ctx.init_market_with_oracle(1, 32, 8, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 100);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 100);
    let position = ctx.init_position(&pdas, &owner);

    // Two independent, permissionless liquidators (neither is the position owner).
    let liq1 = ctx.new_funded_signer();
    let liq2 = ctx.new_funded_signer();
    ctx.init_collateral(&liq1);
    ctx.init_collateral(&liq2);

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1), released back at settle; fund above the limit-price margin so the submit
    // reservation lock succeeds (the position's locked margin stays the actual 15).
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    ctx.set_oracle(&oracle, 29, -8);
    ctx.liquidate(&pdas, &oracle, &liq1, &owner.pubkey());
    assert_eq!(
        ctx.position(&position).size,
        0,
        "first liquidator closes it"
    );
    let liq2_before = ctx.user_collateral(&liq2.pubkey()).balance;
    let insurance_before = ctx.vault().insurance_balance;

    // Second liquidator races in too late → clean revert, no state change.
    assert!(
        ctx.try_liquidate(&pdas, &oracle, &liq2, &owner.pubkey())
            .is_err(),
        "a flat position is not liquidatable"
    );
    assert_eq!(ctx.position(&position).size, 0);
    assert_eq!(
        ctx.user_collateral(&liq2.pubkey()).balance,
        liq2_before,
        "loser paid nothing"
    );
    assert_eq!(
        ctx.vault().insurance_balance,
        insurance_before,
        "no state change"
    );
}

/// A *soft*-stale oracle (too old to read fresh, but within `soft_stale_slots`
/// of the last good update) still permits risk-reducing liquidation off the frozen
/// effective price — a brief oracle hiccup does not strand an underwater position.
#[test]
fn liquidation_proceeds_during_soft_stale_oracle() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = MAINT_BPS;
    ctx.market_soft_stale_slots = 100; // brake cap stays 0 → effective tracks oracle
    let pdas = ctx.init_market_with_oracle(1, 32, 8, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 100);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 100);
    let position = ctx.init_position(&pdas, &owner);

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1), released back at settle; fund above the limit-price margin so the submit
    // reservation lock succeeds (the position's locked margin stays the actual 15).
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    // Bootstrap the effective price to a liquidatable 29 via a fresh crank, which
    // also stamps the last-good-oracle slot.
    ctx.set_oracle(&oracle, 29, -8);
    ctx.update_funding(&pdas, &oracle);
    assert_eq!(ctx.market(&pdas).effective_price_1e8, 29);

    // The oracle now goes stale (wall clock advances past MAX_AGE; slot unchanged,
    // so we stay inside the soft-stale window).
    ctx.set_clock_ts(1_700_000_200);
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());
    assert_eq!(
        ctx.position(&position).size,
        0,
        "liquidated off the frozen effective price during a soft-stale oracle"
    );
}

/// Solvency liquidation prices off the RAW (confidence-checked) oracle, not the
/// braked effective price: a fast crash is liquidatable immediately even though
/// the per-slot brake has only walked the mark partway. The brake still governs
/// the funding mark (known-issues §2.2).
#[test]
fn liquidation_prices_off_raw_oracle_not_braked_mark() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = MAINT_BPS;
    ctx.market_max_price_move_bps = 500; // 5%/slot brake active
    let pdas = ctx.init_market_with_oracle(1, 64, 8, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 100);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 100);
    let position = ctx.init_position(&pdas, &owner);

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1), released back at settle; fund above the limit-price margin so the submit
    // reservation lock succeeds (the position's locked margin stays the actual 15).
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    // Owner opens a long 10 @ 30 (initial margin 15 locked into the position).
    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    // Bootstrap the braked effective price HIGH (35) via update_funding.
    ctx.set_oracle(&oracle, 35, -8);
    ctx.update_funding(&pdas, &oracle);
    let base_slot = ctx.market(&pdas).last_good_oracle_slot;
    assert_eq!(
        ctx.market(&pdas).effective_price_1e8,
        35,
        "bootstrapped high"
    );

    // Crash the oracle to 29 in one slot. The brake caps the effective price near
    // 35 (healthy: equity ~55 >> maintenance), so the OLD braked-mark logic would
    // refuse. The RAW oracle 29 makes the long underwater (equity 5 < maint 14).
    ctx.set_oracle(&oracle, 29, -8);
    ctx.warp_slot(base_slot + 1);
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());

    assert_eq!(
        ctx.position(&position).size,
        0,
        "liquidated at the raw oracle despite the braked mark still being healthy"
    );
    let eff = ctx.market(&pdas).effective_price_1e8;
    assert!(
        eff > 29 && eff <= 35,
        "the brake still governs the funding mark (got {eff})"
    );
}
