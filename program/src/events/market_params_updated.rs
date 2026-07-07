use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

/// Emitted when market parameters change (plan.md §3.2): `kind` is
/// `Market::PENDING_NONE` for a hot `update_market_params` (read the market
/// account for the new values) or `PENDING_RISK_PARAMS` for an applied staged
/// risk update (`payload` carries the 8-byte staged risk config).
#[derive(CodamaType)]
pub struct MarketParamsUpdatedEvent {
    pub market: Address,
    pub kind: u8,
    pub payload: [u8; 64],
}

impl EventDiscriminator for MarketParamsUpdatedEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::MarketParamsUpdated as u8;
}

impl EventSerialize for MarketParamsUpdatedEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.push(self.kind);
        data.extend_from_slice(&self.payload);
        data
    }
}

impl MarketParamsUpdatedEvent {
    pub const DATA_LEN: usize = 32 + 1 + 64;
}
