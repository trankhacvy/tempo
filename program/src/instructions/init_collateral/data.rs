use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for InitCollateral.
///
/// # Layout
/// * `bump` (u8) — bump for the UserCollateral PDA
pub struct InitCollateralData {
    pub bump: u8,
}

impl<'a> TryFrom<&'a [u8]> for InitCollateralData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self { bump: data[0] })
    }
}

impl<'a> InstructionData<'a> for InitCollateralData {
    const LEN: usize = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid() {
        assert_eq!(
            InitCollateralData::try_from(&[252u8][..]).unwrap().bump,
            252
        );
    }

    #[test]
    fn test_too_short() {
        assert!(matches!(
            InitCollateralData::try_from(&[][..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }
}
