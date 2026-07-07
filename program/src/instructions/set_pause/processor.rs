use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    events::MarketPauseChangedEvent,
    instructions::SetPause,
    state::Market,
    traits::{AccountDeserialize, EventSerialize},
    utils::emit_event,
};

/// Processes SetPause (missing-features §3.2): sets the market's pause bitflags.
///
/// Authority-gated and IMMEDIATE (an emergency circuit breaker must not sit
/// behind a timelock; *un*pausing carries no such urgency but shares the path
/// for simplicity). Only intake-side instructions check the flags —
/// `submit_order` and the maker-quote writes reject on `PAUSE_INTAKE`,
/// `start_auction` on `PAUSE_ROLL`. Cancels, cranks, settles, withdrawals, and
/// liquidations are deliberately unguarded, so the in-flight round always drains
/// and users can always exit: a pause can never trap funds.
pub fn process_set_pause(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = SetPause::try_from((instruction_data, accounts))?;
    let market_key = *ix.accounts.market.address();

    {
        let mut acct = *ix.accounts.market;
        let mut market_data = acct.try_borrow_mut()?;
        {
            let market = Market::from_account(&market_data, ix.accounts.market, program_id)?;
            market.validate_authority(ix.accounts.authority.address())?;
        }
        Market::from_bytes_mut(&mut market_data)?.paused = ix.data.paused;
    }

    let event = MarketPauseChangedEvent {
        market: market_key,
        paused: ix.data.paused,
    };
    emit_event(
        program_id,
        ix.accounts.event_authority,
        ix.accounts.tempo_program,
        &event.to_bytes(),
    )
}
