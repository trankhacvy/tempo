use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for InitMakerQuote.
///
/// # Layout (little-endian)
/// * `maker_quote_bump` (u8)
/// * `expiry_slots` (u64) — 0 = never expire
/// * `delegate` ([u8;32]) — all-zero for no delegate
pub struct InitMakerQuoteData {
    pub maker_quote_bump: u8,
    pub expiry_slots: u64,
    pub delegate: [u8; 32],
}

impl<'a> TryFrom<&'a [u8]> for InitMakerQuoteData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            maker_quote_bump: data[0],
            expiry_slots: u64::from_le_bytes(data[1..9].try_into().unwrap()),
            delegate: data[9..41].try_into().unwrap(),
        })
    }
}

impl<'a> InstructionData<'a> for InitMakerQuoteData {
    const LEN: usize = 1 + 8 + 32;
}
