use pinocchio::{
    account::AccountView,
    sysvars::{clock::Clock, Sysvar},
    Address, ProgramResult,
};

use crate::{
    errors::TempoProgramError,
    events::AuthorityTransferredEvent,
    instructions::AcceptAuthorityTransfer,
    state::Market,
    traits::{AccountDeserialize, EventSerialize},
    utils::emit_event,
};

/// Processes AcceptAuthorityTransfer (plan.md §3.3): the STAGED new authority
/// signs to take over — a transfer to a dead/typo'd address can never complete.
pub fn process_accept_authority_transfer(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = AcceptAuthorityTransfer::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();
    let now_slot = Clock::get()?.slot;

    let (old_authority, new_authority) = {
        let mut acct = *ix.accounts.market;
        let mut market_data = acct.try_borrow_mut()?;
        {
            Market::from_account(&market_data, ix.accounts.market, program_id)?;
        }
        let market = Market::from_bytes_mut(&mut market_data)?;
        let payload = market.take_pending(Market::PENDING_AUTHORITY, now_slot)?;
        let staged = Address::new_from_array(payload[0..32].try_into().unwrap());
        if staged != *ix.accounts.new_authority.address() {
            return Err(TempoProgramError::InvalidAuthority.into());
        }
        let old = market.authority;
        market.authority = staged;
        (old, staged)
    };

    let event = AuthorityTransferredEvent {
        market: market_key,
        old_authority,
        new_authority,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )
}
