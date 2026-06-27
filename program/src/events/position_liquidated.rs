use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

/// Emitted by `liquidate` when a position is closed below maintenance margin.
#[derive(CodamaType)]
pub struct PositionLiquidatedEvent {
    pub market: Address,
    pub owner: Address,
    pub mark: u64,
    pub equity: i128,
    pub penalty: u64,
    pub bad_debt: u64,
}

impl EventDiscriminator for PositionLiquidatedEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::PositionLiquidated as u8;
}

impl EventSerialize for PositionLiquidatedEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.extend_from_slice(self.owner.as_ref());
        data.extend_from_slice(&self.mark.to_le_bytes());
        data.extend_from_slice(&self.equity.to_le_bytes());
        data.extend_from_slice(&self.penalty.to_le_bytes());
        data.extend_from_slice(&self.bad_debt.to_le_bytes());
        data
    }
}

impl PositionLiquidatedEvent {
    pub const DATA_LEN: usize = 32 + 32 + 8 + 16 + 8 + 8;
}
