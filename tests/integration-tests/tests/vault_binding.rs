//! The vault is bound to the market. A market that declares a
//! `collateral_mint` must reject any vault for a different mint, even though that
//! foreign vault is itself a canonical `[b"vault", mint]` PDA (so Tier-1
//! `validate_self` passes and only the Tier-2 mint binding catches it).

use tempo_integration_tests::*;

#[test]
fn vault_binding_rejects_foreign_vault() {
    let mut ctx = TestContext::new();
    let mint_a = ctx.create_mint();
    ctx.market_collateral_mint = Some(mint_a);
    ctx.market_crank_fee = 3; // engage the crank-fee path so the vault is read
    let pdas = ctx.init_market(1, 32, 8);

    // A canonical vault for a DIFFERENT mint B — the foreign vault.
    let (vault_authority, _) = ctx.vault_authority_pda();
    let mint_b = ctx.create_mint();
    let vault_ta_b = ctx.create_token_account(&mint_b, &vault_authority);
    let admin = ctx.new_funded_signer();
    let foreign_vault = ctx.init_vault(&admin, &mint_b, &vault_ta_b);

    // The cranker just needs a ledger to exist.
    let cranker = ctx.new_funded_signer();
    ctx.init_collateral(&cranker);

    // Empty round → Accumulating, completeness 0 == 0.
    ctx.process_chunk(&pdas, 0, 32);
    assert_eq!(ctx.market(&pdas).phase, PHASE_ACCUMULATING);

    // Finalize with the foreign vault → rejected by the market↔mint binding
    // (the vault is canonical for mint B, so Tier-1 validate_self passes).
    assert!(
        ctx.try_finalize_clear_with_fee_vault(&pdas, &cranker, foreign_vault)
            .is_err(),
        "a vault for a different mint must be rejected",
    );
    // The failed finalize reverts atomically — the market is not bricked.
    assert_eq!(
        ctx.market(&pdas).phase,
        PHASE_ACCUMULATING,
        "market still Accumulating after the rejected finalize",
    );
}

#[test]
fn finalize_clear_crank_fee_rejects_foreign_cranker_ledger() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    ctx.market_fee_bps = 100; // fees accrue to insurance
    ctx.market_crank_fee = 3;
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    let vault = ctx.init_vault(&admin, &mint, &vault_ta);

    let a = ctx.new_funded_signer();
    let b = ctx.new_funded_signer();
    for t in [&a, &b] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }
    let cranker = ctx.new_funded_signer();
    ctx.init_collateral(&cranker);

    // Round 1: open at 20 → settle fees accrue to insurance (so round 2's crank
    // fee path actually pays, reaching the ledger check). Bid auction: A maker-buys
    // (quote book), B taker-sells (slab).
    ctx.post_maker_order(&pdas, &a, SIDE_BUY, 20, 10);
    let b_sell = ctx.submit_order(&pdas, &b, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &a.pubkey());
    ctx.finalize_clear_with_fee(&pdas, &cranker);
    ctx.settle_maker_quote(&pdas, &a.pubkey());
    ctx.settle_fill_with_margin(&pdas, b_sell, &b.pubkey());
    assert!(ctx.vault().insurance_balance > 0, "fees funded insurance");
    ctx.start_auction(&pdas);

    // Round 2: finalize with the correct vault but a FOREIGN cranker ledger (owned
    // by A, not the cranker). With insurance > 0 the crank fee path runs and must
    // reject the mismatched-owner ledger rather than credit it. Bid auction: A
    // taker-sells (slab), B maker-buys (quote book).
    //
    // First deactivate A's round-1 quote during the fresh Collect window (quotes
    // persist across rounds; clearing is only legal in Collect, and the new
    // sequence must exceed the quote's current one) so it doesn't block round 2's
    // maker completeness.
    ctx.clear_maker_quote(&pdas, &a, 2);
    ctx.submit_order(&pdas, &a, SIDE_SELL, 20, 10);
    ctx.post_maker_order(&pdas, &b, SIDE_BUY, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &b.pubkey());
    let (foreign_ledger, _) = ctx.collateral_pda(&a.pubkey());
    assert!(
        ctx.try_finalize_clear_fee_accounts(&pdas, &cranker, foreign_ledger, vault)
            .is_err(),
        "a cranker ledger owned by someone else must be rejected",
    );
}
