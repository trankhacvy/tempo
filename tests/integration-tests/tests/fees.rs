//! Phase 1 — signed maker/taker fees + integrator revenue share.
//!
//! Verifies (a) the role split (maker pays `maker_fee_bps`, taker `taker_fee_bps`)
//! and the integrator share both conserve `vault_token == Σ balances + insurance`,
//! and (b) a maker rebate is capped at available insurance (never mints money).

use tempo_integration_tests::*;

/// A positive taker fee splits between the integrator ledger and insurance; the
/// maker (fee 0 here) pays nothing. All movements stay inside the vault.
#[test]
fn taker_fee_and_integrator_share_conserve() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    ctx.market_fee_bps = 100; // taker: 1% of notional
    ctx.market_maker_fee_bps = 0; // maker: free
    ctx.market_integrator_share_bps = 5000; // 50% of the positive fee
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let a = ctx.new_funded_signer(); // maker buyer
    let b = ctx.new_funded_signer(); // taker seller
    for t in [&a, &b] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }
    // The integrator just needs a ledger to receive its share.
    let integrator = ctx.new_funded_signer();
    ctx.init_collateral(&integrator);
    let cranker = ctx.new_funded_signer();
    ctx.init_collateral(&cranker);

    // Open at 20: A maker-buys via the quote book (fee 0), B taker-sells (fee =
    // 10*20*1% = 2). Bid auction: maker-buy (BidDemand) vs taker-sell (BidSupply).
    ctx.post_maker_order(&pdas, &a, SIDE_BUY, 20, 10);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.finalize_clear_with_fee(&pdas, &cranker);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_integrator(&pdas, b_sell, &b.pubkey(), &integrator.pubkey());

    let ua = ctx.user_collateral(&a.pubkey());
    let ub = ctx.user_collateral(&b.pubkey());
    let ui = ctx.user_collateral(&integrator.pubkey());
    let insurance = ctx.vault().insurance_balance;

    assert_eq!(ua.balance, 1000, "maker paid no fee");
    assert_eq!(ub.balance, 998, "taker paid a 2-unit fee");
    assert_eq!(ui.balance, 1, "integrator got 50% of the fee");
    assert_eq!(insurance, 1, "insurance got the other 50%");
    assert_eq!(
        ua.balance + ub.balance + ui.balance + insurance,
        ctx.token_balance(&vault_ta),
        "claims + insurance == vault tokens"
    );
    assert_eq!(ua.balance + ub.balance + ui.balance + insurance, 2000);
}

/// A maker rebate is capped at available insurance: with an empty pool the rebate
/// is denied (not minted), so the conservation invariant still holds.
#[test]
fn maker_rebate_capped_by_empty_insurance() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    ctx.market_fee_bps = 0; // taker: free
    ctx.market_maker_fee_bps = -100; // maker rebate 1% (would be 2 per fill)
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    let a = ctx.new_funded_signer(); // maker buyer
    let b = ctx.new_funded_signer(); // taker seller
    for t in [&a, &b] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }
    let cranker = ctx.new_funded_signer();
    ctx.init_collateral(&cranker);

    // Insurance starts empty, so the maker's rebate must be denied (capped at 0).
    // A maker-buys via the quote book (rebate side); B taker-sells. Bid auction.
    ctx.post_maker_order(&pdas, &a, SIDE_BUY, 20, 10);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.finalize_clear_with_fee(&pdas, &cranker);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, b_sell, &b.pubkey());

    let ua = ctx.user_collateral(&a.pubkey());
    let ub = ctx.user_collateral(&b.pubkey());
    let insurance = ctx.vault().insurance_balance;

    assert_eq!(
        ua.balance, 1000,
        "rebate denied: empty insurance can't fund it"
    );
    assert_eq!(ub.balance, 1000, "taker is free");
    assert_eq!(insurance, 0, "insurance never went negative");
    assert_eq!(
        ua.balance + ub.balance + insurance,
        ctx.token_balance(&vault_ta),
        "claims + insurance == vault tokens (no money minted)"
    );
}
