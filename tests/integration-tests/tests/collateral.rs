//! Collateral money path (deposit / withdraw).
//!
//! Drives the full SPL-token plumbing: a 0-decimal collateral mint, a vault
//! token account owned by the vault-authority PDA (`[b"vault_authority"]`), and a
//! user token account. Asserts that deposits move tokens into the vault and
//! credit the ledger, withdrawals do the reverse, and a withdraw exceeding free
//! balance fails.

use tempo_integration_tests::*;

/// Spin up a vault + a funded trader with a token account holding `funding`
/// tokens. Returns (ctx, owner, vault_token_account, user_token_account).
fn setup(
    funding: u64,
) -> (
    TestContext,
    solana_sdk::signature::Keypair,
    solana_sdk::pubkey::Pubkey,
    solana_sdk::pubkey::Pubkey,
) {
    let mut ctx = TestContext::new();

    let mint = ctx.create_mint();

    // Vault token account must be owned by the vault-authority PDA.
    let (vault_authority, _) = ctx.vault_authority_pda();
    let vault_token_account = ctx.create_token_account(&mint, &vault_authority);

    let admin = ctx.new_funded_signer();
    ctx.init_vault(&admin, &mint, &vault_token_account);

    // Trader: ledger + token account funded with `funding` tokens.
    let owner = ctx.new_funded_signer();
    ctx.init_collateral(&owner);
    let user_token_account = ctx.create_token_account(&mint, &owner.pubkey());
    ctx.mint_to(&mint, &user_token_account, funding);

    (ctx, owner, vault_token_account, user_token_account)
}

#[test]
fn deposit_then_withdraw_money_path() {
    let (mut ctx, owner, vault_ta, user_ta) = setup(1_000);

    // --- deposit 1_000 ---
    assert_eq!(ctx.token_balance(&user_ta), 1_000);
    assert_eq!(ctx.token_balance(&vault_ta), 0);

    ctx.deposit(&owner, &vault_ta, &user_ta, 1_000);

    assert_eq!(
        ctx.user_collateral(&owner.pubkey()).balance,
        1_000,
        "ledger credited"
    );
    assert_eq!(
        ctx.token_balance(&vault_ta),
        1_000,
        "vault token account rose by 1000"
    );
    assert_eq!(ctx.token_balance(&user_ta), 0, "user tokens moved out");

    // --- withdraw 400 ---
    ctx.withdraw(&owner, &vault_ta, &user_ta, 400);

    let uc = ctx.user_collateral(&owner.pubkey());
    assert_eq!(uc.balance, 600, "ledger debited to 600");
    assert_eq!(uc.locked, 0, "nothing locked");
    assert_eq!(
        ctx.token_balance(&user_ta),
        400,
        "400 tokens returned to user"
    );
    assert_eq!(
        ctx.token_balance(&vault_ta),
        600,
        "vault holds the remaining 600"
    );
}

#[test]
fn withdraw_over_free_balance_fails() {
    let (mut ctx, owner, vault_ta, user_ta) = setup(1_000);
    ctx.deposit(&owner, &vault_ta, &user_ta, 1_000);

    // Free balance is 1_000; asking for 1_001 must fail (InsufficientCollateral).
    let res = ctx.try_withdraw(&owner, &vault_ta, &user_ta, 1_001);
    assert!(res.is_err(), "withdraw exceeding free balance must fail");

    // State unchanged after the failed withdraw.
    assert_eq!(ctx.user_collateral(&owner.pubkey()).balance, 1_000);
    assert_eq!(ctx.token_balance(&vault_ta), 1_000);
}
