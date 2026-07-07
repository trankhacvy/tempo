use pinocchio::error::ProgramError;

use crate::{
    errors::TempoProgramError, require_len, state::MAX_QUOTES_PER_MAKER, traits::InstructionData,
};

/// Instruction data for InitMakerQuote.
///
/// # Layout (little-endian)
/// * `maker_quote_bump` (u8)
/// * `expiry_slots` (u64) — 0 = never expire
/// * `delegate` ([u8;32]) — all-zero for no delegate
/// * `quote_index` (u16) — which of the maker's concurrent quotes this is
///   (`[0, MAX_QUOTES_PER_MAKER)`, the 4th PDA seed — known-issues §4.9)
pub struct InitMakerQuoteData {
    pub maker_quote_bump: u8,
    pub expiry_slots: u64,
    pub delegate: [u8; 32],
    pub quote_index: u16,
}

impl<'a> TryFrom<&'a [u8]> for InitMakerQuoteData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        let quote_index = u16::from_le_bytes(data[41..43].try_into().unwrap());
        if quote_index >= MAX_QUOTES_PER_MAKER {
            return Err(TempoProgramError::MarketConfigOutOfRange.into());
        }
        Ok(Self {
            maker_quote_bump: data[0],
            expiry_slots: u64::from_le_bytes(data[1..9].try_into().unwrap()),
            delegate: data[9..41].try_into().unwrap(),
            quote_index,
        })
    }
}

impl<'a> InstructionData<'a> for InitMakerQuoteData {
    const LEN: usize = 1 + 8 + 32 + 2;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_index_parsed_and_bounded() {
        let mut buf = [0u8; 43];
        buf[41..43].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(
            InitMakerQuoteData::try_from(&buf[..]).unwrap().quote_index,
            1
        );
        buf[41..43].copy_from_slice(&MAX_QUOTES_PER_MAKER.to_le_bytes());
        assert_eq!(
            InitMakerQuoteData::try_from(&buf[..]).err().unwrap(),
            TempoProgramError::MarketConfigOutOfRange.into()
        );
    }
}
