use pinocchio::error::ProgramError;

use crate::{
    instructions::initialize_market::{validate_brake_config, validate_fee_config},
    traits::InstructionData,
};

/// Instruction data for UpdateMarketParams — the HOT parameter set (plan.md
/// §3.2): every field here is read at use-time from `Market`, so a change
/// simply applies to the next operation and can never strand in-flight state.
/// Risk-class params (margins/penalty/buffer) go through the STAGED
/// propose/apply path instead; structural params (tick size, ticks, shards,
/// cap, mint) are never changeable.
///
/// # Layout (little-endian, 72 bytes)
/// * `maker_fee_bps` (i16) · `taker_fee_bps` (i16) · `integrator_share_bps` (u16)
/// * `crank_fee` (u64) · `max_price_move_bps_per_slot` (u16) · `soft_stale_slots` (u64)
/// * `max_position_notional` (u128) · `min_order_notional` (u64)
/// * `max_open_interest` (u128) · `liquidation_reward_floor` (u64)
pub struct UpdateMarketParamsData {
    pub maker_fee_bps: i16,
    pub taker_fee_bps: i16,
    pub integrator_share_bps: u16,
    pub crank_fee: u64,
    pub max_price_move_bps_per_slot: u16,
    pub soft_stale_slots: u64,
    pub max_position_notional: u128,
    pub min_order_notional: u64,
    pub max_open_interest: u128,
    pub liquidation_reward_floor: u64,
}

impl<'a> TryFrom<&'a [u8]> for UpdateMarketParamsData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        if data.len() != Self::LEN {
            return Err(ProgramError::InvalidInstructionData);
        }
        let maker_fee_bps = i16::from_le_bytes(data[0..2].try_into().unwrap());
        let taker_fee_bps = i16::from_le_bytes(data[2..4].try_into().unwrap());
        let integrator_share_bps = u16::from_le_bytes(data[4..6].try_into().unwrap());
        let crank_fee = u64::from_le_bytes(data[6..14].try_into().unwrap());
        let max_price_move_bps_per_slot = u16::from_le_bytes(data[14..16].try_into().unwrap());
        let soft_stale_slots = u64::from_le_bytes(data[16..24].try_into().unwrap());
        let max_position_notional = u128::from_le_bytes(data[24..40].try_into().unwrap());
        let min_order_notional = u64::from_le_bytes(data[40..48].try_into().unwrap());
        let max_open_interest = u128::from_le_bytes(data[48..64].try_into().unwrap());
        let liquidation_reward_floor = u64::from_le_bytes(data[64..72].try_into().unwrap());

        // ONE source of truth with initialize_market (plan.md §3.2).
        validate_fee_config(maker_fee_bps, taker_fee_bps, integrator_share_bps)?;
        validate_brake_config(max_price_move_bps_per_slot)?;

        Ok(Self {
            maker_fee_bps,
            taker_fee_bps,
            integrator_share_bps,
            crank_fee,
            max_price_move_bps_per_slot,
            soft_stale_slots,
            max_position_notional,
            min_order_notional,
            max_open_interest,
            liquidation_reward_floor,
        })
    }
}

impl<'a> InstructionData<'a> for UpdateMarketParamsData {
    const LEN: usize = 2 + 2 + 2 + 8 + 2 + 8 + 16 + 8 + 16 + 8;
}
