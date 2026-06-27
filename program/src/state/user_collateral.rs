use alloc::vec;
use alloc::vec::Vec;
use codama::CodamaAccount;
use pinocchio::{cpi::Seed, error::ProgramError, Address};

use crate::errors::TempoProgramError;
use crate::traits::{
    AccountDeserialize, AccountSerialize, AccountSize, Discriminator, PdaAccount, PdaSeeds,
    TempoAccountDiscriminators, Versioned,
};
use crate::{assert_no_padding, le_field};

/// A user's collateral ledger. `balance` is free (withdrawable)
/// collateral; `locked` is margin reserved against open positions.
///
/// # PDA Seeds
/// `[b"collateral", owner.as_ref()]`
///
/// # Zero-copy layout (`#[repr(C)]`, **alignment 1**)
/// Address (32) + 2 × [u8;8] (16) + u8 (1) = 49.
#[derive(Clone, Debug, PartialEq, CodamaAccount)]
#[codama(field("discriminator", number(u8), default_value = 7))]
#[codama(discriminator(field = "discriminator"))]
#[codama(seed(type = string(utf8), value = "collateral"))]
#[codama(seed(name = "owner", type = public_key))]
#[repr(C)]
pub struct UserCollateral {
    pub owner: Address,
    pub balance_le: [u8; 8],
    pub locked_le: [u8; 8],
    pub bump: u8,
}

assert_no_padding!(UserCollateral, 32 + 8 + 8 + 1);

impl Discriminator for UserCollateral {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::UserCollateralDiscriminator as u8;
}

impl Versioned for UserCollateral {
    const VERSION: u8 = 1;
}

impl AccountSize for UserCollateral {
    const DATA_LEN: usize = 32 + 8 + 8 + 1;
}

impl AccountDeserialize for UserCollateral {}

impl AccountSerialize for UserCollateral {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.owner.as_ref());
        data.extend_from_slice(&self.balance_le);
        data.extend_from_slice(&self.locked_le);
        data.push(self.bump);
        data
    }
}

impl PdaSeeds for UserCollateral {
    const PREFIX: &'static [u8] = b"collateral";

    #[inline(always)]
    fn seeds(&self) -> Vec<&[u8]> {
        vec![Self::PREFIX, self.owner.as_ref()]
    }

    #[inline(always)]
    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.owner.as_ref()),
            Seed::from(bump.as_slice()),
        ]
    }
}

impl PdaAccount for UserCollateral {
    #[inline(always)]
    fn bump(&self) -> u8 {
        self.bump
    }
}

impl UserCollateral {
    le_field!(balance, set_balance, balance_le, u64);
    le_field!(locked, set_locked, locked_le, u64);

    #[inline(always)]
    pub fn new(bump: u8, owner: Address) -> Self {
        Self {
            owner,
            balance_le: 0u64.to_le_bytes(),
            locked_le: 0u64.to_le_bytes(),
            bump,
        }
    }

    /// Free (withdrawable) collateral = balance − locked.
    #[inline(always)]
    pub fn free(&self) -> u64 {
        self.balance().saturating_sub(self.locked())
    }

    /// Credit deposited collateral.
    #[inline(always)]
    pub fn credit(&mut self, amount: u64) -> Result<(), ProgramError> {
        self.set_balance(
            self.balance()
                .checked_add(amount)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
        Ok(())
    }

    /// Debit withdrawn collateral; fails if it would dip into locked margin.
    #[inline(always)]
    pub fn debit(&mut self, amount: u64) -> Result<(), ProgramError> {
        if amount > self.free() {
            return Err(TempoProgramError::InsufficientCollateral.into());
        }
        self.set_balance(self.balance() - amount);
        Ok(())
    }

    /// Lock `amount` of free balance as margin.
    #[inline(always)]
    pub fn lock(&mut self, amount: u64) -> Result<(), ProgramError> {
        if amount > self.free() {
            return Err(TempoProgramError::InsufficientCollateral.into());
        }
        self.set_locked(
            self.locked()
                .checked_add(amount)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
        Ok(())
    }

    /// Release `amount` of locked margin back to free balance (saturating).
    #[inline(always)]
    pub fn release(&mut self, amount: u64) {
        self.set_locked(self.locked().saturating_sub(amount));
    }

    /// Apply realized PnL (funding + closed-position cash) to the ledger balance.
    /// Positive credits; negative debits (saturating at zero). Returns the
    /// uncovered loss (bad debt) when a loss exceeds the balance.
    #[inline(always)]
    pub fn apply_pnl(&mut self, pnl: i128) -> Result<u64, ProgramError> {
        if pnl >= 0 {
            self.credit(u64::try_from(pnl).map_err(|_| TempoProgramError::MathOverflow)?)?;
            Ok(0)
        } else {
            let loss = u64::try_from(-pnl).map_err(|_| TempoProgramError::MathOverflow)?;
            let bal = self.balance();
            if loss <= bal {
                self.set_balance(bal - loss);
                Ok(0)
            } else {
                self.set_balance(0);
                Ok(loss - bal)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Discriminator;

    #[test]
    fn test_roundtrip_and_accounting() {
        let mut c = UserCollateral::new(255, Address::new_from_array([3u8; 32]));
        c.credit(1_000).unwrap();
        c.lock(300).unwrap();
        assert_eq!(c.balance(), 1_000);
        assert_eq!(c.locked(), 300);
        assert_eq!(c.free(), 700);

        // cannot withdraw locked margin
        assert_eq!(
            c.debit(800),
            Err(TempoProgramError::InsufficientCollateral.into())
        );
        c.debit(700).unwrap();
        assert_eq!(c.balance(), 300);
        assert_eq!(c.free(), 0);

        c.release(300);
        assert_eq!(c.locked(), 0);

        let bytes = c.to_bytes();
        assert_eq!(bytes[0], UserCollateral::DISCRIMINATOR);
        let de = UserCollateral::from_bytes(&bytes).unwrap();
        assert_eq!(de.owner, c.owner);
        assert_eq!(de.balance(), 300);
    }

    #[test]
    fn test_apply_pnl() {
        let mut c = UserCollateral::new(1, Address::new_from_array([0u8; 32]));
        c.credit(1_000).unwrap();
        assert_eq!(c.apply_pnl(250).unwrap(), 0);
        assert_eq!(c.balance(), 1_250);
        assert_eq!(c.apply_pnl(-200).unwrap(), 0);
        assert_eq!(c.balance(), 1_050);
        // Loss beyond balance saturates to 0 and reports the shortfall as bad debt.
        assert_eq!(c.apply_pnl(-1_100).unwrap(), 50);
        assert_eq!(c.balance(), 0);
    }
}
