use alloc::vec::Vec;
use codama::CodamaType;
use pinocchio::Address;

use crate::traits::{EventDiscriminator, EventDiscriminators, EventSerialize};

/// Emitted by `set_pause` (missing-features §3.2): the market's pause bitflags
/// changed. `paused` is the NEW flag set (`Market::PAUSE_INTAKE` |
/// `Market::PAUSE_ROLL`); 0 = fully resumed.
#[derive(CodamaType)]
pub struct MarketPauseChangedEvent {
    pub market: Address,
    pub paused: u8,
}

impl EventDiscriminator for MarketPauseChangedEvent {
    const DISCRIMINATOR: u8 = EventDiscriminators::MarketPauseChanged as u8;
}

impl EventSerialize for MarketPauseChangedEvent {
    #[inline(always)]
    fn to_bytes_inner(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(Self::DATA_LEN);
        data.extend_from_slice(self.market.as_ref());
        data.push(self.paused);
        data
    }
}

impl MarketPauseChangedEvent {
    pub const DATA_LEN: usize = 32 + 1;
}
