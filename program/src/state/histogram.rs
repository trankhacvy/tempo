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

/// `AuctionHistogram` — "the mailboxes" (clearing-protocol §2, system-design §6.2).
///
/// The book for the round being cleared is represented as a fixed-size
/// histogram over price ticks. **The size depends only on the tick count,
/// never on the number of orders.** That decoupling is what makes
/// multi-transaction clearing possible.
///
/// # Dual auction (system-design §1)
/// Each round runs two independent uniform-price crosses: a **bid auction**
/// (maker-buys vs taker-sells) and an **ask auction** (taker-buys vs
/// maker-sells). The histogram holds four bucket arrays, one per [`Region`],
/// each of length `T`.
///
/// # On-disk layout
/// ```text
/// [ disc | version | header | bid_demand | bid_supply | ask_demand | ask_supply ]
///   1      1         53       T*8          T*8          T*8          T*8
/// ```
/// Total account size = 2 + HEADER_LEN + 4*T*8 bytes. The buckets live *after*
/// the header struct and are accessed via the helper methods below on the raw
/// account data slice — they are deliberately NOT fields of the `#[repr(C)]`
/// header (T is dynamic per market).
///
/// # Header layout (`#[repr(C)]`, **alignment 1** — see `le_field!`)
/// 2 × [u8;8] (16) + [u8;4] (4) + Address (32) + u8 (1) = 53.
#[derive(Clone, Debug, PartialEq, CodamaAccount)]
#[codama(field("discriminator", number(u8), default_value = 2))]
#[codama(discriminator(field = "discriminator"))]
#[codama(seed(type = string(utf8), value = "histogram"))]
#[codama(seed(name = "market", type = public_key))]
#[repr(C)]
pub struct AuctionHistogramHeader {
    /// Auction round this histogram is accumulating.
    pub auction_id_le: [u8; 8],
    /// Number of orders folded so far (mirrors `Market.accumulated_order_count`).
    pub accumulated_count_le: [u8; 8],
    /// Number of price ticks (length of each bucket array).
    pub num_ticks_le: [u8; 4],
    /// Market this histogram belongs to.
    pub market: Address,
    /// PDA bump.
    pub bump: u8,
}

assert_no_padding!(AuctionHistogramHeader, 8 * 2 + 4 + 32 + 1);

/// Byte length of one histogram bucket value.
pub const BUCKET_LEN: usize = 8;

/// The four bucket arrays of the dual auction (system-design §1).
///
/// Orders are routed by `(side, is_maker)`:
/// - maker buy  → [`Region::BidDemand`]   - taker sell → [`Region::BidSupply`]
/// - taker buy  → [`Region::AskDemand`]   - maker sell → [`Region::AskSupply`]
///
/// The bid auction crosses `BidDemand` against `BidSupply`; the ask auction
/// crosses `AskDemand` against `AskSupply`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    BidDemand = 0,
    BidSupply = 1,
    AskDemand = 2,
    AskSupply = 3,
}

/// Number of bucket arrays in the histogram.
pub const NUM_REGIONS: usize = 4;

impl Discriminator for AuctionHistogramHeader {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::AuctionHistogramDiscriminator as u8;
}

impl Versioned for AuctionHistogramHeader {
    const VERSION: u8 = 1;
}

impl AccountSize for AuctionHistogramHeader {
    const DATA_LEN: usize = 8 * 2 + 4 + 32 + 1;
}

impl AccountDeserialize for AuctionHistogramHeader {}

impl AccountSerialize for AuctionHistogramHeader {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(&self.auction_id_le);
        data.extend_from_slice(&self.accumulated_count_le);
        data.extend_from_slice(&self.num_ticks_le);
        data.extend_from_slice(self.market.as_ref());
        data.push(self.bump);
        data
    }
}

impl PdaSeeds for AuctionHistogramHeader {
    const PREFIX: &'static [u8] = b"histogram";

    #[inline(always)]
    fn seeds(&self) -> Vec<&[u8]> {
        // NOTE: auction_id is part of the canonical seeds but is a u64; callers
        // that need to derive with the id should build the byte seed locally.
        vec![Self::PREFIX, self.market.as_ref()]
    }

    #[inline(always)]
    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.market.as_ref()),
            Seed::from(bump.as_slice()),
        ]
    }
}

impl PdaAccount for AuctionHistogramHeader {
    #[inline(always)]
    fn bump(&self) -> u8 {
        self.bump
    }
}

impl AuctionHistogramHeader {
    le_field!(auction_id, set_auction_id, auction_id_le, u64);
    le_field!(
        accumulated_count,
        set_accumulated_count,
        accumulated_count_le,
        u64
    );
    le_field!(num_ticks, set_num_ticks, num_ticks_le, u32);

    #[inline(always)]
    pub fn new(bump: u8, market: Address, auction_id: u64, num_ticks: u32) -> Self {
        Self {
            auction_id_le: auction_id.to_le_bytes(),
            accumulated_count_le: 0u64.to_le_bytes(),
            num_ticks_le: num_ticks.to_le_bytes(),
            market,
            bump,
        }
    }

    /// Total account size (incl. disc+version prefix) for `num_ticks` ticks.
    #[inline(always)]
    pub fn account_size(num_ticks: u32) -> usize {
        Self::LEN + Self::buckets_len(num_ticks)
    }

    /// Byte length of the bucket region: `NUM_REGIONS` arrays of `num_ticks` u64.
    #[inline(always)]
    pub fn buckets_len(num_ticks: u32) -> usize {
        NUM_REGIONS * num_ticks as usize * BUCKET_LEN
    }

    /// Byte offset (within the full account data, after disc+version) where the
    /// first bucket array begins.
    #[inline(always)]
    pub const fn buckets_offset() -> usize {
        // disc(1) + version(1) + header data
        Self::LEN
    }
}

// ---------------------------------------------------------------------------
// Bucket access helpers.
//
// These operate on the *full* account data slice (including the 2-byte
// disc+version prefix). They are commutative integer adds into a single
// bucket — folding order is irrelevant to the final histogram
// (clearing-protocol §4.1), which is the core hostile-cranker resistance.
// ---------------------------------------------------------------------------

#[inline(always)]
fn bucket_range(num_ticks: u32, region: Region, tick: u32) -> Option<core::ops::Range<usize>> {
    if tick >= num_ticks {
        return None;
    }
    let idx = region as usize * num_ticks as usize + tick as usize;
    let start = AuctionHistogramHeader::buckets_offset() + idx * BUCKET_LEN;
    Some(start..start + BUCKET_LEN)
}

#[inline(always)]
fn read_bucket(data: &[u8], range: core::ops::Range<usize>) -> Result<u64, ProgramError> {
    let bytes: [u8; 8] = data
        .get(range)
        .ok_or(ProgramError::AccountDataTooSmall)?
        .try_into()
        .map_err(|_| ProgramError::AccountDataTooSmall)?;
    Ok(u64::from_le_bytes(bytes))
}

#[inline(always)]
fn write_bucket(
    data: &mut [u8],
    range: core::ops::Range<usize>,
    value: u64,
) -> Result<(), ProgramError> {
    let dst = data
        .get_mut(range)
        .ok_or(ProgramError::AccountDataTooSmall)?;
    dst.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

/// Read the bucket for `region` at `tick` from the full account data slice.
#[inline(always)]
pub fn read_region(
    data: &[u8],
    num_ticks: u32,
    region: Region,
    tick: u32,
) -> Result<u64, ProgramError> {
    let range = bucket_range(num_ticks, region, tick)
        .ok_or::<ProgramError>(TempoProgramError::InvalidTick.into())?;
    read_bucket(data, range)
}

/// Read an entire region's `num_ticks` buckets in one pass, directly from the
/// contiguous account bytes. Avoids the per-tick `bucket_range` bounds-check +
/// slice-get overhead of calling [`read_region`] in a loop — a meaningful CU
/// saving in `finalize_clear`, which reads all four regions over every tick
/// (cu_optimizations §3: interact with the memory directly).
#[inline]
pub fn read_region_values(
    data: &[u8],
    num_ticks: u32,
    region: Region,
) -> Result<Vec<u64>, ProgramError> {
    let n = num_ticks as usize;
    let start = AuctionHistogramHeader::buckets_offset() + region as usize * n * BUCKET_LEN;
    let end = start + n * BUCKET_LEN;
    let block = data
        .get(start..end)
        .ok_or(ProgramError::AccountDataTooSmall)?;
    let mut out = Vec::with_capacity(n);
    for chunk in block.chunks_exact(BUCKET_LEN) {
        out.push(u64::from_le_bytes(chunk.try_into().unwrap_or([0u8; 8])));
    }
    Ok(out)
}

/// Fold an order quantity into the `region` bucket at `tick` (commutative add).
///
/// Commutativity (clearing-protocol §4.1): this is plain checked integer
/// addition into one bucket, so the final histogram is identical regardless of
/// which crank folds which order in which order.
#[inline(always)]
pub fn fold(
    data: &mut [u8],
    num_ticks: u32,
    region: Region,
    tick: u32,
    qty: u64,
) -> Result<(), ProgramError> {
    let range = bucket_range(num_ticks, region, tick)
        .ok_or::<ProgramError>(TempoProgramError::InvalidTick.into())?;
    let current = read_bucket(data, range.clone())?;
    let updated = current
        .checked_add(qty)
        .ok_or(TempoProgramError::MathOverflow)?;
    write_bucket(data, range, updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Discriminator;

    fn header() -> AuctionHistogramHeader {
        AuctionHistogramHeader::new(254, Address::new_from_array([7u8; 32]), 1, 4)
    }

    #[test]
    fn test_header_roundtrip() {
        let h = header();
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), AuctionHistogramHeader::LEN);
        assert_eq!(bytes[0], AuctionHistogramHeader::DISCRIMINATOR);
        assert_eq!(bytes[1], AuctionHistogramHeader::VERSION);

        let de = AuctionHistogramHeader::from_bytes(&bytes).unwrap();
        assert_eq!(de.auction_id(), 1);
        assert_eq!(de.num_ticks(), 4);
        assert_eq!(de.market, h.market);
        assert_eq!(de.bump, 254);
    }

    #[test]
    fn test_account_size() {
        // disc+version(2) + header data(53) + 4*4*8 buckets(128) = 183
        assert_eq!(AuctionHistogramHeader::account_size(4), 2 + 53 + 128);
        assert_eq!(AuctionHistogramHeader::buckets_len(4), 128);
    }

    fn fresh_account(num_ticks: u32) -> Vec<u8> {
        let h = AuctionHistogramHeader::new(255, Address::new_from_array([1u8; 32]), 0, num_ticks);
        let mut data = vec![0u8; AuctionHistogramHeader::account_size(num_ticks)];
        h.write_to_slice(&mut data).unwrap();
        data
    }

    #[test]
    fn test_fold_and_read() {
        let n = 4;
        let mut data = fresh_account(n);
        fold(&mut data, n, Region::BidDemand, 0, 10).unwrap();
        fold(&mut data, n, Region::BidDemand, 0, 5).unwrap();
        fold(&mut data, n, Region::BidSupply, 3, 7).unwrap();
        fold(&mut data, n, Region::AskDemand, 1, 4).unwrap();
        fold(&mut data, n, Region::AskSupply, 2, 9).unwrap();

        assert_eq!(read_region(&data, n, Region::BidDemand, 0).unwrap(), 15);
        assert_eq!(read_region(&data, n, Region::BidDemand, 1).unwrap(), 0);
        assert_eq!(read_region(&data, n, Region::BidSupply, 3).unwrap(), 7);
        assert_eq!(read_region(&data, n, Region::BidSupply, 0).unwrap(), 0);
        assert_eq!(read_region(&data, n, Region::AskDemand, 1).unwrap(), 4);
        assert_eq!(read_region(&data, n, Region::AskSupply, 2).unwrap(), 9);
        // regions are independent
        assert_eq!(read_region(&data, n, Region::AskDemand, 0).unwrap(), 0);
    }

    #[test]
    fn test_fold_out_of_range() {
        let n = 4;
        let mut data = fresh_account(n);
        assert_eq!(
            fold(&mut data, n, Region::BidDemand, 4, 1),
            Err(TempoProgramError::InvalidTick.into())
        );
        assert_eq!(
            fold(&mut data, n, Region::AskSupply, 99, 1),
            Err(TempoProgramError::InvalidTick.into())
        );
    }

    /// Commutativity: folding the same multiset of (region, tick, qty) entries
    /// in two different orders yields byte-identical bucket regions.
    /// This is the key security property (clearing-protocol §4.1).
    #[test]
    fn test_fold_commutativity() {
        let n = 8;
        let entries: &[(Region, u32, u64)] = &[
            (Region::BidDemand, 1, 5),
            (Region::AskSupply, 6, 3),
            (Region::BidDemand, 1, 2),
            (Region::BidSupply, 4, 9),
            (Region::AskDemand, 6, 1),
            (Region::AskSupply, 2, 8),
            (Region::BidSupply, 7, 4),
        ];

        let mut a = fresh_account(n);
        for &(r, t, q) in entries {
            fold(&mut a, n, r, t, q).unwrap();
        }

        let mut b = fresh_account(n);
        for &(r, t, q) in entries.iter().rev() {
            fold(&mut b, n, r, t, q).unwrap();
        }

        let off = AuctionHistogramHeader::buckets_offset();
        assert_eq!(
            &a[off..],
            &b[off..],
            "histogram must be identical regardless of fold order"
        );
    }
}
