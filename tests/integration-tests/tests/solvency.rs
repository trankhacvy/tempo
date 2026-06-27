//! The global solvency invariant: `vault token holdings == Σ user
//! balances + insurance`. The conserving money path floats PnL through insurance,
//! so liquidation never mints unbacked collateral and bad debt is surfaced, not
//! silently dropped. Each scenario asserts the invariant holds after the close.

use solana_sdk::pubkey::Pubkey;
use tempo_integration_tests::*;

const MAINT_BPS: u16 = 500;

/// `vault token holdings == Σ tracked balances + insurance` (the tracked users are
/// the only depositors, so equality must hold exactly).
fn assert_solvent(ctx: &TestContext, vault_ta: &Pubkey, users: &[Pubkey]) {
    let claims: u64 = users
        .iter()
        .map(|u| ctx.user_collateral(u).balance)
        .sum::<u64>()
        + ctx.vault().insurance_balance;
    assert_eq!(
        ctx.token_balance(vault_ta),
        claims,
        "vault tokens must equal Σ balances + insurance",
    );
}

/// Open a long (owner) against a short (seller), each margined, at price 30.
/// Returns (ctx, pdas, oracle, vault_ta, owner, seller, liquidator).
#[allow(clippy::type_complexity)]
fn open_long_vs_short() -> (
    TestContext,
    MarketPdas,
    Pubkey,
    Pubkey,
    solana_sdk::signature::Keypair,
    solana_sdk::signature::Keypair,
    solana_sdk::signature::Keypair,
) {
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

    // Owner deposits 100 and opens a long; seller deposits 15 to margin the short.
    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let owner_ta = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &owner_ta, 100);
    ctx.deposit(&owner, &vault_ta, &owner_ta, 100);
    ctx.init_position(&pdas, &owner);

    let seller = ctx.new_funded_signer();
    ctx.init_collateral(&seller);
    let seller_ta = ctx.create_token_account(&mint, &seller.pubkey());
    // A taker SELL reserves worst-case margin at submit (window top, missing-features
    // §1.1), released at settle; fund above the limit-price margin so the lock succeeds.
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

    (ctx, pdas, oracle, vault_ta, owner, seller, liquidator)
}

#[test]
fn solvent_liquidation_conserves() {
    let (mut ctx, pdas, oracle, vault_ta, owner, seller, liquidator) = open_long_vs_short();
    let users = [owner.pubkey(), seller.pubkey(), liquidator.pubkey()];
    assert_solvent(&ctx, &vault_ta, &users);

    // Oracle drops to 29: owner unrealized -10, equity 15-10 = 5 < maintenance
    // (10*29*5% = 14) → liquidatable but solvent (no bad debt).
    ctx.set_oracle(&oracle, 29, -8);
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());

    assert_eq!(
        ctx.position(&ctx.position_pda(&pdas, &owner.pubkey()).0)
            .size,
        0
    );
    // The 10-unit loss flowed to insurance; nothing minted, nothing leaked.
    assert_solvent(&ctx, &vault_ta, &users);
}

#[test]
fn bad_debt_liquidation_conserves_and_surfaces() {
    let (mut ctx, pdas, oracle, vault_ta, owner, seller, liquidator) = open_long_vs_short();
    let users = [owner.pubkey(), seller.pubkey(), liquidator.pubkey()];

    // Oracle crashes to 20: owner unrealized -100, equity 15-100 = -85 < 0 → bad
    // debt 85. Only the position collateral (15) is at risk; the owner keeps the
    // rest of their deposit. The 15 collected accrues to insurance; the 85 gap is
    // logged, never silently dropped (#5).
    ctx.set_oracle(&oracle, 20, -8);
    ctx.liquidate(&pdas, &oracle, &liquidator, &owner.pubkey());

    let owner_uc = ctx.user_collateral(&owner.pubkey());
    assert_eq!(owner_uc.locked, 0, "margin released");
    assert_eq!(
        owner_uc.balance, 85,
        "owner keeps the un-margined free balance"
    );
    assert_eq!(
        ctx.vault().insurance_balance,
        15,
        "owner collateral collected to insurance"
    );
    assert_eq!(
        ctx.position(&ctx.position_pda(&pdas, &owner.pubkey()).0)
            .size,
        0
    );

    // The close itself conserves: no claim was minted beyond the vault's tokens.
    assert_solvent(&ctx, &vault_ta, &users);
}
