//! Treasury (plan.md §3.4/§3.5): the permissionless insurance seed, the
//! on-chain `total_user_balance` backing aggregate, and the fail-closed
//! outflow gate (`VaultInvariantViolated`).

use tempo_integration_tests::*;

/// seed_insurance is a pure pool donation: insurance grows by the face amount,
/// vault tokens grow with it, and `total_user_balance` is untouched (pool money
/// is not a user claim) — and a previously-clamped maker REBATE now pays.
#[test]
fn seed_insurance_funds_the_pool_and_rebates_pay() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    ctx.market_maker_fee_bps = -10; // a maker REBATE, paid from insurance
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // Maker + taker, both funded.
    let maker = ctx.new_funded_signer();
    let taker = ctx.new_funded_signer();
    for t in [&maker, &taker] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 10_000);
        ctx.deposit(t, &vault_ta, &ta, 10_000);
        ctx.init_position(&pdas, t);
    }

    // Seed the pool from a donor (permissionless — not the admin).
    let donor = ctx.new_funded_signer();
    let donor_ta = ctx.create_token_account(&mint, &donor.pubkey());
    ctx.mint_to(&mint, &donor_ta, 5_000);
    let users_before = ctx.vault().total_user_balance;
    ctx.seed_insurance(&donor, &vault_ta, &donor_ta, 5_000);
    let v = ctx.vault();
    assert_eq!(
        v.insurance_balance, 5_000,
        "pool credited by the face amount"
    );
    assert_eq!(
        v.total_user_balance, users_before,
        "a donation is pool money, not a user claim"
    );
    assert_eq!(
        ctx.token_balance(&vault_ta),
        20_000 + 5_000,
        "tokens actually arrived"
    );

    // A fill whose maker rebate now PAYS from the seeded pool (on an empty pool
    // this clamped to zero — the P0.6 devnet deadlock's whole story).
    // Notional must be big enough that a 10 bps rebate is ≥ 1 unit
    // (100 lots · 20 = 2000 → rebate 2).
    ctx.post_maker_order(&pdas, &maker, SIDE_BUY, 20, 100);
    let sell = ctx.submit_order(&pdas, &taker, SIDE_SELL, 20, 100);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &maker.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_fill_with_margin(&pdas, sell, &taker.pubkey());
    let insurance_before_maker = ctx.vault().insurance_balance;
    ctx.settle_maker_quote(&pdas, &maker.pubkey());
    assert!(
        ctx.vault().insurance_balance < insurance_before_maker,
        "the maker rebate drew from the seeded pool instead of clamping to zero"
    );

    // The §3.4 aggregate matched Σ ledgers through the whole flow.
    ctx.assert_aggregate(&[maker.pubkey(), taker.pubkey()]);
}

/// The aggregate tracks EVERY balance-changing site through a full lifecycle:
/// deposits, fills with fees, PnL flush, withdrawal.
#[test]
fn aggregate_tracks_the_full_lifecycle() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    ctx.market_fee_bps = 30; // taker fee → insurance (a third flow direction)
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
        ctx.mint_to(&mint, &ta, 1_000);
        ctx.deposit(t, &vault_ta, &ta, 1_000);
        ctx.init_position(&pdas, t);
    }
    let users = [a.pubkey(), b.pubkey()];
    ctx.assert_aggregate(&users); // after deposits: 2000

    // A crossed round with a taker fee (ledger → insurance at settle).
    ctx.post_maker_order(&pdas, &a, SIDE_BUY, 20, 10);
    let sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell, &b.pubkey());
    ctx.assert_aggregate(&users); // after fee flows

    // A withdrawal (ledger + aggregate + tokens all drop together).
    let b_ta = ctx.create_token_account(&mint, &b.pubkey());
    let free = ctx.user_collateral(&b.pubkey()).free();
    ctx.withdraw(&b, &vault_ta, &b_ta, free.min(100));
    ctx.assert_aggregate(&users);
}

/// The fail-closed outflow gate (§4.2): if the aggregate says the vault owes
/// more than its token balance holds, withdrawals are BLOCKED with
/// `VaultInvariantViolated` — drift stops money leaving, it never wedges rounds.
#[test]
fn corrupted_backing_blocks_withdrawals_fail_closed() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8);
    let _ = &pdas;

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 1_000);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 1_000);

    // A healthy withdraw passes the gate.
    ctx.withdraw(&owner, &vault_ta, &owner_ta, 100);

    // Corrupt the aggregate upward (simulating drift / an unbacked claim):
    // total_user_balance := 10_000 while the vault only holds 900 tokens.
    ctx.corrupt_vault_aggregate(10_000);

    // The gate refuses to let tokens leave (Custom(51), fail closed).
    assert!(
        ctx.try_withdraw(&owner, &vault_ta, &owner_ta, 100).is_err(),
        "a vault whose aggregate exceeds its token backing must block outflows"
    );
}
