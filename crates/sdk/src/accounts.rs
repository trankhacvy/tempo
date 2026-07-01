//! Account decoders.
//!
//! The on-disk layout is `[disc(1)][version(1)][fields…]`, but the codama
//! account model omits the version byte, so its `from_bytes` is off by one. We
//! therefore hand-roll little-endian readers over the documented byte offsets
//! (the same approach the integration-test harness uses), which is also robust
//! to the generated client lagging the program version.

use solana_pubkey::Pubkey;

use crate::error::SdkError;

const MARKET_DISCRIMINATOR: u8 = 1;

/// Decoded view of the per-market control block (`program/src/state/market.rs`),
/// carrying the fields the keeper, market-maker, and preflight checks need.
/// Field offsets are relative to the start of account data (after the 2-byte
/// disc+version prefix). `initial_margin_bps`/`max_position_notional` are v8
/// fields; on a shorter account they read as `0` (the program's
/// `initial_margin_bps` falls back to maintenance when zero). PERF-1 (Market v9)
/// removed the redundant `accumulated_order_count`/`active_order_count` mirror
/// fields, shifting every field after `tick_size` down 16 bytes (known-issues §2.1):
/// the authoritative live-order count is `OrderSlabView.count` and the authoritative
/// folded count is `HistogramView.accumulated_count`.
#[derive(Clone, Debug)]
pub struct MarketView {
    pub version: u8,
    pub current_auction_id: u64,
    pub phase_deadline_slot: u64,
    pub tick_size: u64,
    pub orders_per_auction_cap: u32,
    pub num_ticks: u32,
    pub oracle: Pubkey,
    pub phase: u8,
    pub last_funding_ts: u64,
    pub oracle_feed_id: [u8; 32],
    pub maintenance_margin_bps: u16,
    pub collateral_mint: Pubkey,
    pub active_maker_quote_count: u64,
    pub folded_maker_quote_count: u64,
    pub window_floor_price: u64,
    pub initial_margin_bps: u16,
    pub max_position_notional: u128,
    /// Number of OrderSlab shards (Stage A). 1 when the field is absent (pre-shard account).
    pub num_slab_shards: u16,
}

impl MarketView {
    /// Smallest account length that carries every field through `window_floor_price`
    /// (offset 376 + 8). v8 appends `initial_margin_bps` + `max_position_notional`;
    /// Market v9 (PERF-1) dropped the two order-count mirrors (−16 bytes).
    const MIN_LEN: usize = 384;

    pub fn decode(data: &[u8]) -> Result<Self, SdkError> {
        if data.len() < Self::MIN_LEN {
            return Err(SdkError::Decode(format!(
                "market data too short: {} < {}",
                data.len(),
                Self::MIN_LEN
            )));
        }
        if data[0] != MARKET_DISCRIMINATOR {
            return Err(SdkError::Decode(format!(
                "unexpected market discriminator {}",
                data[0]
            )));
        }
        // v8 appends the initial-margin buffer + per-position notional cap. A shorter
        // account simply lacks them; report 0 so the maintenance fallback applies.
        // (Offsets are −16 vs the pre-v9 layout: PERF-1 removed the two order-count
        // mirrors that used to sit at 42/50.)
        let initial_margin_bps = if data.len() >= 386 {
            u16_at(data, 384)
        } else {
            0
        };
        let max_position_notional = if data.len() >= 402 {
            u128_at(data, 386)
        } else {
            0
        };
        // v10+ appends `num_slab_shards` (u16) at offset 402. Design Z (v11) removed the
        // `shards_pending` counter that used to follow it; `shards_ready` now sits at 404.
        let num_slab_shards = if data.len() >= 404 {
            u16_at(data, 402)
        } else {
            1
        };
        Ok(Self {
            version: data[1],
            current_auction_id: u64_at(data, 2),
            phase_deadline_slot: u64_at(data, 10),
            tick_size: u64_at(data, 18),
            orders_per_auction_cap: u32_at(data, 42),
            num_ticks: u32_at(data, 46),
            oracle: pubkey_at(data, 114),
            phase: data[146],
            last_funding_ts: u64_at(data, 164),
            oracle_feed_id: array32_at(data, 172),
            maintenance_margin_bps: u16_at(data, 204),
            collateral_mint: pubkey_at(data, 222),
            active_maker_quote_count: u64_at(data, 262),
            folded_maker_quote_count: u64_at(data, 270),
            window_floor_price: u64_at(data, 376),
            initial_margin_bps,
            max_position_notional,
            num_slab_shards,
        })
    }

    /// The initial-margin requirement in bps, falling back to maintenance when the
    /// v8 field is absent/zero (matches `Market::initial_margin_bps`).
    pub fn effective_initial_margin_bps(&self) -> u16 {
        if self.initial_margin_bps == 0 {
            self.maintenance_margin_bps
        } else {
            self.initial_margin_bps
        }
    }

    /// The highest in-window price (top tick) — a sell can clear no higher, so it
    /// bounds the sell-side worst-case fill (mirrors `submit_order`'s `window_top`).
    pub fn window_top_price(&self) -> u64 {
        self.window_floor_price + (self.num_ticks.saturating_sub(1) as u64) * self.tick_size
    }
}

const ORDER_SLAB_DISCRIMINATOR: u8 = 4;
const CLEARING_DISCRIMINATOR: u8 = 3;
const MAKER_QUOTE_DISCRIMINATOR: u8 = 8;
const HISTOGRAM_DISCRIMINATOR: u8 = 2;
const POSITION_DISCRIMINATOR: u8 = 5;
const USER_COLLATERAL_DISCRIMINATOR: u8 = 7;
const VAULT_DISCRIMINATOR: u8 = 6;
const MARGIN_ACCOUNT_DISCRIMINATOR: u8 = 9;

/// The four dual-auction bucket arrays of an `AuctionHistogram`
/// (`program/src/state/histogram.rs`). On-disk layout:
/// `[disc(1)][version(1)][header(53)][bid_demand][bid_supply][ask_demand][ask_supply]`,
/// each region `num_ticks` LE `u64` buckets. The header carries `auction_id@2`,
/// `accumulated_count@10`, `num_ticks@18`, `market@22`, `bump@54`; buckets begin
/// at offset 55.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistogramView {
    pub auction_id: u64,
    pub num_ticks: u32,
    pub bid_demand: Vec<u64>,
    pub bid_supply: Vec<u64>,
    pub ask_demand: Vec<u64>,
    pub ask_supply: Vec<u64>,
}

/// Offset of the first histogram bucket (2-byte prefix + `AuctionHistogramHeader`
/// `DATA_LEN` = 53).
const HIST_BUCKETS_OFFSET: usize = 55;

impl HistogramView {
    pub fn decode(data: &[u8]) -> Result<Self, SdkError> {
        if data.len() < HIST_BUCKETS_OFFSET {
            return Err(SdkError::Decode(format!(
                "histogram too short: {} < {}",
                data.len(),
                HIST_BUCKETS_OFFSET
            )));
        }
        if data[0] != HISTOGRAM_DISCRIMINATOR {
            return Err(SdkError::Decode(format!(
                "unexpected histogram discriminator {}",
                data[0]
            )));
        }
        let num_ticks = u32_at(data, 18);
        let t = num_ticks as usize;
        let need = HIST_BUCKETS_OFFSET + 4 * t * 8;
        if data.len() < need {
            return Err(SdkError::Decode(format!(
                "histogram too short for {} ticks: {} < {}",
                num_ticks,
                data.len(),
                need
            )));
        }
        let region = |r: usize| -> Vec<u64> {
            let base = HIST_BUCKETS_OFFSET + r * t * 8;
            (0..t).map(|i| u64_at(data, base + i * 8)).collect()
        };
        Ok(Self {
            auction_id: u64_at(data, 2),
            num_ticks,
            bid_demand: region(0),
            bid_supply: region(1),
            ask_demand: region(2),
            ask_supply: region(3),
        })
    }
}

/// Decoded view of a `Position` (`program/src/state/position.rs`, VERSION 3).
/// Offsets are relative to account-data start: `owner@2 market@34 size@66
/// entry_price@74 collateral@82 realized_pnl@90 last_funding_index@106 bump@122
/// last_social_index@123 margin_mode@139`. `size` is signed (+ long, − short).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositionView {
    pub owner: Pubkey,
    pub market: Pubkey,
    pub size: i64,
    pub entry_price: u64,
    pub collateral: u64,
    pub realized_pnl: i128,
    pub margin_mode: u8,
}

impl PositionView {
    /// Smallest length that carries `margin_mode` (offset 139, 1 byte).
    const MIN_LEN: usize = 140;

    pub fn decode(data: &[u8]) -> Result<Self, SdkError> {
        if data.len() < Self::MIN_LEN {
            return Err(SdkError::Decode(format!(
                "position too short: {} < {}",
                data.len(),
                Self::MIN_LEN
            )));
        }
        if data[0] != POSITION_DISCRIMINATOR {
            return Err(SdkError::Decode(format!(
                "unexpected position discriminator {}",
                data[0]
            )));
        }
        Ok(Self {
            owner: pubkey_at(data, 2),
            market: pubkey_at(data, 34),
            size: i64_at(data, 66),
            entry_price: u64_at(data, 74),
            collateral: u64_at(data, 82),
            realized_pnl: i128_at(data, 90),
            margin_mode: data[139],
        })
    }
}

/// Decoded view of a `UserCollateral` ledger (`program/src/state/user_collateral.rs`,
/// VERSION 2 — mint-scoped, CR-3). Offsets: `owner@2 collateral_mint@34 balance@66
/// locked@74`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserCollateralView {
    pub owner: Pubkey,
    pub collateral_mint: Pubkey,
    pub balance: u64,
    pub locked: u64,
}

impl UserCollateralView {
    /// Smallest length that carries `locked` (offset 74, 8 bytes).
    const MIN_LEN: usize = 82;

    pub fn decode(data: &[u8]) -> Result<Self, SdkError> {
        if data.len() < Self::MIN_LEN {
            return Err(SdkError::Decode(format!(
                "user collateral too short: {} < {}",
                data.len(),
                Self::MIN_LEN
            )));
        }
        if data[0] != USER_COLLATERAL_DISCRIMINATOR {
            return Err(SdkError::Decode(format!(
                "unexpected user-collateral discriminator {}",
                data[0]
            )));
        }
        Ok(Self {
            owner: pubkey_at(data, 2),
            collateral_mint: pubkey_at(data, 34),
            balance: u64_at(data, 66),
            locked: u64_at(data, 74),
        })
    }

    /// Collateral not already reserved against open orders/positions.
    pub fn free(&self) -> u64 {
        self.balance.saturating_sub(self.locked)
    }
}

/// Decoded view of the global `Vault` (`program/src/state/vault.rs`, VERSION 2).
/// Offsets relative to account-data start: `collateral_mint@2
/// vault_token_account@34 insurance_balance@66`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaultView {
    pub collateral_mint: Pubkey,
    pub vault_token_account: Pubkey,
    pub insurance_balance: u64,
}

impl VaultView {
    /// Smallest length carrying `insurance_balance` + the two trailing bumps.
    const MIN_LEN: usize = 76;

    pub fn decode(data: &[u8]) -> Result<Self, SdkError> {
        if data.len() < Self::MIN_LEN {
            return Err(SdkError::Decode(format!(
                "vault too short: {} < {}",
                data.len(),
                Self::MIN_LEN
            )));
        }
        if data[0] != VAULT_DISCRIMINATOR {
            return Err(SdkError::Decode(format!(
                "unexpected vault discriminator {}",
                data[0]
            )));
        }
        Ok(Self {
            collateral_mint: pubkey_at(data, 2),
            vault_token_account: pubkey_at(data, 34),
            insurance_balance: u64_at(data, 66),
        })
    }
}

/// Decoded view of a cross-margin `MarginAccount`
/// (`program/src/state/margin_account.rs`, disc 9, VERSION 1) — the owner's member
/// position set. Not in the IDL (its fixed `[u8; 256]` member array has no Codama
/// node). Offsets: `owner@2 position_count@34 bump@35 members@36` (32-byte keys).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarginAccountView {
    pub owner: Pubkey,
    pub bump: u8,
    pub members: Vec<Pubkey>,
}

impl MarginAccountView {
    /// Header through `bump`; the member array follows.
    const MIN_LEN: usize = 36;
    const MAX_MEMBERS: usize = 8;

    pub fn decode(data: &[u8]) -> Result<Self, SdkError> {
        if data.len() < Self::MIN_LEN {
            return Err(SdkError::Decode(format!(
                "margin account too short: {} < {}",
                data.len(),
                Self::MIN_LEN
            )));
        }
        if data[0] != MARGIN_ACCOUNT_DISCRIMINATOR {
            return Err(SdkError::Decode(format!(
                "unexpected margin-account discriminator {}",
                data[0]
            )));
        }
        let count = (data[34] as usize).min(Self::MAX_MEMBERS);
        let need = 36 + count * 32;
        if data.len() < need {
            return Err(SdkError::Decode(format!(
                "margin account too short for {} members: {} < {}",
                count,
                data.len(),
                need
            )));
        }
        let members = (0..count).map(|i| pubkey_at(data, 36 + i * 32)).collect();
        Ok(Self {
            owner: pubkey_at(data, 2),
            bump: data[35],
            members,
        })
    }
}

/// Order-status bytes (`state/order.rs::OrderStatus`).
pub const STATUS_EMPTY: u8 = 0;
pub const STATUS_RESTING: u8 = 1;
pub const STATUS_ACCUMULATED: u8 = 2;
pub const STATUS_CONSUMED: u8 = 3;

/// One decoded slab slot. Slab on-disk layout (`state/order.rs`, v4 sharding):
/// `[disc(1)][version(1)][OrderSlabHeader(75)][Order; capacity]` ⇒ slots start at
/// offset 77, each `Order` is `ORDER_LEN = 88` bytes. Within a slot:
/// `price@0 qty@8 remaining@16 order_id@24 trader@32..64 side@64 is_maker@65
/// status@66 cum_before@72 reserved_margin@80`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlabOrder {
    /// Array index of this slot — this IS the `slot_hint` passed to `settle_fill`.
    pub slot: u32,
    pub order_id: u64,
    pub trader: Pubkey,
    pub side: u8,
    pub status: u8,
    pub price: u64,
    pub quantity: u64,
}

/// Offset of the first order slot in the slab account data (2-byte prefix +
/// `OrderSlabHeader::DATA_LEN` = 75 in v4 sharding: the base 61 + shard_id(2) +
/// resting_count(4) + folded_auction_id(8)).
const SLAB_SLOTS_OFFSET: usize = 77;

/// Decode every non-empty slot from a raw `OrderSlab` account. Empty slots
/// (`status == 0`) are skipped; the returned `slot` field is the on-disk index.
pub fn decode_slab_orders(data: &[u8]) -> Result<Vec<SlabOrder>, SdkError> {
    use crate::consts::ORDER_LEN;
    if data.len() < SLAB_SLOTS_OFFSET {
        return Err(SdkError::Decode(format!(
            "order slab too short: {} < {}",
            data.len(),
            SLAB_SLOTS_OFFSET
        )));
    }
    if data[0] != ORDER_SLAB_DISCRIMINATOR {
        return Err(SdkError::Decode(format!(
            "unexpected order-slab discriminator {}",
            data[0]
        )));
    }
    let mut out = Vec::new();
    let mut off = SLAB_SLOTS_OFFSET;
    let mut slot = 0u32;
    while off + ORDER_LEN <= data.len() {
        let status = data[off + 66];
        if status != STATUS_EMPTY {
            out.push(SlabOrder {
                slot,
                price: u64_at(data, off),
                quantity: u64_at(data, off + 8),
                order_id: u64_at(data, off + 24),
                trader: pubkey_at(data, off + 32),
                side: data[off + 64],
                status,
            });
        }
        off += ORDER_LEN;
        slot += 1;
    }
    Ok(out)
}

/// Mirror of `ClearingResult` (`state/clearing_result.rs`), carrying the fields
/// the keeper needs (round id + per-side matched volumes — a 0/0 round can roll
/// straight through settle) plus the published cross the UI draws (per-side
/// clearing price + marginal tick). Offsets relative to account-data start:
/// `auction_id@2 bid_clearing_price@10 ask_clearing_price@18 bid_matched@26
/// ask_matched@34 bid_marginal_tick@74 ask_marginal_tick@78`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearingResultView {
    pub auction_id: u64,
    pub bid_clearing_price: u64,
    pub ask_clearing_price: u64,
    pub bid_matched_volume: u64,
    pub ask_matched_volume: u64,
    pub bid_marginal_tick: u32,
    pub ask_marginal_tick: u32,
}

impl ClearingResultView {
    /// Smallest length that carries `ask_marginal_tick` (offset 78, 4 bytes).
    const MIN_LEN: usize = 82;

    pub fn decode(data: &[u8]) -> Result<Self, SdkError> {
        if data.len() < Self::MIN_LEN {
            return Err(SdkError::Decode(format!(
                "clearing result too short: {} < {}",
                data.len(),
                Self::MIN_LEN
            )));
        }
        if data[0] != CLEARING_DISCRIMINATOR {
            return Err(SdkError::Decode(format!(
                "unexpected clearing discriminator {}",
                data[0]
            )));
        }
        Ok(Self {
            auction_id: u64_at(data, 2),
            bid_clearing_price: u64_at(data, 10),
            ask_clearing_price: u64_at(data, 18),
            bid_matched_volume: u64_at(data, 26),
            ask_matched_volume: u64_at(data, 34),
            bid_marginal_tick: u32_at(data, 74),
            ask_marginal_tick: u32_at(data, 78),
        })
    }
}

/// Decoded view of a `MakerQuote` (`state/maker_quote.rs`, VERSION 3). Offsets are
/// relative to account-data start (after the 2-byte prefix): `maker@2 market@34
/// sequence@106 mid_tick@114 folded_auction_id@134 settled_auction_id@142
/// num_bids@150 num_asks@151 status@152`. The status byte is at **152** (verified
/// against the v3 struct) — the legacy TS `154` is wrong. `sequence` lets a
/// restarting writer resume from the on-chain monotonic counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MakerQuoteView {
    pub maker: Pubkey,
    pub market: Pubkey,
    pub sequence: u64,
    pub mid_tick: u32,
    pub status: u8,
    pub num_bids: u8,
    pub num_asks: u8,
    pub folded_auction_id: u64,
    pub settled_auction_id: u64,
}

impl MakerQuoteView {
    /// Smallest length that carries `status` (offset 152).
    const MIN_LEN: usize = 153;

    pub fn decode(data: &[u8]) -> Result<Self, SdkError> {
        if data.len() < Self::MIN_LEN {
            return Err(SdkError::Decode(format!(
                "maker quote too short: {} < {}",
                data.len(),
                Self::MIN_LEN
            )));
        }
        if data[0] != MAKER_QUOTE_DISCRIMINATOR {
            return Err(SdkError::Decode(format!(
                "unexpected maker-quote discriminator {}",
                data[0]
            )));
        }
        Ok(Self {
            maker: pubkey_at(data, 2),
            market: pubkey_at(data, 34),
            sequence: u64_at(data, 106),
            mid_tick: u32_at(data, 114),
            folded_auction_id: u64_at(data, 134),
            settled_auction_id: u64_at(data, 142),
            num_bids: data[150],
            num_asks: data[151],
            status: data[152],
        })
    }

    /// Active quote that has not yet folded into `auction_id` — needs a
    /// `process_maker_quote`.
    pub fn needs_fold(&self, auction_id: u64) -> bool {
        self.status == 1 && self.folded_auction_id != auction_id
    }

    /// Quote folded into `auction_id` but not yet settled for it — needs a
    /// `settle_maker_quote` before the round may roll.
    pub fn needs_settle(&self, auction_id: u64) -> bool {
        self.status == 1
            && self.folded_auction_id == auction_id
            && self.settled_auction_id != auction_id
    }
}

#[inline]
fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(d[o..o + 2].try_into().expect("len checked"))
}

#[inline]
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().expect("len checked"))
}

#[inline]
fn u64_at(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().expect("len checked"))
}

#[inline]
fn u128_at(d: &[u8], o: usize) -> u128 {
    u128::from_le_bytes(d[o..o + 16].try_into().expect("len checked"))
}

#[inline]
fn i64_at(d: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(d[o..o + 8].try_into().expect("len checked"))
}

#[inline]
fn i128_at(d: &[u8], o: usize) -> i128 {
    i128::from_le_bytes(d[o..o + 16].try_into().expect("len checked"))
}

#[inline]
fn array32_at(d: &[u8], o: usize) -> [u8; 32] {
    d[o..o + 32].try_into().expect("len checked")
}

#[inline]
fn pubkey_at(d: &[u8], o: usize) -> Pubkey {
    Pubkey::new_from_array(array32_at(d, o))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden test for the offset table: lay out a synthetic v9 Market with known
    /// values at the documented offsets and assert `MarketView` reads them back.
    /// Locks the layout against drift without depending on the program crate. PERF-1
    /// removed the two order-count mirror fields, so every field after `tick_size`
    /// sits 16 bytes lower than the pre-v9 layout (total length 418 → 402).
    #[test]
    fn test_market_view_offsets() {
        let mut d = vec![0u8; 402];
        d[0] = MARKET_DISCRIMINATOR; // disc
        d[1] = 9; // version
        d[2..10].copy_from_slice(&7u64.to_le_bytes()); // current_auction_id
        d[10..18].copy_from_slice(&12345u64.to_le_bytes()); // phase_deadline_slot
        d[18..26].copy_from_slice(&10u64.to_le_bytes()); // tick_size
        d[42..46].copy_from_slice(&24u32.to_le_bytes()); // orders_per_auction_cap
        d[46..50].copy_from_slice(&148u32.to_le_bytes()); // num_ticks
        let oracle = [9u8; 32];
        d[114..146].copy_from_slice(&oracle); // oracle
        d[146] = 1; // phase = Accumulating
        d[164..172].copy_from_slice(&111u64.to_le_bytes()); // last_funding_ts
        d[172..204].copy_from_slice(&[5u8; 32]); // oracle_feed_id
        d[204..206].copy_from_slice(&500u16.to_le_bytes()); // maintenance_margin_bps
        let mint = [3u8; 32];
        d[222..254].copy_from_slice(&mint); // collateral_mint
        d[262..270].copy_from_slice(&6u64.to_le_bytes()); // active_maker_quote_count
        d[270..278].copy_from_slice(&2u64.to_le_bytes()); // folded_maker_quote_count
        d[376..384].copy_from_slice(&9_680u64.to_le_bytes()); // window_floor_price
        d[384..386].copy_from_slice(&600u16.to_le_bytes()); // initial_margin_bps
        d[386..402].copy_from_slice(&1_000_000u128.to_le_bytes()); // max_position_notional

        let m = MarketView::decode(&d).unwrap();
        assert_eq!(m.version, 9);
        assert_eq!(m.current_auction_id, 7);
        assert_eq!(m.phase_deadline_slot, 12345);
        assert_eq!(m.tick_size, 10);
        assert_eq!(m.orders_per_auction_cap, 24);
        assert_eq!(m.num_ticks, 148);
        assert_eq!(m.oracle, Pubkey::new_from_array(oracle));
        assert_eq!(m.phase, 1);
        assert_eq!(m.last_funding_ts, 111);
        assert_eq!(m.oracle_feed_id, [5u8; 32]);
        assert_eq!(m.maintenance_margin_bps, 500);
        assert_eq!(m.collateral_mint, Pubkey::new_from_array(mint));
        assert_eq!(m.active_maker_quote_count, 6);
        assert_eq!(m.folded_maker_quote_count, 2);
        assert_eq!(m.window_floor_price, 9_680);
        assert_eq!(m.initial_margin_bps, 600);
        assert_eq!(m.effective_initial_margin_bps(), 600);
        assert_eq!(m.max_position_notional, 1_000_000);
        assert_eq!(m.window_top_price(), 9_680 + 147 * 10);
    }

    #[test]
    fn test_market_view_rejects_bad_disc_and_short() {
        let mut d = vec![0u8; 402];
        d[0] = 2;
        assert!(MarketView::decode(&d).is_err());
        assert!(MarketView::decode(&[1u8, 9, 0]).is_err());
    }

    #[test]
    fn test_market_view_short_initial_margin_falls_back() {
        let mut d = vec![0u8; 384]; // through window_floor_price: no v8 appends
        d[0] = MARKET_DISCRIMINATOR;
        d[1] = 9;
        d[204..206].copy_from_slice(&500u16.to_le_bytes());
        let m = MarketView::decode(&d).unwrap();
        assert_eq!(m.initial_margin_bps, 0);
        assert_eq!(m.effective_initial_margin_bps(), 500);
        assert_eq!(m.max_position_notional, 0);
    }

    /// Golden test for the slab slot offsets: two non-empty slots + one empty,
    /// asserting `decode_slab_orders` reads each field and skips the empty slot.
    #[test]
    fn test_decode_slab_orders_offsets() {
        use crate::consts::ORDER_LEN;
        let cap = 3usize;
        let mut d = vec![0u8; SLAB_SLOTS_OFFSET + cap * ORDER_LEN];
        d[0] = ORDER_SLAB_DISCRIMINATOR;
        d[1] = 3; // version
        let write_slot = |d: &mut [u8],
                          i: usize,
                          oid: u64,
                          trader: [u8; 32],
                          side: u8,
                          status: u8,
                          price: u64,
                          qty: u64| {
            let b = SLAB_SLOTS_OFFSET + i * ORDER_LEN;
            d[b..b + 8].copy_from_slice(&price.to_le_bytes());
            d[b + 8..b + 16].copy_from_slice(&qty.to_le_bytes());
            d[b + 24..b + 32].copy_from_slice(&oid.to_le_bytes());
            d[b + 32..b + 64].copy_from_slice(&trader);
            d[b + 64] = side;
            d[b + 66] = status;
        };
        write_slot(&mut d, 0, 10, [1u8; 32], 0, STATUS_ACCUMULATED, 100, 5);
        // slot 1 left Empty (status 0) — must be skipped.
        write_slot(&mut d, 2, 12, [2u8; 32], 1, STATUS_RESTING, 200, 7);

        let orders = decode_slab_orders(&d).unwrap();
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].slot, 0);
        assert_eq!(orders[0].order_id, 10);
        assert_eq!(orders[0].trader, Pubkey::new_from_array([1u8; 32]));
        assert_eq!(orders[0].side, 0);
        assert_eq!(orders[0].status, STATUS_ACCUMULATED);
        assert_eq!(orders[0].price, 100);
        assert_eq!(orders[0].quantity, 5);
        assert_eq!(orders[1].slot, 2);
        assert_eq!(orders[1].order_id, 12);
        assert_eq!(orders[1].status, STATUS_RESTING);
    }

    #[test]
    fn test_decode_clearing_result_offsets() {
        let mut d = vec![0u8; 117]; // ClearingResult::LEN
        d[0] = CLEARING_DISCRIMINATOR;
        d[1] = 1;
        d[2..10].copy_from_slice(&42u64.to_le_bytes()); // auction_id
        d[10..18].copy_from_slice(&990u64.to_le_bytes()); // bid_clearing_price
        d[18..26].copy_from_slice(&1010u64.to_le_bytes()); // ask_clearing_price
        d[26..34].copy_from_slice(&1000u64.to_le_bytes()); // bid_matched_volume
        d[34..42].copy_from_slice(&500u64.to_le_bytes()); // ask_matched_volume
        d[74..78].copy_from_slice(&7u32.to_le_bytes()); // bid_marginal_tick
        d[78..82].copy_from_slice(&9u32.to_le_bytes()); // ask_marginal_tick
        let c = ClearingResultView::decode(&d).unwrap();
        assert_eq!(c.auction_id, 42);
        assert_eq!(c.bid_clearing_price, 990);
        assert_eq!(c.ask_clearing_price, 1010);
        assert_eq!(c.bid_matched_volume, 1000);
        assert_eq!(c.ask_matched_volume, 500);
        assert_eq!(c.bid_marginal_tick, 7);
        assert_eq!(c.ask_marginal_tick, 9);
        assert!(ClearingResultView::decode(&[3u8, 1, 0]).is_err());
    }

    #[test]
    fn test_decode_histogram_offsets() {
        let t = 2usize;
        let mut d = vec![0u8; HIST_BUCKETS_OFFSET + 4 * t * 8];
        d[0] = HISTOGRAM_DISCRIMINATOR;
        d[1] = 1;
        d[2..10].copy_from_slice(&13u64.to_le_bytes()); // auction_id
        d[18..22].copy_from_slice(&(t as u32).to_le_bytes()); // num_ticks
        let put = |d: &mut [u8], region: usize, tick: usize, v: u64| {
            let b = HIST_BUCKETS_OFFSET + region * t * 8 + tick * 8;
            d[b..b + 8].copy_from_slice(&v.to_le_bytes());
        };
        put(&mut d, 0, 0, 11); // bid_demand[0]
        put(&mut d, 0, 1, 12);
        put(&mut d, 1, 0, 21); // bid_supply[0]
        put(&mut d, 2, 1, 32); // ask_demand[1]
        put(&mut d, 3, 0, 43); // ask_supply[0]
        let h = HistogramView::decode(&d).unwrap();
        assert_eq!(h.auction_id, 13);
        assert_eq!(h.num_ticks, 2);
        assert_eq!(h.bid_demand, vec![11, 12]);
        assert_eq!(h.bid_supply, vec![21, 0]);
        assert_eq!(h.ask_demand, vec![0, 32]);
        assert_eq!(h.ask_supply, vec![43, 0]);
        // wrong disc + too-short-for-ticks both rejected.
        assert!(HistogramView::decode(&[1u8, 1, 0]).is_err());
        let mut short = d.clone();
        short.truncate(HIST_BUCKETS_OFFSET + 8);
        assert!(HistogramView::decode(&short).is_err());
    }

    #[test]
    fn test_decode_position_offsets() {
        let mut d = vec![0u8; 140];
        d[0] = POSITION_DISCRIMINATOR;
        d[1] = 3;
        d[2..34].copy_from_slice(&[4u8; 32]); // owner
        d[34..66].copy_from_slice(&[6u8; 32]); // market
        d[66..74].copy_from_slice(&(-25i64).to_le_bytes()); // size (short)
        d[74..82].copy_from_slice(&1000u64.to_le_bytes()); // entry_price
        d[82..90].copy_from_slice(&5000u64.to_le_bytes()); // collateral
        d[90..106].copy_from_slice(&(-7i128).to_le_bytes()); // realized_pnl
        d[139] = 1; // margin_mode = cross
        let p = PositionView::decode(&d).unwrap();
        assert_eq!(p.owner, Pubkey::new_from_array([4u8; 32]));
        assert_eq!(p.market, Pubkey::new_from_array([6u8; 32]));
        assert_eq!(p.size, -25);
        assert_eq!(p.entry_price, 1000);
        assert_eq!(p.collateral, 5000);
        assert_eq!(p.realized_pnl, -7);
        assert_eq!(p.margin_mode, 1);
        assert!(PositionView::decode(&[5u8, 3, 0]).is_err());
        assert!(PositionView::decode(&[2u8; 140]).is_err());
    }

    #[test]
    fn test_decode_vault_offsets() {
        let mut d = vec![0u8; 76];
        d[0] = VAULT_DISCRIMINATOR;
        d[1] = 2;
        d[2..34].copy_from_slice(&[3u8; 32]); // collateral_mint
        d[34..66].copy_from_slice(&[4u8; 32]); // vault_token_account
        d[66..74].copy_from_slice(&1_500_000u64.to_le_bytes()); // insurance_balance
        let v = VaultView::decode(&d).unwrap();
        assert_eq!(v.collateral_mint, Pubkey::new_from_array([3u8; 32]));
        assert_eq!(v.vault_token_account, Pubkey::new_from_array([4u8; 32]));
        assert_eq!(v.insurance_balance, 1_500_000);
        assert!(VaultView::decode(&[6u8, 2, 0]).is_err());
        assert!(VaultView::decode(&[1u8; 76]).is_err());
    }

    #[test]
    fn test_decode_margin_account_offsets() {
        let mut d = vec![0u8; 36 + 2 * 32];
        d[0] = MARGIN_ACCOUNT_DISCRIMINATOR;
        d[1] = 1;
        d[2..34].copy_from_slice(&[7u8; 32]); // owner
        d[34] = 2; // position_count
        d[35] = 254; // bump
        d[36..68].copy_from_slice(&[1u8; 32]); // member 0
        d[68..100].copy_from_slice(&[2u8; 32]); // member 1
        let m = MarginAccountView::decode(&d).unwrap();
        assert_eq!(m.owner, Pubkey::new_from_array([7u8; 32]));
        assert_eq!(m.bump, 254);
        assert_eq!(
            m.members,
            vec![
                Pubkey::new_from_array([1u8; 32]),
                Pubkey::new_from_array([2u8; 32])
            ]
        );
        // position_count past MAX_MEMBERS is clamped (and the short buffer rejected).
        let mut over = vec![0u8; 36 + 8 * 32];
        over[0] = MARGIN_ACCOUNT_DISCRIMINATOR;
        over[34] = 200;
        assert_eq!(MarginAccountView::decode(&over).unwrap().members.len(), 8);
        assert!(MarginAccountView::decode(&[9u8, 1, 0]).is_err());
        assert!(MarginAccountView::decode(&[2u8; 36]).is_err());
    }

    #[test]
    fn test_decode_user_collateral_offsets() {
        // CR-3 mint-scoped layout: owner@2 collateral_mint@34 balance@66 locked@74.
        let mut d = vec![0u8; 83];
        d[0] = USER_COLLATERAL_DISCRIMINATOR;
        d[1] = 2;
        d[2..34].copy_from_slice(&[8u8; 32]); // owner
        d[34..66].copy_from_slice(&[5u8; 32]); // collateral_mint
        d[66..74].copy_from_slice(&9000u64.to_le_bytes()); // balance
        d[74..82].copy_from_slice(&2500u64.to_le_bytes()); // locked
        let u = UserCollateralView::decode(&d).unwrap();
        assert_eq!(u.owner, Pubkey::new_from_array([8u8; 32]));
        assert_eq!(u.collateral_mint, Pubkey::new_from_array([5u8; 32]));
        assert_eq!(u.balance, 9000);
        assert_eq!(u.locked, 2500);
        assert_eq!(u.free(), 6500);
        assert!(UserCollateralView::decode(&[7u8, 2, 0]).is_err());
    }

    /// Golden test pinning the maker-quote offsets — status MUST read from 152.
    #[test]
    fn test_decode_maker_quote_offsets() {
        let mut d = vec![0u8; 442]; // MakerQuote full length (2 + DATA_LEN 440)
        d[0] = MAKER_QUOTE_DISCRIMINATOR;
        d[1] = 3;
        d[2..34].copy_from_slice(&[7u8; 32]); // maker
        d[34..66].copy_from_slice(&[9u8; 32]); // market
        d[106..114].copy_from_slice(&88u64.to_le_bytes()); // sequence
        d[114..118].copy_from_slice(&33u32.to_le_bytes()); // mid_tick
        d[134..142].copy_from_slice(&5u64.to_le_bytes()); // folded_auction_id
        d[142..150].copy_from_slice(&4u64.to_le_bytes()); // settled_auction_id
        d[150] = 2; // num_bids
        d[151] = 3; // num_asks
        d[152] = 1; // status (NOT 154)
        let q = MakerQuoteView::decode(&d).unwrap();
        assert_eq!(q.maker, Pubkey::new_from_array([7u8; 32]));
        assert_eq!(q.market, Pubkey::new_from_array([9u8; 32]));
        assert_eq!(q.sequence, 88);
        assert_eq!(q.mid_tick, 33);
        assert_eq!(q.folded_auction_id, 5);
        assert_eq!(q.settled_auction_id, 4);
        assert_eq!(q.num_bids, 2);
        assert_eq!(q.num_asks, 3);
        assert_eq!(q.status, 1);
        // round 5: folded but not settled (settled==4) → needs settle, not fold.
        assert!(q.needs_settle(5));
        assert!(!q.needs_fold(5));
        // round 6: not folded yet → needs fold.
        assert!(q.needs_fold(6));
        assert!(!q.needs_settle(6));
    }
}
