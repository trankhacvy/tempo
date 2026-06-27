//! Phase 3: drive the reference liquidator's pure decision engine
//! (`tempo_liquidator::engine`) + the SDK liquidate builders
//! (`tempo_sdk::ix::liquidate` / `liquidate_cross`) against the REAL program in
//! LiteSVM. Proves the off-chain liquidatable gate agrees with the on-chain one
//! (priced off the same raw oracle via `tempo_math::oracle`) and that the SDK
//! instruction actually closes the position, pays the penalty, and conserves
//! `vault == Σ balances + insurance`.

use tempo_integration_tests::*;

use solana_sdk::pubkey::Pubkey;

use tempo_liquidator::engine;
use tempo_liquidator::snapshot::{Candidate, CrossMember};
use tempo_sdk::accounts::{MarginAccountView, MarketView, PositionView};
use tempo_sdk::ix::{self, LiquidateCrossParams, LiquidateParams};

const MAINT_BPS: u16 = 500;

/// Resolve a market's raw solvency mark exactly as the liquidator does.
fn raw_mark(ctx: &TestContext, oracle: &Pubkey, feed_id: &[u8; 32]) -> u64 {
    let data = ctx.raw_account(oracle).expect("oracle account");
    tempo_math::oracle::read_price(
        &data,
        feed_id,
        ctx.clock_ts(),
        tempo_math::oracle::MAX_AGE_SECS,
    )
    .expect("fresh price")
    .price_1e8
}

#[test]
fn engine_decides_and_sdk_liquidates_isolated() {
    let mut ctx = TestContext::new();
    let oracle = Pubkey::new_unique();
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

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    // Owner opens a long 10 @ 30 (locks initial margin 15).
    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    let market = MarketView::decode(&ctx.raw_account(&pdas.market).expect("market")).expect("mkt");
    let params = |ctx: &TestContext| LiquidateParams {
        liquidator: liquidator.pubkey(),
        market: pdas.market,
        oracle,
        position,
        user_collateral: ctx.collateral_pda(&owner.pubkey()).0,
        vault: ctx.vault_pda().0,
        liquidator_collateral: ctx.collateral_pda(&liquidator.pubkey()).0,
    };
    let candidate = |ctx: &TestContext| {
        let view = PositionView::decode(&ctx.raw_account(&position).expect("pos")).expect("pos");
        Candidate {
            key: position,
            view,
            market: pdas.market,
            oracle,
            mark: raw_mark(ctx, &oracle, &market.oracle_feed_id),
            maintenance_bps: market.maintenance_margin_bps,
        }
    };

    // Healthy at 31: the engine declines and the program rejects the SDK ix.
    ctx.set_oracle(&oracle, 31, -8);
    assert!(
        !engine::isolated_liquidatable(&candidate(&ctx)),
        "engine must not flag a healthy position"
    );
    assert!(
        ctx.send_ix(ix::liquidate(&params(&ctx)), &[&liquidator])
            .is_err(),
        "program rejects liquidating a healthy position"
    );
    assert_eq!(ctx.position(&position).size, 10, "untouched");

    // Underwater at 29: the engine flags it and the SDK ix closes it.
    let insurance_before = ctx.vault().insurance_balance;
    ctx.set_oracle(&oracle, 29, -8);
    assert!(
        engine::isolated_liquidatable(&candidate(&ctx)),
        "engine flags the underwater position"
    );
    ctx.send_ix(ix::liquidate(&params(&ctx)), &[&liquidator])
        .expect("sdk liquidate lands");

    let p = ctx.position(&position);
    assert_eq!(p.size, 0, "position closed");
    assert_eq!(p.collateral, 0);
    assert_eq!(
        ctx.user_collateral(&liquidator.pubkey()).balance,
        2,
        "liquidator earns the penalty"
    );
    assert_eq!(
        ctx.vault().insurance_balance,
        insurance_before + 10,
        "owner's realized loss flows to insurance"
    );
}

#[test]
fn engine_decides_and_sdk_liquidates_cross() {
    let mut ctx = TestContext::new();
    let oracle = Pubkey::new_unique();
    ctx.set_clock_ts(1_700_000_000);
    ctx.market_maint_bps = MAINT_BPS;
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
        .expect("add to margin");

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    ctx.mint_to(&mint, &seller_ta, 100);
    ctx.deposit(&seller, &vault_ta, &seller_ta, 100);
    ctx.init_position(&pdas, &seller);

    let liquidator = ctx.new_funded_signer();
    ctx.init_collateral(&liquidator);

    ctx.post_maker_order(&pdas, &owner, SIDE_BUY, 30, 10);
    let sell_id = ctx.submit_order(&pdas, &seller, SIDE_SELL, 30, 10);
    ctx.process_chunk(&pdas, 0, 8);
    ctx.process_maker_quote(&pdas, &owner.pubkey());
    ctx.finalize_clear(&pdas);
    ctx.settle_maker_quote(&pdas, &owner.pubkey());
    ctx.settle_fill_with_margin(&pdas, sell_id, &seller.pubkey());

    // Crash to 29: combined equity (20 - 10) = 10 < maintenance (10*29*5% = 14).
    ctx.set_oracle(&oracle, 29, -8);

    // Resolve the cross account exactly as `snapshot::resolve_cross` would, then let
    // the engine decide. The `MarginAccountView` decoder is exercised against real
    // on-chain bytes here.
    let margin_pda = tempo_sdk::pda::margin_account(&owner.pubkey()).0;
    let margin =
        MarginAccountView::decode(&ctx.raw_account(&margin_pda).expect("margin")).expect("margin");
    assert_eq!(margin.members.len(), 1, "one grouped member");
    let member_key = margin.members[0];
    let pos = PositionView::decode(&ctx.raw_account(&member_key).expect("pos")).expect("pos");
    assert_eq!(pos.margin_mode, 1, "position is cross-mode");
    let market = MarketView::decode(&ctx.raw_account(&pos.market).expect("mkt")).expect("mkt");
    let member = CrossMember {
        position: member_key,
        size: pos.size,
        entry_price: pos.entry_price,
        realized_pnl: pos.realized_pnl,
        market: pos.market,
        oracle: market.oracle,
        mark: raw_mark(&ctx, &oracle, &market.oracle_feed_id),
        maintenance_bps: market.maintenance_margin_bps,
    };
    let balance = ctx.user_collateral(&owner.pubkey()).balance;
    let legs = engine::cross_liquidatable(balance, &[member]).expect("engine flags the account");

    let vault_tokens = ctx.token_balance(&vault_ta);
    let params = LiquidateCrossParams {
        liquidator: liquidator.pubkey(),
        margin_account: margin_pda,
        user_collateral: ctx.collateral_pda(&owner.pubkey()).0,
        vault: ctx.vault_pda().0,
        liquidator_collateral: ctx.collateral_pda(&liquidator.pubkey()).0,
    };
    ctx.send_ix(ix::liquidate_cross(&params, &legs), &[&liquidator])
        .expect("sdk cross liquidation lands");

    assert_eq!(ctx.position(&position).size, 0, "member closed");
    assert!(
        ctx.user_collateral(&liquidator.pubkey()).balance > 0,
        "liquidator paid a penalty"
    );
    let sum = ctx.user_collateral(&owner.pubkey()).balance
        + ctx.user_collateral(&seller.pubkey()).balance
        + ctx.user_collateral(&liquidator.pubkey()).balance
        + ctx.vault().insurance_balance;
    assert_eq!(sum, vault_tokens, "vault == Σ balances + insurance");
}
