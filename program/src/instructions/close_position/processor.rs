use pinocchio::{account::AccountView, Address, ProgramResult};

use crate::{
    errors::TempoProgramError,
    instructions::ClosePosition,
    state::Position,
    traits::{AccountDeserialize, PdaAccount},
    utils::close_pda_account,
};

/// Processes ClosePosition (missing-features §3.4): closes a FLAT, fully
/// drained `Position` PDA and refunds its rent to the owner — the exit half of
/// `init_position`, so a trader who is done with a market gets their rent back.
///
/// Guards (all must hold, else the close is rejected):
///  * the signer is the position's owner (`InvalidOrderOwner`);
///  * `size == 0` — no open exposure (a live position must be closed by trading
///    or liquidation first, never by deleting the account);
///  * `collateral == 0` — no margin still parked on the position (it flows back
///    to the ledger at the last flattening settle);
///  * `realized_pnl == 0` — no PnL still waiting to be flushed to the ledger
///    (deleting it would silently burn the owner's gain or erase their loss);
///  * `margin_mode == 0` (isolated) — a cross-group member's account is part of
///    the group's solvency set; it must leave the group first
///    (`remove_position_from_margin` resets the mode).
pub fn process_close_position(
    program_id: &Address,
    accounts: &[AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    let ix = ClosePosition::try_from((instruction_data, accounts))?;
    let owner = *ix.accounts.owner.address();

    {
        let pos_data = ix.accounts.position.try_borrow()?;
        let position = Position::from_bytes(&pos_data)?;
        position.validate_self(ix.accounts.position, program_id)?;
        if position.owner != owner {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        // Flat and drained: nothing of value may live in the account it closes.
        if position.size() != 0 || position.collateral() != 0 || position.realized_pnl() != 0 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }
        // Cross-margin members are part of the group's solvency set — leaving
        // the group (remove_position_from_margin) must come first.
        if position.margin_mode != 0 {
            return Err(TempoProgramError::InvalidOrderStatus.into());
        }
    }

    close_pda_account(ix.accounts.position, ix.accounts.owner)
}
