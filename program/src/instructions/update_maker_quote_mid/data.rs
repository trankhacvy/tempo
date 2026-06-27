use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for UpdateMakerQuoteMid.
///
/// # Layout (little-endian)
/// * `sequence` (u64) — must strictly exceed the stored nonce
/// * `mid_tick` (u32) — new ladder anchor
pub struct UpdateMakerQuoteMidData {
    pub sequence: u64,
    pub mid_tick: u32,
}

impl<'a> TryFrom<&'a [u8]> for UpdateMakerQuoteMidData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            sequence: u64::from_le_bytes(data[0..8].try_into().unwrap()),
            mid_tick: u32::from_le_bytes(data[8..12].try_into().unwrap()),
        })
    }
}

impl<'a> InstructionData<'a> for UpdateMakerQuoteMidData {
    const LEN: usize = 8 + 4;
}
