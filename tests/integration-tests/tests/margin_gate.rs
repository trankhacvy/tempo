//! A margin-enabled market must require the collateral ledger on every
//! non-zero fill, so a position can never grow without locked margin. A no-margin
//! market (maintenance_margin_bps == 0) keeps the ledger optional.

use tempo_integration_tests::*;

#[test]
fn settle_requires_collateral_when_margin_enabled() {
    let mut ctx = TestContext::new();
    ctx.market_maint_bps = 500;
    let pdas = ctx.init_market(1, 32, 8);

    let mint = ctx.create_mint();
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_ta = ctx.create_token_account(&mint, &vault_authority);
    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_ta);

    // Owner (taker buyer) + counterparty (maker seller), each funded for margin.
    // The owner is the *taker* whose `settle_fill` path is under test; the maker
    // liquidity comes from the quote book (§1.3, ask auction: taker-buy vs
    // maker-sell).
    let owner = ctx.new_funded_signer();
    let seller = ctx.new_funded_signer();
    for t in [&owner, &seller] {
        ctx.init_collateral(t);
        let ta = ctx.create_token_account(&mint, &t.pubkey());
        ctx.mint_to(&mint, &ta, 1000);
        ctx.deposit(t, &vault_ta, &ta, 1000);
        ctx.init_position(&pdas, t);
    }

    let buy_id = ctx.submit_order(&pdas, &owner, SIDE_BUY, 20, 10);
    ctx.post_maker_order(&pdas, &seller, SIDE_SELL, 20, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &seller.pubkey());
    ctx.finalize_clear(&pdas);

    // Position attached but NO collateral ledger → a non-zero fill is rejected.
    assert!(
        ctx.try_settle_fill_with_position(&pdas, buy_id, &owner)
            .is_err(),
        "a margin market must reject a non-zero fill without the collateral ledger",
    );
    // The order is untouched — nothing grew without margin.
    let order = ctx
        .orders(&pdas)
        .into_iter()
        .find(|o| o.order_id == buy_id)
        .unwrap();
    assert_eq!(order.status, STATUS_ACCUMULATED, "order not consumed");

    // Supplying the collateral ledger settles and locks margin.
    ctx.settle_fill_with_margin(&pdas, buy_id, &owner.pubkey());
    ctx.settle_maker_quote(&pdas, &seller.pubkey());
    assert_eq!(
        ctx.user_collateral(&owner.pubkey()).locked,
        10,
        "initial_margin(10, 20, 500) = 10 locked",
    );
}

#[test]
fn settle_position_only_when_maintenance_bps_zero() {
    // Default market_maint_bps is 0 → a no-margin clearing market.
    let mut ctx = TestContext::new();
    let pdas = ctx.init_market(10, 16, 64);

    // Ask auction: taker-buy (slab) vs maker-sell (quote book). The taker is the
    // order whose collateral-free `settle_fill` is under test; the maker needs a
    // Position but, on a 0-bps market, no collateral.
    let buyer = ctx.new_funded_signer();
    let seller = ctx.new_funded_signer();
    ctx.init_position(&pdas, &seller);
    let buy_id = ctx.submit_order(&pdas, &buyer, SIDE_BUY, 40, 20);
    ctx.post_maker_order(&pdas, &seller, SIDE_SELL, 40, 20);
    ctx.process_chunk(&pdas, 0, 64);
    ctx.process_maker_quote(&pdas, &seller.pubkey());
    ctx.finalize_clear(&pdas);

    // No collateral ledger needed: a non-zero fill settles position-only.
    let (_m, fill) = ctx.settle_fill(&pdas, buy_id);
    assert_eq!(
        fill, 20,
        "non-zero fill settles without collateral on a 0-bps market"
    );
    // The maker side settles position-only too (no collateral/vault).
    ctx.settle_maker_quote_clearing(&pdas, &seller.pubkey());
    let (spos, _) = ctx.position_pda(&pdas, &seller.pubkey());
    let fill_s = (-ctx.position(&spos).size) as u64;
    assert_eq!(fill_s, 20);
}
