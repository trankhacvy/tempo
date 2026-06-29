use pinocchio::{account::AccountView, Address, ProgramResult};
use pinocchio_token::{instructions::Transfer, state::Account as TokenAccount};

use crate::{
    errors::TempoProgramError,
    instructions::Deposit,
    state::{UserCollateral, Vault},
    traits::AccountDeserialize,
};

/// Processes the Deposit instruction: transfers `amount`
/// collateral from the owner's token account into the vault token account, then
/// credits the owner's `UserCollateral` ledger. The owner signs the SPL transfer.
///
/// Only classic SPL Token (no transfer fee) mints are supported: the ledger is
/// credited by the face `amount`, not by the actually-received `post − pre` balance
/// (the token-program id is pinned in `accounts.rs`, HS-12). The deposited token's
/// mint must equal the vault's `collateral_mint`, and the ledger must be scoped to
/// that same mint (CR-3) — so a balance can never be credited under the wrong mint.
pub fn process_deposit(
    _program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = Deposit::try_from((instruction_data, accounts))?;
    let amount = ix.data.amount;

    // The provided vault token account must be the one recorded in the vault;
    // capture the vault's collateral mint to scope the deposit (CR-3 / HS-12).
    let collateral_mint = {
        let vault_data = ix.accounts.vault.try_borrow()?;
        let vault = Vault::from_bytes(&vault_data)?;
        if vault.vault_token_account != *ix.accounts.vault_token_account.address() {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
        vault.collateral_mint
    };

    // The ledger must belong to the signing owner AND be scoped to the vault's mint
    // (CR-3): a mint-A ledger can never be credited from a mint-B vault.
    {
        let uc_data = ix.accounts.user_collateral.try_borrow()?;
        let uc = UserCollateral::from_bytes(&uc_data)?;
        if uc.owner != *ix.accounts.owner.address() || uc.collateral_mint != collateral_mint {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
    }

    // HS-12: the deposited token account must hold the vault's collateral mint, so
    // the credited `amount` corresponds to the asset the vault actually backs.
    {
        let user_token = TokenAccount::from_account_view(ix.accounts.user_token_account)?;
        if *user_token.mint() != collateral_mint {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
    }

    // Move tokens (owner-signed). All program account borrows are dropped above.
    Transfer::new(
        ix.accounts.user_token_account,
        ix.accounts.vault_token_account,
        ix.accounts.owner,
        amount,
    )
    .invoke()?;

    // Credit the ledger.
    {
        let mut acct = *ix.accounts.user_collateral;
        let mut uc_data = acct.try_borrow_mut()?;
        let uc = UserCollateral::from_bytes_mut(&mut uc_data)?;
        uc.credit(amount)?;
    }

    Ok(())
}
