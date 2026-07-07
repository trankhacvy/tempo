use pinocchio::error::ProgramError;

use crate::traits::InstructionData;

/// Instruction data for ProposeAuthorityTransfer (plan.md §3.3): two-step —
/// the NEW authority must sign `accept_authority_transfer`, so a typo'd dead
/// address can never take over (no delay needed; the accept signature IS the
/// safety).
///
/// # Layout
/// * `new_authority` ([u8;32])
pub struct ProposeAuthorityTransferData {
    pub new_authority: [u8; 32],
}

impl<'a> TryFrom<&'a [u8]> for ProposeAuthorityTransferData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        if data.len() != Self::LEN {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(Self {
            new_authority: data[0..32].try_into().unwrap(),
        })
    }
}

impl<'a> InstructionData<'a> for ProposeAuthorityTransferData {
    const LEN: usize = 32;
}
