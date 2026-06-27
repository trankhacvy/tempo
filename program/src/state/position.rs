use alloc::vec;
use alloc::vec::Vec;
use codama::CodamaAccount;
use pinocchio::{cpi::Seed, error::ProgramError, Address};

use crate::errors::TempoProgramError;
use crate::funding::funding_payment;
use crate::traits::{
    AccountDeserialize, AccountSerialize, AccountSize, Discriminator, PdaAccount, PdaSeeds,
    TempoAccountDiscriminators, Versioned,
};
use crate::{assert_no_padding, le_field};

/// A trader's perp position in one market (system-design §6.4).
///
/// `size` is signed base units (+long / −short). `entry_price` is the volume-
/// weighted average entry of the open exposure. `realized_pnl` accumulates PnL
/// closed by opposing fills. `last_funding_index` is the funding index the
/// position last settled against (see `funding.rs`). `collateral` is the margin
/// allocated to this position.
///
/// # PDA Seeds
/// `[b"position", market.as_ref(), owner.as_ref()]`
///
/// # Zero-copy layout (`#[repr(C)]`, **alignment 1** — see `le_field!`)
/// 2 × Address (64) + 3 × [u8;8] (24) + 2 × [u8;16] (32) + u8 (1) + [u8;16] (16) + u8 (1) = 138.
#[derive(Clone, Debug, PartialEq, CodamaAccount)]
#[codama(field("discriminator", number(u8), default_value = 5))]
#[codama(discriminator(field = "discriminator"))]
#[codama(seed(type = string(utf8), value = "position"))]
#[codama(seed(name = "market", type = public_key))]
#[codama(seed(name = "owner", type = public_key))]
#[repr(C)]
pub struct Position {
    pub owner: Address,
    pub market: Address,
    pub size_le: [u8; 8],
    pub entry_price_le: [u8; 8],
    pub collateral_le: [u8; 8],
    pub realized_pnl_le: [u8; 16],
    pub last_funding_index_le: [u8; 16],
    pub bump: u8,
    /// Socialized-loss index (for the position's current side) last settled
    /// against (ADL). Appended in VERSION 2; keeps prior offsets stable.
    pub last_social_index_le: [u8; 16],
    /// Margin mode: 0 = isolated (uses `UserCollateral.locked`), 1 = cross
    /// (backed by combined equity). Appended in VERSION 3; keeps prior offsets stable.
    pub margin_mode: u8,
}

assert_no_padding!(Position, 32 * 2 + 8 * 3 + 16 * 2 + 1 + 16 + 1);

impl Discriminator for Position {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::PositionDiscriminator as u8;
}

impl Versioned for Position {
    const VERSION: u8 = 3;
}

impl AccountSize for Position {
    const DATA_LEN: usize = 32 * 2 + 8 * 3 + 16 * 2 + 1 + 16 + 1;
}

impl AccountDeserialize for Position {}

impl AccountSerialize for Position {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.owner.as_ref());
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(&self.size_le);
        data.extend_from_slice(&self.entry_price_le);
        data.extend_from_slice(&self.collateral_le);
        data.extend_from_slice(&self.realized_pnl_le);
        data.extend_from_slice(&self.last_funding_index_le);
        data.push(self.bump);
        data.extend_from_slice(&self.last_social_index_le);
        data.push(self.margin_mode);
        data
    }
}

impl PdaSeeds for Position {
    const PREFIX: &'static [u8] = b"position";

    #[inline(always)]
    fn seeds(&self) -> Vec<&[u8]> {
        vec![Self::PREFIX, self.market.as_ref(), self.owner.as_ref()]
    }

    #[inline(always)]
    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.market.as_ref()),
            Seed::from(self.owner.as_ref()),
            Seed::from(bump.as_slice()),
        ]
    }
}

impl PdaAccount for Position {
    #[inline(always)]
    fn bump(&self) -> u8 {
        self.bump
    }
}

impl Position {
    le_field!(size, set_size, size_le, i64);
    le_field!(entry_price, set_entry_price, entry_price_le, u64);
    le_field!(collateral, set_collateral, collateral_le, u64);
    le_field!(realized_pnl, set_realized_pnl, realized_pnl_le, i128);
    le_field!(
        last_funding_index,
        set_last_funding_index,
        last_funding_index_le,
        i128
    );
    le_field!(
        last_social_index,
        set_last_social_index,
        last_social_index_le,
        i128
    );

    #[inline(always)]
    pub fn new(bump: u8, owner: Address, market: Address, funding_index: i128) -> Self {
        Self {
            owner,
            market,
            size_le: 0i64.to_le_bytes(),
            entry_price_le: 0u64.to_le_bytes(),
            collateral_le: 0u64.to_le_bytes(),
            realized_pnl_le: 0i128.to_le_bytes(),
            last_funding_index_le: funding_index.to_le_bytes(),
            bump,
            last_social_index_le: 0i128.to_le_bytes(),
            margin_mode: 0,
        }
    }

    /// Apply a fill to the position: VWAP-average the entry when increasing the
    /// same direction, realize PnL when an opposing fill reduces or flips it
    /// (system-design §3.2). `is_buy` is the fill side, `qty` the base amount,
    /// `price` the clearing price. Rounds realized PnL toward zero.
    pub fn apply_fill(
        &mut self,
        is_buy: bool,
        qty: u64,
        price: u64,
        social_long: i128,
        social_short: i128,
    ) -> Result<(), ProgramError> {
        if qty == 0 {
            return Ok(());
        }
        let old_size = self.size() as i128;
        let signed = if is_buy { qty as i128 } else { -(qty as i128) };
        let new_size = old_size
            .checked_add(signed)
            .ok_or(TempoProgramError::MathOverflow)?;
        let entry = self.entry_price() as i128;
        let p = price as i128;

        let opening_or_increasing = old_size == 0 || (old_size > 0) == is_buy;
        if opening_or_increasing {
            // VWAP the entry over the combined exposure.
            let old_abs = old_size.unsigned_abs() as i128;
            let add_abs = qty as i128;
            let new_abs = old_abs + add_abs;
            let new_entry = (old_abs * entry + add_abs * p) / new_abs;
            self.set_entry_price(
                u64::try_from(new_entry).map_err(|_| TempoProgramError::MathOverflow)?,
            );
        } else {
            // Opposing fill: realize PnL on the closed portion.
            let closed = core::cmp::min(old_size.unsigned_abs() as i128, qty as i128);
            let pnl = if old_size > 0 {
                (p - entry) * closed
            } else {
                (entry - p) * closed
            };
            self.set_realized_pnl(
                self.realized_pnl()
                    .checked_add(pnl)
                    .ok_or(TempoProgramError::MathOverflow)?,
            );
            // If the fill flips the position, the remainder opens at `price`.
            if (qty as i128) > old_size.unsigned_abs() as i128 {
                self.set_entry_price(price);
            }
        }

        self.set_size(i64::try_from(new_size).map_err(|_| TempoProgramError::MathOverflow)?);

        // Re-snapshot the social-loss checkpoint whenever the position opens from
        // flat OR flips sign, so settle_social_loss always compares against the
        // CURRENT side's index and a flip can never under-charge (known-issues §1.5).
        let opened = old_size == 0 && new_size != 0;
        let flipped = old_size != 0 && new_size != 0 && (old_size > 0) != (new_size > 0);
        if opened || flipped {
            self.snapshot_social_index(social_long, social_short);
        }
        Ok(())
    }

    /// Settle funding up to `index_now`, moving the owed amount into realized
    /// PnL and advancing the position's funding checkpoint.
    pub fn settle_funding(&mut self, index_now: i128) -> Result<(), ProgramError> {
        let pay = funding_payment(self.size() as i128, index_now, self.last_funding_index())?;
        // The position *pays* `pay` (positive) → reduces realized PnL.
        self.set_realized_pnl(
            self.realized_pnl()
                .checked_sub(pay)
                .ok_or(TempoProgramError::MathOverflow)?,
        );
        self.set_last_funding_index(index_now);
        Ok(())
    }

    /// Settle socialized loss (ADL) for the position's current side against
    /// the market's per-side index. Only ever charges (a position never receives
    /// free value here): the owed amount is `|size| · max(0, index_now − last) /
    /// FUNDING_SCALE`, debited from realized PnL. `index_long`/`index_short` are
    /// the market's current per-side indices. A flat position just snapshots.
    ///
    /// Note: a single checkpoint tracks the position's *current* side, so a
    /// flip can under-charge the new side until its next snapshot; it never
    /// over-charges. Fresh opens are re-snapshotted by the caller after the fill.
    pub fn settle_social_loss(
        &mut self,
        index_long: i128,
        index_short: i128,
    ) -> Result<(), ProgramError> {
        let size = self.size() as i128;
        if size == 0 {
            self.set_last_social_index(0);
            return Ok(());
        }
        let index_now = if size > 0 { index_long } else { index_short };
        let delta = index_now.saturating_sub(self.last_social_index());
        if delta > 0 {
            let pay = size
                .unsigned_abs()
                .checked_mul(delta as u128)
                .ok_or(TempoProgramError::MathOverflow)?
                / (crate::funding::FUNDING_SCALE as u128);
            let pay = i128::try_from(pay).map_err(|_| TempoProgramError::MathOverflow)?;
            self.set_realized_pnl(
                self.realized_pnl()
                    .checked_sub(pay)
                    .ok_or(TempoProgramError::MathOverflow)?,
            );
        }
        self.set_last_social_index(index_now);
        Ok(())
    }

    /// Snapshot the social-loss checkpoint to the index of the position's current
    /// side, so a freshly opened position does not pay loss socialized before it
    /// existed. Called by settle processors right after a flat→open fill.
    pub fn snapshot_social_index(&mut self, index_long: i128, index_short: i128) {
        let size = self.size() as i128;
        let index_now = if size >= 0 { index_long } else { index_short };
        self.set_last_social_index(index_now);
    }
}

/// Pure mirror of `Position::settle_social_loss` that returns the owed socialized
/// loss without mutating. Used by cross-margin health checks to dock unsettled
/// social loss on read-only legs (known-issues §1.4). Positive = the position owes.
#[inline(always)]
pub fn pending_social_loss(
    size_signed: i128,
    index_long: i128,
    index_short: i128,
    last_social: i128,
) -> i128 {
    if size_signed == 0 {
        return 0;
    }
    let index_now = if size_signed > 0 {
        index_long
    } else {
        index_short
    };
    let delta = index_now.saturating_sub(last_social);
    if delta <= 0 {
        return 0;
    }
    let pay = size_signed.unsigned_abs().saturating_mul(delta as u128)
        / (crate::funding::FUNDING_SCALE as u128);
    i128::try_from(pay).unwrap_or(i128::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Discriminator;

    fn pos() -> Position {
        Position::new(
            255,
            Address::new_from_array([1u8; 32]),
            Address::new_from_array([2u8; 32]),
            0,
        )
    }

    #[test]
    fn test_roundtrip() {
        let mut p = pos();
        p.set_size(-42);
        p.set_entry_price(100);
        p.set_realized_pnl(-7);
        p.set_last_funding_index(123);
        let bytes = p.to_bytes();
        assert_eq!(bytes.len(), Position::LEN);
        assert_eq!(bytes[0], Position::DISCRIMINATOR);
        let de = Position::from_bytes(&bytes).unwrap();
        assert_eq!(de.size(), -42);
        assert_eq!(de.entry_price(), 100);
        assert_eq!(de.realized_pnl(), -7);
        assert_eq!(de.last_funding_index(), 123);
        assert_eq!(de.owner, p.owner);
    }

    #[test]
    fn test_open_and_increase_vwap() {
        let mut p = pos();
        p.apply_fill(true, 100, 10, 0, 0).unwrap(); // long 100 @ 10
        assert_eq!(p.size(), 100);
        assert_eq!(p.entry_price(), 10);
        p.apply_fill(true, 100, 20, 0, 0).unwrap(); // +100 @ 20 -> avg 15
        assert_eq!(p.size(), 200);
        assert_eq!(p.entry_price(), 15);
        assert_eq!(p.realized_pnl(), 0);
    }

    #[test]
    fn test_reduce_realizes_pnl() {
        let mut p = pos();
        p.apply_fill(true, 100, 10, 0, 0).unwrap(); // long 100 @ 10
        p.apply_fill(false, 40, 15, 0, 0).unwrap(); // sell 40 @ 15 -> realize (15-10)*40=200
        assert_eq!(p.size(), 60);
        assert_eq!(p.entry_price(), 10); // entry unchanged on reduce
        assert_eq!(p.realized_pnl(), 200);
    }

    #[test]
    fn test_flip_realizes_and_reopens() {
        let mut p = pos();
        p.apply_fill(true, 100, 10, 0, 0).unwrap(); // long 100 @ 10
        p.apply_fill(false, 150, 12, 0, 0).unwrap(); // sell 150 @ 12: close 100 (+200), flip to short 50 @ 12
        assert_eq!(p.size(), -50);
        assert_eq!(p.entry_price(), 12);
        assert_eq!(p.realized_pnl(), (12 - 10) * 100);
    }

    #[test]
    fn test_settle_social_loss_charges_current_side_only() {
        let scale = crate::funding::FUNDING_SCALE;
        // long 1000: pays the long-side index (1% → 10), short index ignored.
        let mut p = pos();
        p.apply_fill(true, 1000, 100, 0, 0).unwrap();
        p.settle_social_loss(scale / 100, scale / 50).unwrap();
        assert_eq!(p.realized_pnl(), -10);
        assert_eq!(p.last_social_index(), scale / 100);
        // settling again at the same index charges nothing more.
        p.settle_social_loss(scale / 100, scale / 50).unwrap();
        assert_eq!(p.realized_pnl(), -10);

        // a short position is charged by the short index only.
        let mut s = pos();
        s.apply_fill(false, 1000, 100, 0, 0).unwrap();
        s.settle_social_loss(scale / 100, 0).unwrap();
        assert_eq!(s.realized_pnl(), 0, "short index flat → no charge");
    }

    #[test]
    fn test_settle_social_loss_never_credits() {
        let scale = crate::funding::FUNDING_SCALE;
        let mut p = pos();
        p.apply_fill(true, 1000, 100, 0, 0).unwrap();
        p.set_last_social_index(scale / 50); // checkpoint above current index
        p.settle_social_loss(scale / 100, 0).unwrap(); // delta negative
        assert_eq!(p.realized_pnl(), 0, "never receives free value");
    }

    #[test]
    fn test_apply_fill_resnapshots_social_on_flip() {
        let scale = crate::funding::FUNDING_SCALE;
        let mut p = pos();
        // Open long while long-index = 1%, short-index = 5%.
        p.apply_fill(true, 100, 10, scale / 100, scale / 20)
            .unwrap();
        assert_eq!(
            p.last_social_index(),
            scale / 100,
            "opened long → long index"
        );
        // Flip to short in one fill; the checkpoint must move to the short index
        // so the next settle charges against short (no under-charge, §1.5).
        p.apply_fill(false, 150, 12, scale / 100, scale / 20)
            .unwrap();
        assert!(p.size() < 0);
        assert_eq!(p.last_social_index(), scale / 20, "flip → short index");
    }

    #[test]
    fn test_pending_social_loss_matches_settle() {
        let scale = crate::funding::FUNDING_SCALE;
        // long 1000, long-index 1%, checkpoint 0 → owes 10; the mutating settle
        // moves exactly that into realized PnL.
        let pending = pending_social_loss(1000, scale / 100, scale / 50, 0);
        let mut p = pos();
        p.apply_fill(true, 1000, 100, 0, 0).unwrap();
        let before = p.realized_pnl();
        p.settle_social_loss(scale / 100, scale / 50).unwrap();
        assert_eq!(pending, before - p.realized_pnl());
        // a checkpoint at/above the current index owes nothing.
        assert_eq!(pending_social_loss(1000, scale / 100, 0, scale / 100), 0);
        // flat owes nothing.
        assert_eq!(pending_social_loss(0, scale, scale, 0), 0);
    }

    #[test]
    fn test_settle_funding_pays_from_pnl() {
        let mut p = pos();
        p.apply_fill(true, 1000, 100, 0, 0).unwrap(); // long 1000
                                                      // index advances by 0.01 * FUNDING_SCALE → long pays 10.
        p.settle_funding(crate::funding::FUNDING_SCALE / 100)
            .unwrap();
        assert_eq!(p.realized_pnl(), -10);
        assert_eq!(p.last_funding_index(), crate::funding::FUNDING_SCALE / 100);
    }
}
