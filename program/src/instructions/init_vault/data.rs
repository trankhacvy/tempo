use pinocchio::error::ProgramError;

use crate::{require_len, traits::InstructionData};

/// Instruction data for InitVault.
///
/// # Layout
/// * `vault_bump` (u8) — bump for the Vault PDA
/// * `authority_bump` (u8) — bump for the vault authority PDA
pub struct InitVaultData {
    pub vault_bump: u8,
    pub authority_bump: u8,
}

impl<'a> TryFrom<&'a [u8]> for InitVaultData {
    type Error = ProgramError;

    #[inline(always)]
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        require_len!(data, Self::LEN);
        Ok(Self {
            vault_bump: data[0],
            authority_bump: data[1],
        })
    }
}

impl<'a> InstructionData<'a> for InitVaultData {
    const LEN: usize = 1 + 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid() {
        let d = InitVaultData::try_from(&[254u8, 253][..]).unwrap();
        assert_eq!(d.vault_bump, 254);
        assert_eq!(d.authority_bump, 253);
    }

    #[test]
    fn test_too_short() {
        assert!(matches!(
            InitVaultData::try_from(&[0u8; 1][..]),
            Err(ProgramError::InvalidInstructionData)
        ));
    }
}
