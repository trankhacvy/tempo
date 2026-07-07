use pinocchio::{account::AccountView, Address, ProgramResult};
use pinocchio_token::{instructions::Transfer, state::Account as TokenAccount};

use crate::{
    errors::TempoProgramError,
    events::InsuranceSeededEvent,
    instructions::SeedInsurance,
    state::Vault,
    traits::{AccountDeserialize, EventSerialize, PdaAccount},
    utils::emit_event,
};

/// Processes SeedInsurance (missing-features §4.1, permissionless donate):
/// transfers `amount` from the donor's token account into the vault token
/// account and credits `Vault.insurance_balance` by the same face amount.
///
/// Conservation is trivial: both sides of the backing invariant
/// (`vault_token ≥ Σ balances + insurance`) grow together, and
/// `total_user_balance` is untouched — this is pool money, not a user claim.
/// Face-amount crediting is valid for the same reason as `deposit` (the token
/// program is pinned, HS-12 — no fee-on-transfer mints).
///
/// Why this exists: a fresh money-path market has a zero pool, so the FIRST
/// profitable maker settle fails `InsuranceInsolvent` and the round deadlocks
/// (losses that would fill the pool need the roll the keeper correctly
/// withholds) — reproduced live on devnet, see plan.md P0.6. Seeding breaks the
/// deadlock without weakening the fail-closed gate.
pub fn process_seed_insurance(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = SeedInsurance::try_from((instruction_data, accounts))?;
    let amount = ix.data.amount;
    let donor = *ix.accounts.donor.address();

    // The provided vault token account must be the one the vault records, the
    // vault must be the canonical PDA for its own mint, and the donor's token
    // account must hold that mint (HS-12) so the credited amount corresponds to
    // the asset the vault actually backs.
    let collateral_mint = {
        let vault_data = ix.accounts.vault.try_borrow()?;
        let vault = Vault::from_bytes(&vault_data)?;
        vault.validate_self(ix.accounts.vault, program_id)?;
        if vault.vault_token_account != *ix.accounts.vault_token_account.address() {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
        vault.collateral_mint
    };
    {
        let donor_token = TokenAccount::from_account_view(ix.accounts.donor_token_account)?;
        if *donor_token.mint() != collateral_mint {
            return Err(TempoProgramError::InvalidCollateralAccount.into());
        }
    }

    // Move tokens (donor-signed). All program-account borrows are dropped above.
    Transfer::new(
        ix.accounts.donor_token_account,
        ix.accounts.vault_token_account,
        ix.accounts.donor,
        amount,
    )
    .invoke()?;

    // Credit the pool by the face amount.
    {
        let mut acct = *ix.accounts.vault;
        let mut vault_data = acct.try_borrow_mut()?;
        let vault = Vault::from_bytes_mut(&mut vault_data)?;
        vault.set_insurance_balance(
            vault
                .insurance_balance()
                .checked_add(amount)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
    }

    let event = InsuranceSeededEvent {
        collateral_mint,
        donor,
        amount,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )
}
