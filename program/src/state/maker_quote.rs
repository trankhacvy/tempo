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

/// Max ladder levels per side (bid/ask). Tunable; 8 keeps the account ~316 bytes.
pub const MAX_LEVELS: usize = 8;
/// Bytes per level: a `u16` tick offset + a `u64` base-lot size.
pub const LEVEL_BYTES: usize = 2 + 8;
/// Byte length of one side's flat level region.
pub const LEVELS_LEN: usize = MAX_LEVELS * LEVEL_BYTES;
/// Bytes per fold-snapshot entry (one `u64` `cum_before` per ladder level).
pub const SNAPSHOT_BYTES: usize = 8;
/// Byte length of one side's flat fold-snapshot region.
pub const SNAPSHOTS_LEN: usize = MAX_LEVELS * SNAPSHOT_BYTES;
/// Sentinel snapshot meaning "this level was NOT folded this round" (off-grid or
/// the whole quote expired). A level carrying it fills zero in settlement, so a
/// never-folded level can never mint position (§1.6). `u64::MAX` is unreachable as
/// a real bucket prefix (it would require ~1.8e19 base lots resting at one tick).
pub const SNAPSHOT_UNFOLDED: u64 = u64::MAX;
/// Max concurrent quotes per (market, maker) — bounds `quote_index`, the 4th PDA
/// seed (known-issues §4.9: the old 3-seed set allowed exactly ONE ladder per
/// maker per market, capping posted depth and blocking re-quotes mid-round).
pub const MAX_QUOTES_PER_MAKER: u16 = 4;

/// A maker's persistent parametric quote for one market (parametric maker book).
///
/// The ladder is anchored to `mid_tick`: bid level `k` rests at `mid_tick -
/// offset_k`, ask level `k` at `mid_tick + offset_k`. Re-quoting is O(1) (move
/// `mid_tick`); the levels themselves rarely change. A crank folds each active
/// quote into the histogram once per round during ACCUMULATE.
///
/// Levels are stored as two flat byte regions (`MAX_LEVELS` × (u16 offset, u64
/// size)) to keep the struct alignment 1 and Codama-friendly.
///
/// # PDA Seeds
/// `[b"maker_quote", market.as_ref(), maker.as_ref(), quote_index_le]`
/// (`quote_index` ∈ `[0, MAX_QUOTES_PER_MAKER)` — a maker may run several
/// concurrent ladders, §4.9.)
#[derive(Clone, Debug, PartialEq, CodamaAccount)]
#[codama(field("discriminator", number(u8), default_value = 8))]
#[codama(discriminator(field = "discriminator"))]
#[codama(seed(type = string(utf8), value = "maker_quote"))]
#[codama(seed(name = "market", type = public_key))]
#[codama(seed(name = "maker", type = public_key))]
#[codama(seed(name = "quoteIndex", type = number(u16)))]
#[repr(C)]
pub struct MakerQuote {
    pub maker: Address,
    pub market: Address,
    /// Optional delegate allowed to write the ladder (never to move funds).
    pub delegate: Address,
    /// Stable id (the marginal-tick tie-break order across makers).
    pub quote_id_le: [u8; 8],
    /// Monotonic per-quote nonce (replay protection).
    pub sequence_le: [u8; 8],
    /// Anchor tick for the ladder.
    pub mid_tick_le: [u8; 4],
    /// Slot of the last ladder write (expiry clock).
    pub last_update_slot_le: [u8; 8],
    /// Skip folding if `slot - last_update_slot > expiry_slots`; 0 = never expire.
    pub expiry_slots_le: [u8; 8],
    /// Auction id this quote was last folded into (fold-once idempotency).
    pub folded_auction_id_le: [u8; 8],
    /// Auction id this quote was last settled for (settle-once idempotency).
    pub settled_auction_id_le: [u8; 8],
    pub num_bids: u8,
    pub num_asks: u8,
    /// 0 = inactive (skipped, not counted), 1 = active.
    pub status: u8,
    pub bump: u8,
    /// Flat bid ladder: `MAX_LEVELS` × (u16 offset, u64 size), little-endian.
    /// (Size is a literal `80 = LEVELS_LEN` so the Codama derive can resolve it.)
    pub bid_levels_le: [u8; 80],
    /// Flat ask ladder: `MAX_LEVELS` × (u16 offset, u64 size), little-endian.
    pub ask_levels_le: [u8; 80],
    /// Per-level marginal-tick `cum_before` captured at fold time: bid level `i`'s
    /// `BidDemand[tick]` value *before* this quote folded into it (§1.6). Because
    /// only maker quotes feed the maker regions (taker orders go to the taker
    /// regions after §1.3), this is exactly the total maker quantity at that tick
    /// from quotes folded earlier — the conserving telescoping prefix for
    /// `compute_marginal_fill`. `SNAPSHOT_UNFOLDED` = not folded this round.
    /// (Size is a literal `64 = SNAPSHOTS_LEN` so the Codama derive can resolve it.)
    pub bid_snapshots_le: [u8; 64],
    /// Per-level marginal-tick `cum_before` for ask levels (`AskSupply[tick]` before
    /// this quote's fold). See `bid_snapshots_le`.
    pub ask_snapshots_le: [u8; 64],
    // --- v4 (plan.md §2.4): multi-quote seeds + quote-time margin ---
    /// Which of the maker's concurrent quotes this is (`[0, MAX_QUOTES_PER_MAKER)`);
    /// the 4th PDA seed (known-issues §4.9).
    pub quote_index_le: [u8; 2],
    /// STANDING worst-case margin locked in the maker's `UserCollateral` for this
    /// ladder (missing-features §7.1). Recomputed (delta-locked) on every levels
    /// write; released in full by `clear_maker_quote`. The ladder is persistent —
    /// it re-folds at full size every round — so the reservation is a standing
    /// lock, not per-round: an unbacked ladder can never fold into the histogram
    /// and steer the clearing price.
    pub reserved_margin_le: [u8; 8],
    /// Window-top price snapshotted when the reservation was last computed —
    /// mirrors `Order.worst_price` (stable across window recenters, DDR-3).
    pub worst_price_le: [u8; 8],
}

assert_no_padding!(
    MakerQuote,
    32 * 3 + 8 * 2 + 4 + 8 * 4 + 4 + LEVELS_LEN * 2 + SNAPSHOTS_LEN * 2 + 2 + 8 + 8
);

impl Discriminator for MakerQuote {
    const DISCRIMINATOR: u8 = TempoAccountDiscriminators::MakerQuoteDiscriminator as u8;
}

impl Versioned for MakerQuote {
    // v2: the dead `sync_spread_ticks` field was removed from the middle of the
    // struct (known-issues §3); bump so a pre-v2 quote fails the version check
    // loudly rather than folding/settling against off-by-two level data.
    // v3: appended the per-level fold-snapshot regions (known-issues §1.6) — a
    // pre-v3 quote lacks them, so the version check forces a re-provision rather
    // than reading snapshots out of the (shorter) old account tail.
    // v4 (plan.md §2.4): appended `quote_index` (now the 4th PDA seed — the
    // ADDRESS of every quote changes, §4.9) + the quote-time margin fields
    // `reserved_margin`/`worst_price` (§7.1). Old v3 quotes are simply orphaned
    // at their old addresses: `clear_maker_quote` + `close_maker_quote` them
    // (rent refunded) and re-init at the new addresses.
    const VERSION: u8 = 4;
}

impl AccountSize for MakerQuote {
    const DATA_LEN: usize =
        32 * 3 + 8 * 2 + 4 + 8 * 4 + 4 + LEVELS_LEN * 2 + SNAPSHOTS_LEN * 2 + 2 + 8 + 8;
}

impl AccountDeserialize for MakerQuote {}

impl AccountSerialize for MakerQuote {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.maker.as_ref());
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(self.delegate.as_ref());
        data.extend_from_slice(&self.quote_id_le);
        data.extend_from_slice(&self.sequence_le);
        data.extend_from_slice(&self.mid_tick_le);
        data.extend_from_slice(&self.last_update_slot_le);
        data.extend_from_slice(&self.expiry_slots_le);
        data.extend_from_slice(&self.folded_auction_id_le);
        data.extend_from_slice(&self.settled_auction_id_le);
        data.push(self.num_bids);
        data.push(self.num_asks);
        data.push(self.status);
        data.push(self.bump);
        data.extend_from_slice(&self.bid_levels_le);
        data.extend_from_slice(&self.ask_levels_le);
        data.extend_from_slice(&self.bid_snapshots_le);
        data.extend_from_slice(&self.ask_snapshots_le);
        data.extend_from_slice(&self.quote_index_le);
        data.extend_from_slice(&self.reserved_margin_le);
        data.extend_from_slice(&self.worst_price_le);
        data
    }
}

impl PdaSeeds for MakerQuote {
    const PREFIX: &'static [u8] = b"maker_quote";

    #[inline(always)]
    fn seeds(&self) -> Vec<&[u8]> {
        vec![
            Self::PREFIX,
            self.market.as_ref(),
            self.maker.as_ref(),
            &self.quote_index_le,
        ]
    }

    #[inline(always)]
    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.market.as_ref()),
            Seed::from(self.maker.as_ref()),
            Seed::from(self.quote_index_le.as_slice()),
            Seed::from(bump.as_slice()),
        ]
    }
}

impl PdaAccount for MakerQuote {
    #[inline(always)]
    fn bump(&self) -> u8 {
        self.bump
    }
}

impl MakerQuote {
    le_field!(quote_id, set_quote_id, quote_id_le, u64);
    le_field!(sequence, set_sequence, sequence_le, u64);
    le_field!(mid_tick, set_mid_tick, mid_tick_le, u32);
    le_field!(
        last_update_slot,
        set_last_update_slot,
        last_update_slot_le,
        u64
    );
    le_field!(expiry_slots, set_expiry_slots, expiry_slots_le, u64);
    le_field!(
        folded_auction_id,
        set_folded_auction_id,
        folded_auction_id_le,
        u64
    );
    le_field!(
        settled_auction_id,
        set_settled_auction_id,
        settled_auction_id_le,
        u64
    );
    le_field!(quote_index, set_quote_index, quote_index_le, u16);
    le_field!(
        reserved_margin,
        set_reserved_margin,
        reserved_margin_le,
        u64
    );
    le_field!(worst_price, set_worst_price, worst_price_le, u64);

    /// Read bid level `i` as `(offset_ticks, size)`.
    #[inline(always)]
    pub fn bid_level(&self, i: usize) -> (u16, u64) {
        read_level(&self.bid_levels_le, i)
    }

    /// Read ask level `i` as `(offset_ticks, size)`.
    #[inline(always)]
    pub fn ask_level(&self, i: usize) -> (u16, u64) {
        read_level(&self.ask_levels_le, i)
    }

    #[inline(always)]
    pub fn set_bid_level(&mut self, i: usize, offset: u16, size: u64) {
        write_level(&mut self.bid_levels_le, i, offset, size);
    }

    #[inline(always)]
    pub fn set_ask_level(&mut self, i: usize, offset: u16, size: u64) {
        write_level(&mut self.ask_levels_le, i, offset, size);
    }

    /// Read bid level `i`'s fold snapshot (`cum_before`); `SNAPSHOT_UNFOLDED` if
    /// the level was not folded this round.
    #[inline(always)]
    pub fn bid_snapshot(&self, i: usize) -> u64 {
        read_snapshot(&self.bid_snapshots_le, i)
    }

    /// Read ask level `i`'s fold snapshot. See [`MakerQuote::bid_snapshot`].
    #[inline(always)]
    pub fn ask_snapshot(&self, i: usize) -> u64 {
        read_snapshot(&self.ask_snapshots_le, i)
    }

    #[inline(always)]
    pub fn set_bid_snapshot(&mut self, i: usize, cum_before: u64) {
        write_snapshot(&mut self.bid_snapshots_le, i, cum_before);
    }

    #[inline(always)]
    pub fn set_ask_snapshot(&mut self, i: usize, cum_before: u64) {
        write_snapshot(&mut self.ask_snapshots_le, i, cum_before);
    }

    /// Reset every level's snapshot (both sides) to `SNAPSHOT_UNFOLDED`. Called at
    /// the start of each fold so a level not folded this round (off-grid, or the
    /// whole quote expired) carries the sentinel and fills zero in settlement —
    /// never a stale prefix from a prior round.
    #[inline(always)]
    pub fn reset_snapshots(&mut self) {
        self.bid_snapshots_le = [0xFFu8; SNAPSHOTS_LEN];
        self.ask_snapshots_le = [0xFFu8; SNAPSHOTS_LEN];
    }

    /// True if the quote has gone stale past its `expiry_slots` window.
    #[inline(always)]
    pub fn is_expired(&self, now_slot: u64) -> bool {
        let exp = self.expiry_slots();
        exp != 0 && now_slot.saturating_sub(self.last_update_slot()) > exp
    }

    /// Zero the ladders + counts (the levels region of a fresh account is already 0).
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    pub fn new(
        bump: u8,
        maker: Address,
        market: Address,
        delegate: Address,
        quote_id: u64,
        expiry_slots: u64,
        last_update_slot: u64,
        quote_index: u16,
    ) -> Self {
        Self {
            maker,
            market,
            delegate,
            quote_id_le: quote_id.to_le_bytes(),
            sequence_le: 0u64.to_le_bytes(),
            mid_tick_le: 0u32.to_le_bytes(),
            last_update_slot_le: last_update_slot.to_le_bytes(),
            expiry_slots_le: expiry_slots.to_le_bytes(),
            folded_auction_id_le: u64::MAX.to_le_bytes(),
            settled_auction_id_le: u64::MAX.to_le_bytes(),
            num_bids: 0,
            num_asks: 0,
            status: 1,
            bump,
            bid_levels_le: [0u8; LEVELS_LEN],
            ask_levels_le: [0u8; LEVELS_LEN],
            // Snapshots start "unfolded" (all-0xFF = u64::MAX per slot) so a quote
            // settled before its first fold fills zero rather than reading stale 0s.
            bid_snapshots_le: [0xFFu8; SNAPSHOTS_LEN],
            ask_snapshots_le: [0xFFu8; SNAPSHOTS_LEN],
            quote_index_le: quote_index.to_le_bytes(),
            reserved_margin_le: 0u64.to_le_bytes(),
            worst_price_le: 0u64.to_le_bytes(),
        }
    }
}

#[inline(always)]
fn read_level(region: &[u8; LEVELS_LEN], i: usize) -> (u16, u64) {
    let base = i * LEVEL_BYTES;
    let offset = u16::from_le_bytes([region[base], region[base + 1]]);
    let size = u64::from_le_bytes(region[base + 2..base + LEVEL_BYTES].try_into().unwrap());
    (offset, size)
}

#[inline(always)]
fn write_level(region: &mut [u8; LEVELS_LEN], i: usize, offset: u16, size: u64) {
    let base = i * LEVEL_BYTES;
    region[base..base + 2].copy_from_slice(&offset.to_le_bytes());
    region[base + 2..base + LEVEL_BYTES].copy_from_slice(&size.to_le_bytes());
}

#[inline(always)]
fn read_snapshot(region: &[u8; SNAPSHOTS_LEN], i: usize) -> u64 {
    let base = i * SNAPSHOT_BYTES;
    u64::from_le_bytes(region[base..base + SNAPSHOT_BYTES].try_into().unwrap())
}

#[inline(always)]
fn write_snapshot(region: &mut [u8; SNAPSHOTS_LEN], i: usize, cum_before: u64) {
    let base = i * SNAPSHOT_BYTES;
    region[base..base + SNAPSHOT_BYTES].copy_from_slice(&cum_before.to_le_bytes());
}

/// Validate writer authority: the signer must be the maker or the delegate.
#[inline(always)]
pub fn require_quote_writer(quote: &MakerQuote, signer: &Address) -> Result<(), ProgramError> {
    if *signer != quote.maker && *signer != quote.delegate {
        return Err(TempoProgramError::InvalidAuthority.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Discriminator;

    fn quote() -> MakerQuote {
        MakerQuote::new(
            255,
            Address::new_from_array([1u8; 32]),
            Address::new_from_array([2u8; 32]),
            Address::new_from_array([3u8; 32]),
            7,
            2,
            100,
            1, // quote_index
        )
    }

    #[test]
    fn test_roundtrip() {
        let mut q = quote();
        q.set_mid_tick(40);
        q.set_sequence(9);
        q.num_bids = 2;
        q.num_asks = 1;
        q.set_bid_level(0, 1, 500);
        q.set_bid_level(1, 3, 700);
        q.set_ask_level(0, 2, 600);
        // A fresh quote's snapshots are all the unfolded sentinel; set a couple.
        q.set_bid_snapshot(1, 250);
        q.set_ask_snapshot(0, 0);

        let bytes = q.to_bytes();
        assert_eq!(bytes.len(), MakerQuote::LEN);
        assert_eq!(bytes[0], MakerQuote::DISCRIMINATOR);
        assert_eq!(bytes[1], MakerQuote::VERSION);
        assert_eq!(MakerQuote::VERSION, 4);

        let de = MakerQuote::from_bytes(&bytes).unwrap();
        assert_eq!(de.quote_id(), 7);
        assert_eq!(de.mid_tick(), 40);
        assert_eq!(de.sequence(), 9);
        assert_eq!(de.num_bids, 2);
        assert_eq!(de.bid_level(0), (1, 500));
        assert_eq!(de.bid_level(1), (3, 700));
        assert_eq!(de.ask_level(0), (2, 600));
        assert_eq!(de.maker, q.maker);
        assert_eq!(de.delegate, q.delegate);
        // Snapshots round-trip: index 0 defaults to the sentinel, the set ones stick.
        assert_eq!(de.bid_snapshot(0), SNAPSHOT_UNFOLDED);
        assert_eq!(de.bid_snapshot(1), 250);
        assert_eq!(de.ask_snapshot(0), 0);
        assert_eq!(de.ask_snapshot(1), SNAPSHOT_UNFOLDED);
        // v4 fields round-trip; a fresh quote carries no reservation.
        assert_eq!(de.quote_index(), 1);
        assert_eq!(de.reserved_margin(), 0);
        assert_eq!(de.worst_price(), 0);
    }

    #[test]
    fn test_quote_index_is_a_seed() {
        // Two quotes differing only in quote_index must derive DIFFERENT PDAs
        // (known-issues §4.9 — this is what allows concurrent ladders).
        let a = quote(); // index 1
        let mut b = quote();
        b.set_quote_index(2);
        assert_ne!(a.seeds()[3], b.seeds()[3]);
        assert_eq!(a.seeds().len(), 4);
    }

    #[test]
    fn test_reset_snapshots() {
        let mut q = quote();
        q.set_bid_snapshot(0, 11);
        q.set_ask_snapshot(2, 22);
        q.reset_snapshots();
        for i in 0..MAX_LEVELS {
            assert_eq!(q.bid_snapshot(i), SNAPSHOT_UNFOLDED);
            assert_eq!(q.ask_snapshot(i), SNAPSHOT_UNFOLDED);
        }
    }

    #[test]
    fn test_expiry() {
        let q = quote(); // expiry_slots = 2, last_update_slot = 100
        assert!(!q.is_expired(101));
        assert!(!q.is_expired(102));
        assert!(q.is_expired(103));
        let mut never = quote();
        never.set_expiry_slots(0);
        assert!(!never.is_expired(10_000));
    }

    #[test]
    fn test_writer_authority() {
        let q = quote();
        assert!(require_quote_writer(&q, &Address::new_from_array([1u8; 32])).is_ok()); // maker
        assert!(require_quote_writer(&q, &Address::new_from_array([3u8; 32])).is_ok()); // delegate
        assert_eq!(
            require_quote_writer(&q, &Address::new_from_array([9u8; 32])),
            Err(TempoProgramError::InvalidAuthority.into())
        );
    }
}
