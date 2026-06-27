use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for InitPosition.
///
/// # Layout
/// * `position_bump` (u8) — bump for the Position PDA
pub struct InitPositionData {
    pub position_bump: u8,
}

impl<'a> TryFrom<&'a [u8]> for InitPositionData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            position_bump: data[0],
        })
    }
}

impl<'a> InstructionData<'a> for InitPositionData {
    const LEN: usize = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid() {
        assert_eq!(
            InitPositionData::try_from(&[253u8][..])
                .unwrap()
                .position_bump,
            253
        );
    }

    #[test]
    fn test_too_short() {
        assert!(matches!(
            InitPositionData::try_from(&[][..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }
}
