use alloc::vec::Vec;
use pinocchio::{account::AccountView, cpi::Seed, error::ProgramError, Address, ProgramResult};

use crate::{
    instructions::InitVault,
    state::Vault,
    traits::{AccountSerialize, AccountSize, PdaSeeds},
    utils::create_pda_account,
};

/// Processes the InitVault instruction (admin): creates the
/// global `Vault` singleton PDA. Trusts the admin to pass a correctly-owned
/// `vault_token_account` (owned by the vault authority PDA); its address is just
/// recorded here.
pub fn process_init_vault(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = InitVault::try_from((instruction_data, accounts))?;

    let collateral_mint = *ix.accounts.collateral_mint.address();
    let vault_token_account = *ix.accounts.vault_token_account.address();

    // v3: the admin signer is recorded as the vault authority (gates the
    // Phase-3 staged insurance withdraw).
    let vault = Vault::new(
        ix.data.vault_bump,
        ix.data.authority_bump,
        collateral_mint,
        vault_token_account,
        *ix.accounts.admin.address(),
    );
    vault.validate_pda(ix.accounts.vault, program_id, ix.data.vault_bump)?;

    let bump = [ix.data.vault_bump];
    let seeds: Vec<Seed> = vault.seeds_with_bump(&bump);
    let seeds_array: [Seed; 3] = seeds
        .try_into()
        .map_err(|_| ProgramError::InvalidArgument)?;
    create_pda_account(
        ix.accounts.payer,
        Vault::LEN,
        program_id,
        ix.accounts.vault,
        seeds_array,
    )?;

    {
        let mut acct = *ix.accounts.vault;
        let mut slice = acct.try_borrow_mut()?;
        vault.write_to_slice(&mut slice)?;
    }

    Ok(())
}
