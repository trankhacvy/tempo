use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_signer, verify_writable},
};

/// Accounts for LiquidateCross (account-level liquidation).
///
/// # Account layout
/// ```text
/// 0  [signer]   liquidator
/// 1  []         margin_account
/// 2  [writable] user_collateral (owner's shared ledger)
/// 3  [writable] vault
/// 4  [writable] liquidator_collateral
/// 5  []         event_authority
/// 6  []         tempo_program
/// 7+ one entry per member, in `live_mask` order: a *live* member is a
///    `(position, market, oracle)` triple, a *flat* member (size 0) is a bare
///    `position` account (no market/oracle needed — known-issues §2.4). The close
///    target is the first non-flat member (written). The oracle is the market's bound
///    Pyth account — solvency is priced off it raw, not the braked effective price
///    (known-issues §2.2). At least one live target triple is always present.
/// ```
pub struct LiquidateCrossAccounts<'a> {
    pub liquidator: &'a AccountView,
    pub margin_account: &'a AccountView,
    pub user_collateral: &'a AccountView,
    pub vault: &'a AccountView,
    pub liquidator_collateral: &'a AccountView,
    pub event_authority: &'a AccountView,
    pub tempo_program: &'a AccountView,
    /// Trailing `(position, market, oracle)` triples — target first.
    pub members: &'a [AccountView],
}

impl<'a> TryFrom<&'a [AccountView]> for LiquidateCrossAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        // 7 fixed + at least one member account. The floor is one *bare* member (not a
        // full triple) so an all-flat group — which supplies only bare positions and
        // is un-liquidatable — reaches the processor's combined-health check and gets
        // the semantic `NotLiquidatable`, rather than aborting here as
        // `NotEnoughAccountKeys` (known-issues §2.9c). The processor's exact
        // `live_mask` length check still rejects a genuinely short account list.
        if accounts.len() < 8 {
            return Err(ProgramError::NotEnoughAccountKeys);
        }
        let (fixed, members) = accounts.split_at(7);
        let [liquidator, margin_account, user_collateral, vault, liquidator_collateral, event_authority, tempo_program] =
            fixed
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(liquidator, false)?;
        verify_writable(user_collateral, true)?;
        verify_writable(vault, true)?;
        verify_writable(liquidator_collateral, true)?;

        Ok(Self {
            liquidator,
            margin_account,
            user_collateral,
            vault,
            liquidator_collateral,
            event_authority,
            tempo_program,
            members,
        })
    }
}

impl<'a> InstructionAccounts<'a> for LiquidateCrossAccounts<'a> {}
