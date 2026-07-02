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
/// The ledger is **mint-scoped** (CR-3): each `(owner, collateral_mint)` pair has
/// its own ledger so a balance deposited under one mint can never be withdrawn
/// against another mint's per-mint vault. `collateral_mint` is both a stored field
/// and a PDA seed; `deposit`/`withdraw`/`withdraw_cross` assert it matches the
/// vault's `collateral_mint`.
///
/// # PDA Seeds
/// `[b"collateral", owner.as_ref(), collateral_mint.as_ref()]`
///
/// # Zero-copy layout (`#[repr(C)]`, **alignment 1**)
/// 2 × Address (64) + 2 × [u8;8] (16) + u8 (1) = 81.
///
/// # Migration
/// Adding `collateral_mint` to the seeds changes the PDA **address**, so an
/// in-place realloc migration (as `migrate_position` does) is impossible — a v1
/// ledger lives at a different address than its v2 counterpart. The `VERSION` bump
/// makes any stale-layout account fail the version check loudly; existing v1
/// `[b"collateral", owner]` ledgers must be **re-provisioned** (withdraw, then
/// `init_collateral` at the new mint-scoped address). This is a latent fix (only
/// one collateral mint exists today), so re-provisioning is acceptable.
#[derive(Clone, Debug, PartialEq, CodamaAccount)]
#[codama(field("discriminator", number(u8), default_value = 7))]
#[codama(discriminator(field = "discriminator"))]
#[codama(seed(type = string(utf8), value = "collateral"))]
#[codama(seed(name = "owner", type = public_key))]
#[codama(seed(name = "mint", type = public_key))]
#[repr(C)]
pub struct UserCollateral {
    pub owner: Address,
    pub collateral_mint: Address,
    pub balance_le: [u8; 8],
    pub locked_le: [u8; 8],
    pub bump: u8,
}

assert_no_padding!(UserCollateral, 32 + 32 + 8 + 8 + 1);

impl Discriminator for UserCollateral {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::UserCollateralDiscriminator as u8;
}

impl Versioned for UserCollateral {
    // v2: `collateral_mint` added + folded into the PDA seeds (CR-3). The bump makes
    // a pre-v2 (mint-less) ledger fail the version check rather than be mis-read.
    const VERSION: u8 = 2;
}

impl AccountSize for UserCollateral {
    const DATA_LEN: usize = 32 + 32 + 8 + 8 + 1;
}

impl AccountDeserialize for UserCollateral {}

impl AccountSerialize for UserCollateral {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.owner.as_ref());
        data.extend_from_slice(self.collateral_mint.as_ref());
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
        vec![
            Self::PREFIX,
            self.owner.as_ref(),
            self.collateral_mint.as_ref(),
        ]
    }

    #[inline(always)]
    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.owner.as_ref()),
            Seed::from(self.collateral_mint.as_ref()),
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
    pub fn new(bump: u8, owner: Address, collateral_mint: Address) -> Self {
        Self {
            owner,
            collateral_mint,
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

    /// Lock up to `amount` of free balance, returning how much was actually locked
    /// (`== amount` unless free balance is short). Never fails — used on the
    /// `settle_fill` re-lock path (DDR-3): a resting order the recentered tick
    /// window gapped through can need more margin than its reservation released, and
    /// a matched fill can't be un-filled (conservation), so the settle must not
    /// revert. Any uncovered remainder leaves the position below initial margin for
    /// the liquidation backstop rather than wedging the round. The caller sets the
    /// position's collateral to what was actually locked so it never over-reports.
    #[inline(always)]
    pub fn lock_up_to(&mut self, amount: u64) -> u64 {
        let lockable = amount.min(self.free());
        self.set_locked(self.locked().saturating_add(lockable));
        lockable
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
        let mut c = UserCollateral::new(
            255,
            Address::new_from_array([3u8; 32]),
            Address::new_from_array([7u8; 32]),
        );
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
        assert_eq!(bytes.len(), UserCollateral::LEN);
        assert_eq!(bytes[0], UserCollateral::DISCRIMINATOR);
        assert_eq!(bytes[1], UserCollateral::VERSION);
        let de = UserCollateral::from_bytes(&bytes).unwrap();
        assert_eq!(de.owner, c.owner);
        assert_eq!(de.collateral_mint, c.collateral_mint);
        assert_eq!(de.balance(), 300);
    }

    #[test]
    fn test_lock_up_to_caps_and_never_fails() {
        // DDR-3: the settle re-lock path uses `lock_up_to`, which locks what's
        // available and never errors (so a resting order the window gapped through
        // can always settle instead of wedging the round).
        let mut c = UserCollateral::new(
            255,
            Address::new_from_array([3u8; 32]),
            Address::new_from_array([7u8; 32]),
        );
        c.credit(1_000).unwrap();
        // Enough free → locks exactly the request.
        assert_eq!(c.lock_up_to(300), 300);
        assert_eq!(c.locked(), 300);
        assert_eq!(c.free(), 700);
        // More than free → caps at free (700), leaving the remainder uncovered.
        assert_eq!(c.lock_up_to(1_000), 700);
        assert_eq!(c.locked(), 1_000);
        assert_eq!(c.free(), 0);
        // Nothing free → locks 0, still no panic/error.
        assert_eq!(c.lock_up_to(500), 0);
        assert_eq!(c.locked(), 1_000);
    }

    #[test]
    fn test_mint_in_seeds() {
        // The collateral_mint is folded into the PDA seeds (CR-3): two ledgers for
        // the same owner under different mints derive different addresses.
        let owner = Address::new_from_array([3u8; 32]);
        let a = UserCollateral::new(1, owner, Address::new_from_array([1u8; 32]));
        let b = UserCollateral::new(1, owner, Address::new_from_array([2u8; 32]));
        assert_eq!(a.seeds().len(), 3);
        assert_eq!(a.seeds()[0], UserCollateral::PREFIX);
        assert_eq!(a.seeds()[1], owner.as_ref());
        assert_ne!(a.seeds()[2], b.seeds()[2]);
    }

    #[test]
    fn test_apply_pnl() {
        let mut c = UserCollateral::new(
            1,
            Address::new_from_array([0u8; 32]),
            Address::new_from_array([9u8; 32]),
        );
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
