use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for InitMarginAccount.
///
/// # Layout
/// * `margin_bump` (u8) — bump for the MarginAccount PDA
pub struct InitMarginAccountData {
    pub margin_bump: u8,
}

impl<'a> TryFrom<&'a [u8]> for InitMarginAccountData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            margin_bump: data[0],
        })
    }
}

impl<'a> InstructionData<'a> for InitMarginAccountData {
    const LEN: usize = 1;
}
