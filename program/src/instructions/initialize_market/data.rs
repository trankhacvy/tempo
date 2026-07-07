use pinocchio::error::ProgramError;

use crate::{
    errors::TempoProgramError,
    require_len,
    state::{OrderSlabHeader, ORDER_LEN},
    traits::{AccountSize, InstructionData},
};

/// Upper bound on `num_ticks`. The histogram is created via CPI, and Solana
/// caps a CPI-created/grown account at 10_240 bytes (`MAX_PERMITTED_DATA_INCREASE`),
/// so the histogram (`≈ 4 · num_ticks · 8` bytes + header) must fit: 256 ticks ≈
/// 8 KB, comfortably under the limit. (Larger tick counts need pre-sized accounts
/// grown over multiple txs, or sharding — a follow-up.)
pub const MAX_NUM_TICKS: u32 = 256;
/// Upper bound on `orders_per_auction_cap` (one OrderSlab shard's capacity). Each
/// shard is created by `init_shard` via a single CPI `CreateAccount`, which Solana
/// caps at 10_240 bytes (`MAX_PERMITTED_DATA_INCREASE`). At the current Stage-C1
/// `ORDER_LEN = 112` the single-`CreateAccount` ceiling is `(10_240 −
/// OrderSlabHeader::LEN(77)) / 112 = 90`, and we fix the cap at exactly **90**: sized for
/// this final `Order` size so shards never needed re-provisioning between Stage B (104) and
/// Stage C1 (112) — `77 + 90 · 112 = 10_157 ≤ 10_240` fits one `CreateAccount`. The `const _` assert
/// below fails the build if a future `ORDER_LEN` growth breaches the limit at the CURRENT
/// size, forcing this constant down in lockstep instead of drifting silently.
pub const MAX_ORDERS_PER_AUCTION_CAP: u32 = 90;

const _: () = assert!(
    OrderSlabHeader::LEN + (MAX_ORDERS_PER_AUCTION_CAP as usize) * ORDER_LEN <= 10_240,
    "a max-cap OrderSlab shard must fit one CPI CreateAccount (10_240 bytes)"
);
/// Upper bound on `num_slab_shards` (Stage A sharding). A market has this many
/// `OrderSlab` shards (created one-per-tx by `init_shard`). Bounded so a caller can't
/// force an unbounded number of completeness-aggregate slots; 256 shards × ~90 orders
/// ≈ 23k orders/round is already far past the throughput target.
pub const MAX_SLAB_SHARDS: u16 = 256;

/// Upper bound on `maintenance_margin_bps` (50%). A maintenance margin above half
/// the notional is economically nonsensical for a perp (sub-2× max leverage).
pub const MAX_MAINTENANCE_MARGIN_BPS: u16 = 5_000;
/// Upper bound on `initial_margin_bps` (100% — a position fully collateralized at
/// open). The lower bound is `maintenance_margin_bps` (enforced per-market).
pub const MAX_INITIAL_MARGIN_BPS: u16 = 10_000;
/// Upper bound on `liquidation_penalty_bps` (50%).
pub const MAX_LIQUIDATION_PENALTY_BPS: u16 = 5_000;

/// Instruction data for InitializeMarket.
///
/// # Layout (little-endian)
/// * `market_bump` (u8)
/// * `histogram_bump` (u8)
/// * `order_slab_bump` (u8)
/// * `tick_size` (u64)
/// * `num_ticks` (u32)
/// * `orders_per_auction_cap` (u32)
/// * `oracle_feed_id` ([u8;32])
/// * `maintenance_margin_bps` (u16)
/// * `liquidation_penalty_bps` (u16)
/// * `maker_fee_bps` (i16, signed — negative = rebate)
/// * `taker_fee_bps` (i16, signed — negative = rebate)
/// * `integrator_share_bps` (u16, 0..=10_000)
/// * `crank_fee` (u64)
/// * `collateral_mint` ([u8;32]) — zero for a market with no declared money path
/// * `max_price_move_bps_per_slot` (u16)
/// * `soft_stale_slots` (u64)
/// * `initial_margin_bps` (u16) — initial-margin buffer, must be ≥ `maintenance_margin_bps`
/// * `max_position_notional` (u128) — per-position notional cap (0 = disabled)
/// * `num_slab_shards` (u16) — number of OrderSlab shards (Stage A sharding, ≥ 1). The
///   shards themselves are created one-per-tx by `init_shard`, not here.
/// * `min_order_notional` (u64) — minimum `quantity·price` per order/maker level
///   (anti-dust, missing-features §2.6). 0 = disabled.
/// * `max_open_interest` (u128) — per-side OI soft cap (missing-features §1.2).
///   0 = disabled.
/// * `liquidation_reward_floor` (u64) — flat liquidator reward floor paid from
///   insurance when the equity-capped penalty is smaller (§6.2). 0 = disabled.
/// * `liquidation_close_buffer_bps` (u16) — partial-liquidation health buffer
///   above maintenance (§6.1). 0 = partial liquidation disabled (full close).
///
/// Note: `order_slab_bump` is retained for wire-format stability but is UNUSED — Stage A
/// creates the slab shards via `init_shard`, not in `initialize_market`.
pub struct InitializeMarketData {
    pub market_bump: u8,
    pub histogram_bump: u8,
    pub order_slab_bump: u8,
    pub tick_size: u64,
    pub num_ticks: u32,
    pub orders_per_auction_cap: u32,
    pub oracle_feed_id: [u8; 32],
    pub maintenance_margin_bps: u16,
    pub liquidation_penalty_bps: u16,
    pub maker_fee_bps: i16,
    pub taker_fee_bps: i16,
    pub integrator_share_bps: u16,
    pub crank_fee: u64,
    pub collateral_mint: [u8; 32],
    pub max_price_move_bps_per_slot: u16,
    pub soft_stale_slots: u64,
    pub initial_margin_bps: u16,
    pub max_position_notional: u128,
    pub num_slab_shards: u16,
    pub min_order_notional: u64,
    pub max_open_interest: u128,
    pub liquidation_reward_floor: u64,
    pub liquidation_close_buffer_bps: u16,
}

impl<'a> TryFrom<&'a [u8]> for InitializeMarketData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);

        let market_bump = data[0];
        let histogram_bump = data[1];
        let order_slab_bump = data[2];
        let tick_size = u64::from_le_bytes(data[3..11].try_into().unwrap());
        let num_ticks = u32::from_le_bytes(data[11..15].try_into().unwrap());
        let orders_per_auction_cap = u32::from_le_bytes(data[15..19].try_into().unwrap());
        let oracle_feed_id: [u8; 32] = data[19..51].try_into().unwrap();
        let maintenance_margin_bps = u16::from_le_bytes(data[51..53].try_into().unwrap());
        let liquidation_penalty_bps = u16::from_le_bytes(data[53..55].try_into().unwrap());
        let maker_fee_bps = i16::from_le_bytes(data[55..57].try_into().unwrap());
        let taker_fee_bps = i16::from_le_bytes(data[57..59].try_into().unwrap());
        let integrator_share_bps = u16::from_le_bytes(data[59..61].try_into().unwrap());
        let crank_fee = u64::from_le_bytes(data[61..69].try_into().unwrap());
        let collateral_mint: [u8; 32] = data[69..101].try_into().unwrap();
        let max_price_move_bps_per_slot = u16::from_le_bytes(data[101..103].try_into().unwrap());
        let soft_stale_slots = u64::from_le_bytes(data[103..111].try_into().unwrap());
        let initial_margin_bps = u16::from_le_bytes(data[111..113].try_into().unwrap());
        let max_position_notional = u128::from_le_bytes(data[113..129].try_into().unwrap());
        let num_slab_shards = u16::from_le_bytes(data[129..131].try_into().unwrap());
        let min_order_notional = u64::from_le_bytes(data[131..139].try_into().unwrap());
        let max_open_interest = u128::from_le_bytes(data[139..155].try_into().unwrap());
        let liquidation_reward_floor = u64::from_le_bytes(data[155..163].try_into().unwrap());
        let liquidation_close_buffer_bps = u16::from_le_bytes(data[163..165].try_into().unwrap());

        if tick_size == 0 {
            return Err(TempoProgramError::InvalidPrice.into());
        }
        if num_ticks == 0 || orders_per_auction_cap == 0 {
            return Err(TempoProgramError::InvalidQuantity.into());
        }
        // Upper-bound the account-sizing params: permissionless market
        // creation must not let a caller mint arbitrarily large histogram/slab
        // accounts.
        if num_ticks > MAX_NUM_TICKS || orders_per_auction_cap > MAX_ORDERS_PER_AUCTION_CAP {
            return Err(TempoProgramError::MarketConfigOutOfRange.into());
        }
        // At least one shard, and bounded (Stage A sharding).
        if num_slab_shards == 0 || num_slab_shards > MAX_SLAB_SHARDS {
            return Err(TempoProgramError::MarketConfigOutOfRange.into());
        }
        // Fee config bounds: signed fees within ±10% and the integrator share a
        // valid bps fraction.
        if maker_fee_bps.unsigned_abs() > 1000 || taker_fee_bps.unsigned_abs() > 1000 {
            return Err(TempoProgramError::MarketConfigOutOfRange.into());
        }
        if integrator_share_bps > 10_000 {
            return Err(TempoProgramError::MarketConfigOutOfRange.into());
        }
        // Price-move cap is a bps fraction (0 disables the brake → price jumps to
        // target). The soft-stale window is unbounded by construction.
        if max_price_move_bps_per_slot > 10_000 {
            return Err(TempoProgramError::MarketConfigOutOfRange.into());
        }
        // Risk config (missing-features §1.1/§1.2/§1.3): a market is either a pure
        // clearing benchmark with NO money path (every risk bps zero) or a full perp
        // with sane, ordered bounds. The initial margin is the buffer locked at open;
        // requiring `initial >= maintenance` is what stops a position opening already
        // on its liquidation line. `max_position_notional` is an opaque cap (0 = off).
        // The partial-liquidation buffer is a money-path knob: bounded, and only
        // meaningful when a maintenance margin exists (plan.md §2.1).
        if liquidation_close_buffer_bps > 10_000 {
            return Err(TempoProgramError::MarketConfigOutOfRange.into());
        }
        if maintenance_margin_bps == 0 {
            if initial_margin_bps != 0
                || liquidation_penalty_bps != 0
                || liquidation_close_buffer_bps != 0
            {
                return Err(TempoProgramError::MarketConfigOutOfRange.into());
            }
        } else {
            if maintenance_margin_bps > MAX_MAINTENANCE_MARGIN_BPS {
                return Err(TempoProgramError::MarketConfigOutOfRange.into());
            }
            if initial_margin_bps < maintenance_margin_bps
                || initial_margin_bps > MAX_INITIAL_MARGIN_BPS
            {
                return Err(TempoProgramError::MarketConfigOutOfRange.into());
            }
            if liquidation_penalty_bps > MAX_LIQUIDATION_PENALTY_BPS {
                return Err(TempoProgramError::MarketConfigOutOfRange.into());
            }
        }

        Ok(Self {
            market_bump,
            histogram_bump,
            order_slab_bump,
            tick_size,
            num_ticks,
            orders_per_auction_cap,
            oracle_feed_id,
            maintenance_margin_bps,
            liquidation_penalty_bps,
            maker_fee_bps,
            taker_fee_bps,
            integrator_share_bps,
            crank_fee,
            collateral_mint,
            max_price_move_bps_per_slot,
            soft_stale_slots,
            initial_margin_bps,
            max_position_notional,
            num_slab_shards,
            min_order_notional,
            max_open_interest,
            liquidation_reward_floor,
            liquidation_close_buffer_bps,
        })
    }
}

impl<'a> InstructionData<'a> for InitializeMarketData {
    const LEN: usize = 1
        + 1
        + 1
        + 8
        + 4
        + 4
        + 32
        + 2
        + 2
        + 2
        + 2
        + 2
        + 8
        + 32
        + 2
        + 8
        + 2
        + 16
        + 2
        + 8
        + 16
        + 8
        + 2;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(tick_size: u64, num_ticks: u32, cap: u32) -> [u8; 165] {
        let mut buf = [0u8; 165];
        buf[0] = 250;
        buf[1] = 251;
        buf[2] = 252;
        buf[3..11].copy_from_slice(&tick_size.to_le_bytes());
        buf[11..15].copy_from_slice(&num_ticks.to_le_bytes());
        buf[15..19].copy_from_slice(&cap.to_le_bytes());
        buf[19..51].copy_from_slice(&[9u8; 32]);
        buf[51..53].copy_from_slice(&500u16.to_le_bytes());
        buf[53..55].copy_from_slice(&100u16.to_le_bytes());
        buf[55..57].copy_from_slice(&(-5i16).to_le_bytes());
        buf[57..59].copy_from_slice(&30i16.to_le_bytes());
        buf[59..61].copy_from_slice(&5000u16.to_le_bytes());
        buf[61..69].copy_from_slice(&5u64.to_le_bytes());
        buf[69..101].copy_from_slice(&[8u8; 32]);
        buf[101..103].copy_from_slice(&50u16.to_le_bytes());
        buf[103..111].copy_from_slice(&30u64.to_le_bytes());
        // initial_margin_bps (≥ maintenance 500) + max_position_notional (0 = off).
        buf[111..113].copy_from_slice(&1000u16.to_le_bytes());
        buf[113..129].copy_from_slice(&0u128.to_le_bytes());
        // num_slab_shards (Stage A) — default 16 for the tests.
        buf[129..131].copy_from_slice(&16u16.to_le_bytes());
        // v12 tail: min_order_notional + max_open_interest + reward floor +
        // close buffer (all 0 = disabled except the buffer exercised below).
        buf[131..139].copy_from_slice(&0u64.to_le_bytes());
        buf[139..155].copy_from_slice(&0u128.to_le_bytes());
        buf[155..163].copy_from_slice(&0u64.to_le_bytes());
        buf[163..165].copy_from_slice(&0u16.to_le_bytes());
        buf
    }

    #[test]
    fn test_valid() {
        let buf = encode(10, 64, 90);
        let d = InitializeMarketData::try_from(&buf[..]).unwrap();
        assert_eq!(d.market_bump, 250);
        assert_eq!(d.histogram_bump, 251);
        assert_eq!(d.order_slab_bump, 252);
        assert_eq!(d.tick_size, 10);
        assert_eq!(d.num_ticks, 64);
        assert_eq!(d.orders_per_auction_cap, 90);
        assert_eq!(d.oracle_feed_id, [9u8; 32]);
        assert_eq!(d.maintenance_margin_bps, 500);
        assert_eq!(d.liquidation_penalty_bps, 100);
        assert_eq!(d.maker_fee_bps, -5);
        assert_eq!(d.taker_fee_bps, 30);
        assert_eq!(d.integrator_share_bps, 5000);
        assert_eq!(d.crank_fee, 5);
        assert_eq!(d.collateral_mint, [8u8; 32]);
        assert_eq!(d.initial_margin_bps, 1000);
        assert_eq!(d.max_position_notional, 0);
        assert_eq!(d.num_slab_shards, 16);
        assert_eq!(d.min_order_notional, 0);
        assert_eq!(d.max_open_interest, 0);
        assert_eq!(d.liquidation_reward_floor, 0);
        assert_eq!(d.liquidation_close_buffer_bps, 0);
    }

    #[test]
    fn test_len_is_165() {
        assert_eq!(InitializeMarketData::LEN, 165);
    }

    #[test]
    fn test_v12_fields_parsed() {
        let mut buf = encode(10, 64, 90);
        buf[131..139].copy_from_slice(&5_000u64.to_le_bytes());
        buf[139..155].copy_from_slice(&1_000_000u128.to_le_bytes());
        buf[155..163].copy_from_slice(&77u64.to_le_bytes());
        buf[163..165].copy_from_slice(&250u16.to_le_bytes());
        let d = InitializeMarketData::try_from(&buf[..]).unwrap();
        assert_eq!(d.min_order_notional, 5_000);
        assert_eq!(d.max_open_interest, 1_000_000);
        assert_eq!(d.liquidation_reward_floor, 77);
        assert_eq!(d.liquidation_close_buffer_bps, 250);
    }

    #[test]
    fn test_close_buffer_over_max_rejected() {
        let mut buf = encode(10, 64, 90);
        buf[163..165].copy_from_slice(&10_001u16.to_le_bytes());
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }

    #[test]
    fn test_close_buffer_requires_money_path() {
        // maintenance 0 (no money path) + nonzero close buffer → rejected.
        let mut buf = encode(10, 64, 90);
        buf[51..53].copy_from_slice(&0u16.to_le_bytes()); // maintenance 0
        buf[53..55].copy_from_slice(&0u16.to_le_bytes()); // penalty 0
        buf[111..113].copy_from_slice(&0u16.to_le_bytes()); // initial 0
        buf[163..165].copy_from_slice(&100u16.to_le_bytes()); // buffer ≠ 0
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }

    #[test]
    fn test_zero_shards_rejected() {
        let mut buf = encode(10, 64, 90);
        buf[129..131].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }

    #[test]
    fn test_shards_over_max_rejected() {
        let mut buf = encode(10, 64, 90);
        buf[129..131].copy_from_slice(&(MAX_SLAB_SHARDS + 1).to_le_bytes());
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }

    #[test]
    fn test_initial_below_maintenance_rejected() {
        // maintenance 500 but initial 400 (< maintenance) → rejected.
        let mut buf = encode(10, 64, 90);
        buf[111..113].copy_from_slice(&400u16.to_le_bytes());
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }

    #[test]
    fn test_maintenance_over_max_rejected() {
        let mut buf = encode(10, 64, 90);
        buf[51..53].copy_from_slice(&(MAX_MAINTENANCE_MARGIN_BPS + 1).to_le_bytes());
        // keep initial ≥ maintenance so we isolate the maintenance bound
        buf[111..113].copy_from_slice(&(MAX_MAINTENANCE_MARGIN_BPS + 1).to_le_bytes());
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }

    #[test]
    fn test_no_money_path_requires_all_risk_zero() {
        // maintenance 0 (no money path) but a non-zero initial margin → rejected.
        let mut buf = encode(10, 64, 90);
        buf[51..53].copy_from_slice(&0u16.to_le_bytes()); // maintenance 0
        buf[53..55].copy_from_slice(&0u16.to_le_bytes()); // penalty 0
        buf[111..113].copy_from_slice(&100u16.to_le_bytes()); // initial 100 ≠ 0
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
        // all-zero risk is the valid pure-clearing-benchmark market.
        buf[111..113].copy_from_slice(&0u16.to_le_bytes());
        assert!(InitializeMarketData::try_from(&buf[..]).is_ok());
    }

    #[test]
    fn test_too_short() {
        let buf = [0u8; 5];
        assert!(matches!(
            InitializeMarketData::try_from(&buf[..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }

    #[test]
    fn test_zero_tick_size_rejected() {
        let buf = encode(0, 64, 256);
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::InvalidPrice.into()
        );
    }

    #[test]
    fn test_zero_num_ticks_rejected() {
        let buf = encode(10, 0, 256);
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::InvalidQuantity.into()
        );
    }

    #[test]
    fn test_num_ticks_over_max_rejected() {
        let buf = encode(10, MAX_NUM_TICKS + 1, 256);
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }

    #[test]
    fn test_cap_over_max_rejected() {
        let buf = encode(10, 64, MAX_ORDERS_PER_AUCTION_CAP + 1);
        assert_eq!(
            InitializeMarketData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }

    #[test]
    fn test_max_bounds_accepted() {
        let buf = encode(10, MAX_NUM_TICKS, MAX_ORDERS_PER_AUCTION_CAP);
        assert!(InitializeMarketData::try_from(&buf[..]).is_ok());
    }
}
