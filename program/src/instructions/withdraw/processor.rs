use pinocchio::{
    account::AccountView,
    cpi::{Seed, Signer},
    Address, ProgramResult,
};
use pinocchio_token::{instructions::Transfer, state::Account as TokenAccount};

use crate::{
    errors::TempoProgramError,
    instructions::Withdraw,
    state::{UserCollateral, Vault},
    traits::AccountDeserialize,
};

/// Processes the Withdraw instruction: debits the owner's free
/// collateral (fails if it would dip into locked margin), then transfers the
/// tokens out of the vault token account with the vault authority PDA signing.
///
/// The ledger must be scoped to the vault's `collateral_mint` (CR-3) and the
/// destination token account must hold that same mint (HS-12), so a balance can
/// never be drained against the wrong (e.g. more valuable) per-mint vault. Only
/// classic SPL Token (no transfer fee) mints are supported (token program pinned in
/// `accounts.rs`).
pub fn process_withdraw(
    _program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = Withdraw::try_from((instruction_data, accounts))?;
    let amount = ix.data.amount;

    // Read the vault: confirm the token account + capture the authority bump + mint.
    let (authority_bump, collateral_mint) = {
        let vault_data = ix.accounts.vault.try_borrow()?;
        let vault = Vault::from_bytes(&vault_data)?;
        if vault.vault_token_account != *ix.accounts.vault_token_account.address() {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
        (vault.authority_bump, vault.collateral_mint)
    };

    // HS-12: the destination token account must hold the vault's collateral mint.
    {
        let user_token = TokenAccount::from_account_view(ix.accounts.user_token_account)?;
        if *user_token.mint() != collateral_mint {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
    }

    // The ledger must belong to the signing owner AND be scoped to the vault's mint
    // (CR-3); debit free collateral.
    {
        let mut acct = *ix.accounts.user_collateral;
        let mut uc_data = acct.try_borrow_mut()?;
        let uc = UserCollateral::from_bytes_mut(&mut uc_data)?;
        if uc.owner != *ix.accounts.owner.address() || uc.collateral_mint != collateral_mint {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
        uc.debit(amount)?;
    }

    // Move tokens out of the vault (authority PDA-signed). All borrows dropped.
    let bump = [authority_bump];
    let signer_seeds: [Seed; 2] = [Seed::from(Vault::AUTHORITY_PREFIX), Seed::from(&bump)];
    let signer = Signer::from(&signer_seeds);
    Transfer::new(
        ix.accounts.vault_token_account,
        ix.accounts.user_token_account,
        ix.accounts.vault_authority,
        amount,
    )
    .invoke_signed(&[signer])?;

    Ok(())
}
