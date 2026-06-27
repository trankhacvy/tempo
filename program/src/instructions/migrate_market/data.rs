use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for MigrateMarket — the two VERSION-5 risk-config values the
/// admin chooses for the brake/soft-stale guards. Every other newly-appended
/// field (open interest, social-loss indices, effective price) initializes to 0.
///
/// # Layout
/// * `max_price_move_bps_per_slot` (u16) — meltdown-brake cap (0 = disabled)
/// * `soft_stale_slots` (u64) — soft-stale window (0 = disabled)
pub struct MigrateMarketData {
    pub max_price_move_bps_per_slot: u16,
    pub soft_stale_slots: u64,
}

impl<'a> TryFrom<&'a [u8]> for MigrateMarketData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            max_price_move_bps_per_slot: u16::from_le_bytes(data[0..2].try_into().unwrap()),
            soft_stale_slots: u64::from_le_bytes(data[2..10].try_into().unwrap()),
        })
    }
}

impl<'a> InstructionData<'a> for MigrateMarketData {
    const LEN: usize = 10;
}
