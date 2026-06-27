use alloc::vec::Vec;
use pinocchio::error::ProgramError;

use crate::{require_len, validate_discriminator};

/// Discriminator for account types
pub trait Discriminator {
    const DISCRIMINATOR: u8;
}

/// Version marker for account types
pub trait Versioned {
    const VERSION: u8;
}

/// Account size constants
pub trait AccountSize: Discriminator + Versioned + Sized {
    /// Size of the account data (excluding discriminator and version)
    const DATA_LEN: usize;

    /// Total size including discriminator and version
    const LEN: usize = 1 + 1 + Self::DATA_LEN;
}

/// Zero-copy account deserialization
pub trait AccountDeserialize: AccountSize {
    /// Zero-copy read from byte slice (validates discriminator, skips version)
    #[inline(always)]
    fn from_bytes(data: &[u8]) -> Result<&Self, ProgramError> {
        require_len!(data, Self::LEN);
        validate_discriminator!(data, Self::DISCRIMINATOR);
        if data[1] != Self::VERSION {
            return Err(ProgramError::InvalidAccountData);
        }

        // Skip discriminator (byte 0) and version (byte 1)
        unsafe { Self::from_bytes_unchecked(&data[2..]) }
    }

    /// Zero-copy read without discriminator validation
    ///
    /// # Safety
    /// Caller must ensure data is valid, properly sized, and aligned.
    /// Struct must be `#[repr(C)]` with no padding.
    #[inline(always)]
    unsafe fn from_bytes_unchecked(data: &[u8]) -> Result<&Self, ProgramError> {
        if data.len() < Self::DATA_LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(&*(data.as_ptr() as *const Self))
    }

    /// Mutable zero-copy access
    #[inline(always)]
    fn from_bytes_mut(data: &mut [u8]) -> Result<&mut Self, ProgramError> {
        require_len!(data, Self::LEN);
        validate_discriminator!(data, Self::DISCRIMINATOR);
        if data[1] != Self::VERSION {
            return Err(ProgramError::InvalidAccountData);
        }

        // Skip discriminator (byte 0) and version (byte 1)
        unsafe { Self::from_bytes_mut_unchecked(&mut data[2..]) }
    }

    /// Mutable zero-copy access without validation
    ///
    /// # Safety
    /// Caller must ensure data is valid, properly sized, and aligned.
    /// Struct must be `#[repr(C)]` with no padding.
    #[inline(always)]
    unsafe fn from_bytes_mut_unchecked(data: &mut [u8]) -> Result<&mut Self, ProgramError> {
        if data.len() < Self::DATA_LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(&mut *(data.as_mut_ptr() as *mut Self))
    }
}

/// Account discriminator values for this program
#[repr(u8)]
pub enum TempoAccountDiscriminators {
    MarketDiscriminator = 1,
    AuctionHistogramDiscriminator = 2,
    ClearingResultDiscriminator = 3,
    OrderSlabDiscriminator = 4,
    PositionDiscriminator = 5,
    VaultDiscriminator = 6,
    UserCollateralDiscriminator = 7,
    MakerQuoteDiscriminator = 8,
    MarginAccountDiscriminator = 9,
}

/// Manual account deserialization (non-zero-copy)
///
/// Use this for accounts where zero-copy deserialization isn't possible
/// due to alignment constraints.
pub trait AccountParse: AccountSize {
    /// Parse account from bytes (validates discriminator, skips version)
    fn parse_from_bytes(data: &[u8]) -> Result<Self, ProgramError>;
}

/// Account serialization with discriminator and version prefix
pub trait AccountSerialize: Discriminator + Versioned {
    /// Serialize account data without discriminator/version
    fn to_bytes_inner(&self) -> Vec<u8>;

    /// Serialize with discriminator and version prefix
    #[inline(always)]
    fn to_bytes(&self) -> Vec<u8> {
        let inner = self.to_bytes_inner();
        let mut data = Vec::with_capacity(1 + 1 + inner.len());
        data.push(Self::DISCRIMINATOR);
        data.push(Self::VERSION);
        data.extend_from_slice(&inner);
        data
    }

    /// Write directly to a mutable slice
    #[inline(always)]
    fn write_to_slice(&self, dest: &mut [u8]) -> Result<(), ProgramError> {
        let bytes = self.to_bytes();
        if dest.len() < bytes.len() {
            return Err(ProgramError::AccountDataTooSmall);
        }
        dest[..bytes.len()].copy_from_slice(&bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use crate::state::Market;
    use alloc::vec;
    use pinocchio::Address;

    fn test_market() -> Market {
        let authority = Address::new_from_array([2u8; 32]);
        let market_seed = Address::new_from_array([1u8; 32]);
        let oracle = Address::new_from_array([3u8; 32]);
        Market::new(
            100,
            authority,
            market_seed,
            oracle,
            [0u8; 32],
            10,
            64,
            256,
            500,
            100,
            0,
            0,
            0,
            0,
            Address::new_from_array([0u8; 32]),
            0,
            0,
            0,
            0,
        )
    }

    #[test]
    fn test_from_bytes_mut_modifies_original() {
        let market = test_market();
        let mut bytes = market.to_bytes();

        {
            let market_mut = Market::from_bytes_mut(&mut bytes).unwrap();
            market_mut.bump = 200;
        }

        let deserialized = Market::from_bytes(&bytes).unwrap();
        assert_eq!(deserialized.bump, 200);
    }

    #[test]
    fn test_from_bytes_unchecked_skips_discriminator_and_version() {
        let market = test_market();
        let bytes = market.to_bytes();

        // Skip discriminator (byte 0) and version (byte 1)
        let result = unsafe { Market::from_bytes_unchecked(&bytes[2..]) };
        assert!(result.is_ok());

        let deserialized = result.unwrap();
        assert_eq!(deserialized.bump, 100);
    }

    #[test]
    fn test_from_bytes_unchecked_too_short() {
        let data = [0u8; 4];
        let result = unsafe { Market::from_bytes_unchecked(&data) };
        assert_eq!(result, Err(ProgramError::InvalidAccountData));
    }

    #[test]
    fn test_to_bytes_roundtrip() {
        let market = test_market();
        let bytes = market.to_bytes();
        let deserialized = Market::from_bytes(&bytes).unwrap();

        assert_eq!(deserialized.bump, market.bump);
        assert_eq!(deserialized.authority, market.authority);
        assert_eq!(deserialized.tick_size(), market.tick_size());
    }

    #[test]
    fn test_from_bytes_wrong_version() {
        let market = test_market();
        let mut bytes = market.to_bytes();
        bytes[1] = Market::VERSION.wrapping_add(1);

        let result = Market::from_bytes(&bytes);
        assert_eq!(result, Err(ProgramError::InvalidAccountData));
    }

    #[test]
    fn test_write_to_slice_exact_size() {
        let market = test_market();

        let mut dest = vec![0u8; Market::LEN];
        assert!(market.write_to_slice(&mut dest).is_ok());

        let deserialized = Market::from_bytes(&dest).unwrap();
        assert_eq!(deserialized.bump, 100);
    }

    #[test]
    fn test_version_auto_serialized() {
        let market = test_market();
        let bytes = market.to_bytes();

        // Byte 0 = discriminator, Byte 1 = version
        assert_eq!(bytes[0], Market::DISCRIMINATOR);
        assert_eq!(bytes[1], Market::VERSION);
    }

    #[test]
    fn test_account_discriminators_are_non_zero_and_stable() {
        assert_eq!(TempoAccountDiscriminators::MarketDiscriminator as u8, 1);
        assert_eq!(
            TempoAccountDiscriminators::AuctionHistogramDiscriminator as u8,
            2
        );
        assert_eq!(
            TempoAccountDiscriminators::ClearingResultDiscriminator as u8,
            3
        );
        assert_eq!(TempoAccountDiscriminators::OrderSlabDiscriminator as u8, 4);
    }
}
