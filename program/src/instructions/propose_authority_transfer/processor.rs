use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{instructions::ProposeAuthorityTransfer, state::Market, traits::AccountDeserialize};

/// Processes ProposeAuthorityTransfer (plan.md §3.3): stages the new authority.
/// Effective immediately — the safety is the two-step shape (the NEW key must
/// sign the accept), not a delay.
pub fn process_propose_authority_transfer(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ProposeAuthorityTransfer::try_from((instruction_data, accounts))?;
    let now_slot = Clock::get()?.slot;

    let mut acct = *ix.accounts.market;
    let mut market_data = acct.try_borrow_mut()?;
    {
        let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
        market.validate_authority(ix.accounts.authority.address())?;
    }
    Market::from_bytes_mut(&mut market_data)?.stage_pending(
        Market::PENDING_AUTHORITY,
        &ix.data.new_authority,
        now_slot,
    );
    Ok(())
}
