use pinocchio::{account::AccountView, error::ProgramError};

use crate::{
    traits::InstructionAccounts,
    utils::{verify_current_program_account, verify_signer, verify_writable},
};

/// Accounts for WithdrawCross (cross-margin extraction).
///
/// # Account layout
/// ```text
/// 0  [signer]   owner
/// 1  []         margin_account (owner's group / member set)
/// 2  [writable] user_collateral (shared ledger to debit)
/// 3  []         vault
/// 4  []         vault_authority (signs the token transfer)
/// 5  [writable] vault_token_account
/// 6  [writable] user_token_account
/// 7  []         token_program
/// 8+ one entry per member, in member order matching `live_mask`: a *live* member is
///    a `(position, market, oracle)` triple, a *flat* member (size 0) is a bare
///    `position` account (no market/oracle needed — known-issues §2.4). The oracle is
///    the market's bound Pyth account — combined health is priced off it raw, not the
///    braked effective price (known-issues §2.2).
/// ```
pub struct WithdrawCrossAccounts<'a> {
    pub owner: &'a AccountView,
    pub margin_account: &'a AccountView,
    pub user_collateral: &'a AccountView,
    pub vault: &'a AccountView,
    pub vault_authority: &'a AccountView,
    pub vault_token_account: &'a AccountView,
    pub user_token_account: &'a AccountView,
    pub token_program: &'a AccountView,
    /// Trailing `(position, market, oracle)` triples — one per group member.
    pub members: &'a [AccountView],
}

impl<'a> TryFrom<&'a [AccountView]> for WithdrawCrossAccounts<'a> {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(accounts: &'a [AccountView]) -> Result<Self, Self::Error> {
        if accounts.len() < 8 {
            return Err(ProgramError::NotEnoughAccountKeys);
        }
        let (fixed, members) = accounts.split_at(8);
        let [owner, margin_account, user_collateral, vault, vault_authority, vault_token_account, user_token_account, token_program] =
            fixed
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        verify_signer(owner, false)?;
        verify_writable(user_collateral, true)?;
        verify_current_program_account(user_collateral)?;
        verify_current_program_account(vault)?;
        verify_writable(vault_token_account, true)?;
        verify_writable(user_token_account, true)?;

        Ok(Self {
            owner,
            margin_account,
            user_collateral,
            vault,
            vault_authority,
            vault_token_account,
            user_token_account,
            token_program,
            members,
        })
    }
}

impl<'a> InstructionAccounts<'a> for WithdrawCrossAccounts<'a> {}
