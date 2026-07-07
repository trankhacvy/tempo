use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    instructions::ProposeInsuranceWithdraw,
    state::{Market, Vault},
    traits::{AccountDeserialize, PdaAccount},
};

/// Processes ProposeInsuranceWithdraw (plan.md §4.4): the vault authority
/// stages an insurance withdrawal behind the consensus-enforced delay (reuses
/// the market engine's constant — one delay policy). The amount is bounded by
/// the CURRENT pool at propose time and re-clamped at apply.
pub fn process_propose_insurance_withdraw(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ProposeInsuranceWithdraw::try_from((instruction_data, accounts))?;
    let now_slot = Clock::get()?.slot;

    let mut acct = *ix.accounts.vault;
    let mut vault_data = acct.try_borrow_mut()?;
    let vault = Vault::from_bytes_mut(&mut vault_data)?;
    vault.validate_self(ix.accounts.vault, program_id)?;
    if vault.authority != *ix.accounts.authority.address() {
        return Err(TempoProgramError::InvalidAuthority.into());
    }
    if ix.data.amount > vault.insurance_balance() {
        return Err(TempoProgramError::InsuranceInsolvent.into());
    }
    vault.set_pending_withdraw_amount(ix.data.amount);
    vault.set_pending_withdraw_slot(now_slot.saturating_add(Market::RISK_UPDATE_DELAY_SLOTS));
    Ok(())
}
