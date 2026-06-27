use pinocchio::error::ProgramError;

use crate::{
    errors::TempoProgramError,
    require_len,
    state::{LEVELS_LEN, MAX_LEVELS},
    traits::InstructionData,
};

/// Instruction data for UpdateMakerQuoteLevels.
///
/// # Layout (little-endian)
/// * `sequence` (u64)
/// * `mid_tick` (u32)
/// * `num_bids` (u8), `num_asks` (u8)
/// * `bid_levels` ([u8; LEVELS_LEN]) — `MAX_LEVELS` × (u16 offset, u64 size), padded
/// * `ask_levels` ([u8; LEVELS_LEN])
pub struct UpdateMakerQuoteLevelsData {
    pub sequence: u64,
    pub mid_tick: u32,
    pub num_bids: u8,
    pub num_asks: u8,
    pub bid_levels: [u8; LEVELS_LEN],
    pub ask_levels: [u8; LEVELS_LEN],
}

impl<'a> TryFrom<&'a [u8]> for UpdateMakerQuoteLevelsData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        let num_bids = data[12];
        let num_asks = data[13];
        if num_bids as usize > MAX_LEVELS || num_asks as usize > MAX_LEVELS {
            return Err(TempoProgramError::MarketConfigOutOfRange.into());
        }
        Ok(Self {
            sequence: u64::from_le_bytes(data[0..8].try_into().unwrap()),
            mid_tick: u32::from_le_bytes(data[8..12].try_into().unwrap()),
            num_bids,
            num_asks,
            bid_levels: data[14..14 + LEVELS_LEN].try_into().unwrap(),
            ask_levels: data[14 + LEVELS_LEN..14 + 2 * LEVELS_LEN]
                .try_into()
                .unwrap(),
        })
    }
}

impl<'a> InstructionData<'a> for UpdateMakerQuoteLevelsData {
    const LEN: usize = 8 + 4 + 1 + 1 + LEVELS_LEN * 2;
}
