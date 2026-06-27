//! Cross-margin account grouping (init + add member).

use tempo_integration_tests::*;

/// An owner can create a cross-margin group and bind flat positions across
/// different markets into it; the member set is recorded for the completeness
/// rule. Duplicates, non-owned positions, and non-flat positions are rejected.
#[test]
fn margin_group_binds_positions_and_enforces_rules() {
    let mut ctx = TestContext::new();
    let pdas_a = ctx.init_market(1, 32, 8);
    let pdas_b = ctx.init_market(1, 32, 8);

    let owner = ctx.new_funded_signer();
    let pos_a = ctx.init_position(&pdas_a, &owner);
    let pos_b = ctx.init_position(&pdas_b, &owner);

    let margin = ctx.init_margin_account(&owner);
    assert_eq!(
        ctx.margin_account(&owner.pubkey()).0,
        0,
        "fresh group is empty"
    );

    ctx.add_position_to_margin(&pdas_a, &owner, &pos_a)
        .expect("add A");
    ctx.add_position_to_margin(&pdas_b, &owner, &pos_b)
        .expect("add B");
    let (count, members) = ctx.margin_account(&owner.pubkey());
    assert_eq!(count, 2);
    assert!(members.contains(&pos_a) && members.contains(&pos_b));

    // Duplicate add rejects.
    assert!(
        ctx.add_position_to_margin(&pdas_a, &owner, &pos_a).is_err(),
        "duplicate member rejected"
    );

    // Another owner cannot bind their position into this group.
    let other = ctx.new_funded_signer();
    let pos_other = ctx.init_position(&pdas_a, &other);
    assert!(
        ctx.add_position_to_margin(&pdas_a, &other, &pos_other)
            .is_err(),
        "other owner has no group / mismatched owner"
    );

    let _ = margin;
}

/// A flat member can be unbound from the group, freeing its slot for reuse so a
/// group that churns through positions is never permanently full (known-issues §2.4).
#[test]
fn margin_group_member_can_be_removed_and_slot_reused() {
    let mut ctx = TestContext::new();
    let pdas_a = ctx.init_market(1, 32, 8);
    let pdas_b = ctx.init_market(1, 32, 8);
    let owner = ctx.new_funded_signer();
    let pos_a = ctx.init_position(&pdas_a, &owner);
    let pos_b = ctx.init_position(&pdas_b, &owner);
    ctx.init_margin_account(&owner);

    ctx.add_position_to_margin(&pdas_a, &owner, &pos_a)
        .expect("add A");
    ctx.add_position_to_margin(&pdas_b, &owner, &pos_b)
        .expect("add B");
    assert_eq!(ctx.margin_account(&owner.pubkey()).0, 2);

    // Remove A (flat) → count drops, A no longer a member.
    ctx.remove_position_from_margin(&owner, &pos_a)
        .expect("remove A");
    let (count, members) = ctx.margin_account(&owner.pubkey());
    assert_eq!(count, 1);
    assert!(!members.contains(&pos_a) && members.contains(&pos_b));

    // The freed slot is reusable: A can be re-added.
    ctx.add_position_to_margin(&pdas_a, &owner, &pos_a)
        .expect("re-add A");
    assert_eq!(ctx.margin_account(&owner.pubkey()).0, 2);

    // Removing a non-member fails.
    let other = ctx.new_funded_signer();
    let pos_other = ctx.init_position(&pdas_a, &other);
    assert!(
        ctx.remove_position_from_margin(&owner, &pos_other).is_err(),
        "non-member / non-owner removal rejected"
    );
}

/// A position with an in-flight (accumulated, not-yet-settled) order cannot be
/// bound into a cross-margin group — it could otherwise settle as isolated after
/// the flip, locking no margin (known-issues §2.5).
#[test]
fn add_to_margin_rejects_position_with_in_flight_order() {
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(1, 32, 8);
    let owner = ctx.new_funded_signer();
    let position = ctx.init_position(&pdas, &owner);
    ctx.init_margin_account(&owner);

    // Owner has a resting order that gets folded (Accumulated) but not settled;
    // the position is still flat (size 0) so the old size==0 check would pass.
    ctx.submit_order(&pdas, &owner, 0, 10, 1);
    ctx.process_chunk(&pdas, 0, 8);

    assert!(
        ctx.add_position_to_margin(&pdas, &owner, &position)
            .is_err(),
        "in-flight accumulated order must block the bind"
    );
}

/// Cross-margin withdraw enforces the combined-health check (post-withdraw
/// recognized equity must cover combined maintenance) and the completeness rule
/// (every member position+market must be supplied).
#[test]
fn cross_margin_withdraw_respects_combined_health_and_completeness() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = 500;
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

    // Group the position while it is still flat (the bind-time rule).
    ctx.init_margin_account(&owner);
    ctx.add_position_to_margin(&pdas, &owner, &position)
        .expect("add");

    // A seller so the owner's maker-buy crosses; seller posts its own margin.
    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1); fund above the limit-price margin so the reservation lock succeeds.
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    // Owner opens long 10 @ 30 (locks initial margin 15) via the quote book.
    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    // Establish the market's effective risk price (30).
    ctx.set_oracle(&oracle, 30, -8);
    ctx.update_funding(&pdas, &oracle);

    let members = [(position, pdas.market, oracle)];

    // Completeness: omitting the member rejects.
    assert!(
        ctx.try_withdraw_cross(&owner, &vault_ta, &owner_ta, 10, &[])
            .is_err(),
        "missing member must fail closed"
    );

    // Combined health: balance 100, maintenance 15 → can withdraw down to 15.
    assert!(
        ctx.try_withdraw_cross(&owner, &vault_ta, &owner_ta, 86, &members)
            .is_err(),
        "withdrawing past combined maintenance rejects"
    );
    ctx.try_withdraw_cross(&owner, &vault_ta, &owner_ta, 85, &members)
        .expect("withdraw down to maintenance");
    assert_eq!(ctx.user_collateral(&owner.pubkey()).balance, 15);
    assert_eq!(ctx.token_balance(&owner_ta), 85);
}

/// Account-level liquidation closes a member of a combined-unhealthy group,
/// drawing the loss from the SHARED balance (not the position's isolated margin),
/// paying the liquidator a penalty, and conserving `vault == Σ balances + insurance`.
#[test]
fn cross_margin_liquidation_draws_from_shared_balance_and_conserves() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market_with_oracle(1, 64, 8, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // Owner posts just above the open margin so a loss leaves a non-zero residual
    // (a clean penalty path), then groups the position while flat.
    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 20);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 20);
    let position = ctx.init_position(&pdas, &owner);
    ctx.init_margin_account(&owner);
    ctx.add_position_to_margin(&pdas, &owner, &position)
        .expect("add");

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1); fund above the limit-price margin so the reservation lock succeeds.
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    // Owner opens long 10 @ 30 (locks 15) via the quote book.
    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    // Effective price drops to 29: combined equity (20 - 10 loss = 10) < maintenance
    // (10*29*5% = 14) → the account is liquidatable.
    ctx.set_oracle(&oracle, 29, -8);
    ctx.update_funding(&pdas, &oracle);

    let members = [(position, pdas.market, oracle)];
    let vault_tokens = ctx.token_balance(&vault_ta);
    ctx.try_liquidate_cross(&liquidator, &owner.pubkey(), &members)
        .expect("cross liquidation");

    // Position closed.
    assert_eq!(ctx.position(&position).size, 0, "member closed");
    // Liquidator earned a penalty (account had residual to pay it).
    assert!(
        ctx.user_collateral(&liquidator.pubkey()).balance > 0,
        "liquidator paid a penalty"
    );
    // Conservation: vault tokens still equal Σ ledger balances + insurance.
    let sum = ctx.user_collateral(&owner.pubkey()).balance
        + ctx.user_collateral(&seller.pubkey()).balance
        + ctx.user_collateral(&liquidator.pubkey()).balance
        + ctx.vault().insurance_balance;
    assert_eq!(sum, vault_tokens, "vault == Σ balances + insurance");
}

/// Audit F1: `liquidate_cross` rejects a duplicated member. A liquidator that
/// supplies the same position twice (to pad the count while omitting a winning
/// leg) is refused before any close, so a combined-healthy account cannot be
/// liquidated by hiding a member.
#[test]
fn cross_liquidation_rejects_duplicate_members() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = 500;
    let pdas_a = ctx.init_market_with_oracle(1, 64, 8, oracle);
    let pdas_b = ctx.init_market_with_oracle(1, 64, 8, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 1000);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 1000);
    let pos_a = ctx.init_position(&pdas_a, &owner);
    let pos_b = ctx.init_position(&pdas_b, &owner);

    // Group both positions while flat, then open A (cross mode locks nothing).
    ctx.init_margin_account(&owner);
    ctx.add_position_to_margin(&pdas_a, &owner, &pos_a)
        .expect("add A");
    ctx.add_position_to_margin(&pdas_b, &owner, &pos_b)
        .expect("add B");

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    ctx.mint_to(&mint, &seller_ta, 1000);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 1000);
    ctx.init_position(&pdas_a, &seller);

    ctx.post_maker_order(&pdas_a, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas_a, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas_a, 0, 8);
    ctx.process_maker_quote(&pdas_a, &owner.pubkey());
    ctx.finalize_clear(&pdas_a);
    ctx.settle_maker_quote(&pdas_a, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas_a, sell_id, &seller.pubkey());
    ctx.set_oracle(&oracle, 30, -8);
    ctx.update_funding(&pdas_a, &oracle);

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    // Supplying pos_a twice (count is 2, so the length check passes) is rejected.
    assert!(
        ctx.try_liquidate_cross(
            &liquidator,
            &owner.pubkey(),
            &[
                (pos_a, pdas_a.market, oracle),
                (pos_a, pdas_a.market, oracle),
            ],
        )
        .is_err(),
        "duplicate member must be rejected before any close"
    );
    assert_eq!(ctx.position(&pos_a).size, 10, "position untouched");
}

/// Audit F2: `withdraw_cross` cannot drain margin locked by an isolated position
/// that is not a member of the supplied group. The owner's isolated lock is
/// protected even though the cross group's combined maintenance is zero.
#[test]
fn cross_withdraw_cannot_drain_isolated_locked_margin() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = 500;
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
    // Isolated position (NOT added to any group) — settle_fill locks 15.
    ctx.init_position(&pdas, &owner);

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1); fund above the limit-price margin so the reservation lock succeeds.
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
    assert_eq!(
        ctx.user_collateral(&owner.pubkey()).locked,
        15,
        "isolated lock"
    );

    // Empty cross group: combined maintenance is 0, but the 15 locked for the
    // isolated position must stay backed.
    ctx.init_margin_account(&owner);
    assert!(
        ctx.try_withdraw_cross(&owner, &vault_ta, &owner_ta, 90, &[])
            .is_err(),
        "cross withdraw cannot dip into isolated locked margin"
    );
    ctx.try_withdraw_cross(&owner, &vault_ta, &owner_ta, 85, &[])
        .expect("withdraw down to the protected locked amount");
    assert_eq!(ctx.user_collateral(&owner.pubkey()).balance, 15);
}

/// Audit F5: `withdraw_cross` rejects a vault account that is not owned by this
/// program (parity with `withdraw`). A foreign/forged vault is refused before any
/// transfer, so its `vault_token_account`/`authority_bump` can never be trusted.
#[test]
fn cross_withdraw_rejects_foreign_vault() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let _pdas = ctx.init_market(1, 64, 8);

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
    ctx.init_margin_account(&owner);

    // A non-program-owned account passed in the vault slot is rejected at parse.
    let foreign_vault = solana_sdk::pubkey::Pubkey::new_unique();
    assert!(
        ctx.try_withdraw_cross_with_vault(&owner, &foreign_vault, &vault_ta, &owner_ta, 10, &[])
            .is_err(),
        "foreign vault must be rejected"
    );
}

/// §2.2 regression: a cross-margin account that is healthy at the *braked* (lagged)
/// effective price but underwater at the *real* oracle must be liquidatable
/// immediately. `liquidate_cross` now prices each leg's solvency off its raw,
/// confidence-checked oracle (not the braked effective price), so the per-slot
/// anti-manipulation brake can no longer double as an anti-liquidation brake during
/// a crash. (Before the fix it priced off `risk_price` = the lagged mark and would
/// have returned `NotLiquidatable` here.)
#[test]
fn cross_liquidation_not_delayed_by_brake() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market_with_oracle(1, 64, 8, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 20);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 20);
    let position = ctx.init_position(&pdas, &owner);
    ctx.init_margin_account(&owner);
    ctx.add_position_to_margin(&pdas, &owner, &position)
        .expect("add");

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1); fund above the limit-price margin so the reservation lock succeeds.
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    // Owner opens long 10 @ 30 via the quote book.
    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    // Establish the braked effective price at 30 (healthy: equity 20 ≥ maint 15).
    ctx.set_oracle(&oracle, 30, -8);
    ctx.update_funding(&pdas, &oracle);

    // CRASH: the real oracle drops to 28, but we DON'T crank — the braked effective
    // price stays frozen at 30. At the raw price the account is underwater (equity
    // 20 + (28-30)*10 = 0 < maint 10*28*5% = 14), at the braked price it looks fine.
    ctx.set_oracle(&oracle, 28, -8);

    let members = [(position, pdas.market, oracle)];
    let vault_tokens = ctx.token_balance(&vault_ta);
    ctx.try_liquidate_cross(&liquidator, &owner.pubkey(), &members)
        .expect("crash is liquidatable at the real oracle despite the lagged brake");

    assert_eq!(
        ctx.position(&position).size,
        0,
        "member closed at the real (raw-oracle) price, not blocked by the brake"
    );
    // Conservation still holds across the brake-priced close.
    let sum = ctx.user_collateral(&owner.pubkey()).balance
        + ctx.user_collateral(&seller.pubkey()).balance
        + ctx.user_collateral(&liquidator.pubkey()).balance
        + ctx.vault().insurance_balance;
    assert_eq!(sum, vault_tokens, "vault == Σ balances + insurance");
}

/// §2.2 regression (symmetric): the braked effective price must not inflate
/// `withdraw_cross` equity. An owner whose long is losing at the real oracle cannot
/// withdraw against a stale-favorable braked mark. `withdraw_cross` prices combined
/// health off each leg's raw oracle, so the over-withdrawal a lagged mark would
/// permit during a crash is rejected.
#[test]
fn cross_withdraw_not_inflated_by_brake() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = 500;
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
    ctx.init_margin_account(&owner);
    ctx.add_position_to_margin(&pdas, &owner, &position)
        .expect("add");

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1); fund above the limit-price margin so the reservation lock succeeds.
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    // Owner opens long 10 @ 30 via the quote book.
    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    // Braked effective price at 30 (loss 0 → at the braked mark the owner could
    // withdraw down to maintenance 15, i.e. 85 out — what the old code allowed).
    ctx.set_oracle(&oracle, 30, -8);
    ctx.update_funding(&pdas, &oracle);

    // CRASH to 25 without cranking: braked stays 30, raw = 25. At the raw price the
    // long is down (25-30)*10 = -50, so recognized equity is balance - 50.
    ctx.set_oracle(&oracle, 25, -8);

    let members = [(position, pdas.market, oracle)];
    // Withdrawing 85 (allowed at the lagged mark) must now reject: equity_after
    // (100-85) - 50 = -35 < maint 10*25*5% = 12.
    assert!(
        ctx.try_withdraw_cross(&owner, &vault_ta, &owner_ta, 85, &members)
            .is_err(),
        "over-withdrawal against the braked mark is rejected at the real oracle"
    );
    // A withdrawal the raw price actually backs still succeeds: (100-30) - 50 = 20 ≥ 12.
    ctx.try_withdraw_cross(&owner, &vault_ta, &owner_ta, 30, &members)
        .expect("a raw-oracle-backed withdrawal succeeds");
    assert_eq!(ctx.user_collateral(&owner.pubkey()).balance, 70);
}

/// §2.4: a *flat* (size-0) group member can be supplied as a bare `position`
/// account — no market, no oracle — so it costs one account instead of three. The
/// flat leg contributes nothing but its (zero) realized PnL, and its market need
/// never be cranked. A non-flat leg supplied as flat fails closed (it would hide
/// its loss + maintenance from the combined-health gate).
#[test]
fn cross_withdraw_accepts_flat_member_as_bare_single() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = 500;
    let pdas_a = ctx.init_market_with_oracle(1, 64, 8, oracle);
    let pdas_b = ctx.init_market_with_oracle(1, 64, 8, oracle);

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
    // pos_a goes live on market A; pos_b stays flat on market B.
    let pos_a = ctx.init_position(&pdas_a, &owner);
    let pos_b = ctx.init_position(&pdas_b, &owner);
    ctx.init_margin_account(&owner);
    ctx.add_position_to_margin(&pdas_a, &owner, &pos_a)
        .expect("add A");
    ctx.add_position_to_margin(&pdas_b, &owner, &pos_b)
        .expect("add B");

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1); fund above the limit-price margin so the reservation lock succeeds.
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas_a, &seller);

    // Owner opens long 10 @ 30 on A via the quote book.
    ctx.post_maker_order(&pdas_a, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas_a, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas_a, 0, 8);
    ctx.process_maker_quote(&pdas_a, &owner.pubkey());
    ctx.finalize_clear(&pdas_a);
    ctx.settle_maker_quote(&pdas_a, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas_a, sell_id, &seller.pubkey());

    // Only market A is cranked. Market B is deliberately never given an effective
    // price — a flat leg must not require its market/oracle.
    ctx.set_oracle(&oracle, 30, -8);
    ctx.update_funding(&pdas_a, &oracle);

    // Members in group order: A live (triple), B flat (bare single) → 4 accounts, not
    // the 6 two triples would cost. Withdraw down to A's maintenance (15) succeeds.
    let legs = [
        CrossLeg::Live(pos_a, pdas_a.market, oracle),
        CrossLeg::Flat(pos_b),
    ];
    assert!(
        ctx.try_withdraw_cross_mixed(&owner, &vault_ta, &owner_ta, 86, &legs)
            .is_err(),
        "past combined maintenance still rejects with a flat leg present"
    );
    ctx.try_withdraw_cross_mixed(&owner, &vault_ta, &owner_ta, 85, &legs)
        .expect("flat member as a bare single is accepted (no market/oracle for B)");
    assert_eq!(ctx.user_collateral(&owner.pubkey()).balance, 15);

    // Fail-closed: claiming the LIVE leg (pos_a, size 10) as flat is rejected — it
    // would otherwise omit its loss + maintenance from the health gate.
    let bad = [CrossLeg::Flat(pos_a), CrossLeg::Flat(pos_b)];
    assert!(
        ctx.try_withdraw_cross_mixed(&owner, &vault_ta, &owner_ta, 1, &bad)
            .is_err(),
        "a non-flat leg supplied as flat must fail closed"
    );
}

/// §2.4: a cross-margin liquidation can carry a *flat* member as a bare single (no
/// market/oracle); the close target is the first non-flat member. Conservation still
/// holds across the close.
#[test]
fn cross_liquidation_accepts_flat_member_as_bare_single() {
    let mut ctx = TestContext::new();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = 500;
    let pdas_a = ctx.init_market_with_oracle(1, 64, 8, oracle);
    let pdas_b = ctx.init_market_with_oracle(1, 64, 8, oracle);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 20);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 20);
    let pos_a = ctx.init_position(&pdas_a, &owner);
    let pos_b = ctx.init_position(&pdas_b, &owner);
    ctx.init_margin_account(&owner);
    ctx.add_position_to_margin(&pdas_a, &owner, &pos_a)
        .expect("add A");
    ctx.add_position_to_margin(&pdas_b, &owner, &pos_b)
        .expect("add B");

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1); fund above the limit-price margin so the reservation lock succeeds.
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas_a, &seller);

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    // Owner opens long 10 @ 30 on A via the quote book.
    ctx.post_maker_order(&pdas_a, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas_a, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas_a, 0, 8);
    ctx.process_maker_quote(&pdas_a, &owner.pubkey());
    ctx.finalize_clear(&pdas_a);
    ctx.settle_maker_quote(&pdas_a, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas_a, sell_id, &seller.pubkey());

    // Crash to 29: combined equity (20 - 10) = 10 < maintenance (10*29*5% = 14).
    // Only A is cranked; the flat B leg needs no market/oracle.
    ctx.set_oracle(&oracle, 29, -8);
    ctx.update_funding(&pdas_a, &oracle);

    let legs = [
        CrossLeg::Live(pos_a, pdas_a.market, oracle),
        CrossLeg::Flat(pos_b),
    ];
    let vault_tokens = ctx.token_balance(&vault_ta);
    ctx.try_liquidate_cross_mixed(&liquidator, &owner.pubkey(), &legs)
        .expect("liquidation closes the first non-flat member with a flat leg present");

    assert_eq!(ctx.position(&pos_a).size, 0, "the live member was closed");
    assert_eq!(
        ctx.position(&pos_b).size,
        0,
        "the flat member is untouched (still flat)"
    );
    let sum = ctx.user_collateral(&owner.pubkey()).balance
        + ctx.user_collateral(&seller.pubkey()).balance
        + ctx.user_collateral(&liquidator.pubkey()).balance
        + ctx.vault().insurance_balance;
    assert_eq!(sum, vault_tokens, "vault == Σ balances + insurance");
}
