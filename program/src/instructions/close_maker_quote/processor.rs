use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::CloseMakerQuote,
    state::MakerQuote,
    traits::{AccountDeserialize, PdaAccount},
    utils::close_pda_account,
};

/// Processes CloseMakerQuote: closes an already-cleared (`status == 0`)
/// `MakerQuote` PDA and refunds its rent to the maker, freeing the deterministic
/// `[b"maker_quote", market, maker]` address so the maker can `init_maker_quote`
/// again (known-issues §3 — `clear_maker_quote` deactivates but never closes,
/// trapping rent and locking the maker out of re-quoting).
///
/// Only the maker — not a write delegate — may close, and only an inactive quote:
/// an active quote is still folded into the histogram and counts toward the
/// `finalize_clear` completeness denominator, so it must be cleared first
/// (`clear_maker_quote`, which decrements `active_maker_quote_count`).
pub fn process_close_maker_quote(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = CloseMakerQuote::try_from((instruction_data, accounts))?;
    let maker = *ix.accounts.maker.address();

    {
        let quote_data = ix.accounts.maker_quote.try_borrow()?;
        let quote = MakerQuote::from_bytes(&quote_data)?;
        quote.validate_self(ix.accounts.maker_quote, program_id)?;
        if quote.maker != maker {
            return Err(TempoProgramError::InvalidAuthority.into());
        }
        if quote.status != 0 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }
    }

    close_pda_account(ix.accounts.maker_quote, ix.accounts.maker)
}
