use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for ClearMakerQuote.
///
/// # Layout (little-endian)
/// * `sequence` (u64) — must strictly exceed the stored nonce
pub struct ClearMakerQuoteData {
    pub sequence: u64,
}

impl<'a> TryFrom<&'a [u8]> for ClearMakerQuoteData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            sequence: u64::from_le_bytes(data[0..8].try_into().unwrap()),
        })
    }
}

impl<'a> InstructionData<'a> for ClearMakerQuoteData {
    const LEN: usize = 8;
}
