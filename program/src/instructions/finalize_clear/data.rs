use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for FinalizeClear.
///
/// # Layout
/// * `clearing_bump` (u8) — bump for the ClearingResult PDA
pub struct FinalizeClearData {
    pub clearing_bump: u8,
}

impl<'a> TryFrom<&'a [u8]> for FinalizeClearData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            clearing_bump: data[0],
        })
    }
}

impl<'a> InstructionData<'a> for FinalizeClearData {
    const LEN: usize = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid() {
        let buf = [254u8];
        assert_eq!(
            FinalizeClearData::try_from(&buf[..]).unwrap().clearing_bump,
            254
        );
    }

    #[test]
    fn test_too_short() {
        let buf: [u8; 0] = [];
        assert!(matches!(
            FinalizeClearData::try_from(&buf[..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }
}
