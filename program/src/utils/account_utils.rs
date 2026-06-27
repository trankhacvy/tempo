//! Account validation utilities.
//!
//! # Security Note: Closed Account Discriminator Pattern
//!
//! This program does NOT implement the closed account discriminator pattern (setting first byte
//! to 0xff before closing). This is intentional because all account access validates ownership
//! via `verify_owned_by()`. After closing, accounts are assigned to the system program, so any
//! attempt to use a "revived" account fails the ownership check.

use crate::ID as TEMPO_PROGRAM_ID;
use pinocchio::{account::AccountView, address::Address, error::ProgramError};

/// Verify account as writable, returning an error if it is expected to be writable but is not.
#[inline(always)]
pub fn verify_writable(account: &AccountView, expect_writable: bool) -> Result<(), ProgramError> {
    if expect_writable && !account.is_writable() {
        return Err(ProgramError::Immutable);
    }
    Ok(())
}

/// Verify account is read-only, returning an error if it is writable.
///
/// Enforces that accounts declared as read-only in the IDL are actually passed as read-only,
/// preventing accidental mutations even if the processor has bugs.
#[inline(always)]
pub fn verify_readonly(account: &AccountView) -> Result<(), ProgramError> {
    if account.is_writable() {
        return Err(ProgramError::AccountBorrowFailed);
    }
    Ok(())
}

/// Verify account as a signer, returning an error if it is not (and writable if expected).
#[inline(always)]
pub fn verify_signer(account: &AccountView, expect_writable: bool) -> Result<(), ProgramError> {
    if !account.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    verify_writable(account, expect_writable)
}

/// Verify account's owner, returning an error if it is not the expected owner.
#[inline(always)]
pub fn verify_owned_by(account: &AccountView, owner: &Address) -> Result<(), ProgramError> {
    if !account.owned_by(owner) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    Ok(())
}

/// Verify account is a system account, returning an error if it is not.
#[inline(always)]
pub fn verify_system_account(account: &AccountView) -> Result<(), ProgramError> {
    verify_owned_by(account, &pinocchio_system::ID)
}

/// Verify account is owned by the current program, returning an error if it is not.
#[inline(always)]
pub fn verify_current_program_account(account: &AccountView) -> Result<(), ProgramError> {
    verify_owned_by(account, &TEMPO_PROGRAM_ID)
}
