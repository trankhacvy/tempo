/// Validate the length of instruction data.
///
/// # Arguments
/// * `data` - The data to validate.
/// * `len` - The expected length.
///
/// # Returns
/// * `Result<(), ProgramError>` - The result of the operation
#[macro_export]
macro_rules! require_len {
    ($data:expr, $len:expr) => {
        if $data.len() < $len {
            return Err(ProgramError::InvalidInstructionData);
        }
    };
}

/// Validate the length of account data.
///
/// # Arguments
/// * `data` - The account data to validate.
/// * `len` - The expected length.
///
/// # Returns
/// * `Result<(), ProgramError>` - The result of the operation
#[macro_export]
macro_rules! require_account_len {
    ($data:expr, $len:expr) => {
        if $data.len() < $len {
            return Err(ProgramError::InvalidAccountData);
        }
    };
}

/// Validate the discriminator of the account.
///
/// # Arguments
/// * `data` - The account's data to validate.
/// * `discriminator` - The expected discriminator.
///
/// # Returns
/// * `Result<(), ProgramError>` - The result of the operation
#[macro_export]
macro_rules! validate_discriminator {
    ($data:expr, $discriminator:expr) => {
        if $data.is_empty() || $data[0] != $discriminator {
            return Err(ProgramError::InvalidAccountData);
        }
    };
}

/// Compile-time assertion that a struct has no implicit padding.
/// Use this for zero-copy structs to ensure memory layout matches serialized format.
///
/// # Example
/// ```ignore
/// assert_no_padding!(Order, 32 + 1 + 1 + 8 + 8 + 8 + 1 + 8);
/// ```
#[macro_export]
macro_rules! assert_no_padding {
    ($struct:ty, $expected_size:expr) => {
        const _: () = assert!(
            core::mem::size_of::<$struct>() == $expected_size,
            concat!(
                stringify!($struct),
                " struct size mismatch - check for padding"
            )
        );
    };
}

/// Define an instruction struct and implement all boilerplate traits.
///
/// Generates the struct definition, `From`, `TryFrom`, and `Instruction` trait impls.
///
/// # Example
/// ```ignore
/// define_instruction!(SubmitOrder, SubmitOrderAccounts, SubmitOrderData);
/// ```
#[macro_export]
macro_rules! define_instruction {
    ($name:ident, $accounts:ident, $data:ident) => {
        pub struct $name<'a> {
            pub accounts: $accounts<'a>,
            pub data: $data,
        }

        impl<'a> From<($accounts<'a>, $data)> for $name<'a> {
            #[inline(always)]
            fn from((accounts, data): ($accounts<'a>, $data)) -> Self {
                Self { accounts, data }
            }
        }

        impl<'a> TryFrom<(&'a [u8], &'a [pinocchio::account::AccountView])> for $name<'a> {
            type Error = pinocchio::error::ProgramError;

            #[inline(always)]
            fn try_from(
                (data, accounts): (&'a [u8], &'a [pinocchio::account::AccountView]),
            ) -> Result<Self, Self::Error> {
                <Self as $crate::traits::Instruction>::parse(data, accounts)
            }
        }

        impl<'a> $crate::traits::Instruction<'a> for $name<'a> {
            type Accounts = $accounts<'a>;
            type Data = $data;

            #[inline(always)]
            fn accounts(&self) -> &Self::Accounts {
                &self.accounts
            }

            #[inline(always)]
            fn data(&self) -> &Self::Data {
                &self.data
            }
        }
    };
}

/// Generate little-endian byte-array accessors for a zero-copy integer field.
///
/// Zero-copy account structs in this program are `#[repr(C)]` with
/// **alignment 1** — every multi-byte integer is stored as a `[u8; N]`
/// little-endian array rather than a native `u64`/`u32`. This is required
/// because account data is pointer-cast at byte offset 2 (after the 1-byte
/// discriminator + 1-byte version prefix), which is not 8-byte aligned, so a
/// native-aligned field would be an unaligned read (UB). Alignment-1 structs
/// make the cast always valid on every target.
///
/// Generates `fn <name>(&self) -> $ty` and `fn set_<name>(&mut self, v: $ty)`.
#[macro_export]
macro_rules! le_field {
    ($name:ident, $set:ident, $field:ident, u64) => {
        #[inline(always)]
        pub fn $name(&self) -> u64 {
            u64::from_le_bytes(self.$field)
        }
        #[inline(always)]
        pub fn $set(&mut self, v: u64) {
            self.$field = v.to_le_bytes();
        }
    };
    ($name:ident, $set:ident, $field:ident, u32) => {
        #[inline(always)]
        pub fn $name(&self) -> u32 {
            u32::from_le_bytes(self.$field)
        }
        #[inline(always)]
        pub fn $set(&mut self, v: u32) {
            self.$field = v.to_le_bytes();
        }
    };
    ($name:ident, $set:ident, $field:ident, i64) => {
        #[inline(always)]
        pub fn $name(&self) -> i64 {
            i64::from_le_bytes(self.$field)
        }
        #[inline(always)]
        pub fn $set(&mut self, v: i64) {
            self.$field = v.to_le_bytes();
        }
    };
    ($name:ident, $set:ident, $field:ident, i128) => {
        #[inline(always)]
        pub fn $name(&self) -> i128 {
            i128::from_le_bytes(self.$field)
        }
        #[inline(always)]
        pub fn $set(&mut self, v: i128) {
            self.$field = v.to_le_bytes();
        }
    };
    ($name:ident, $set:ident, $field:ident, u128) => {
        #[inline(always)]
        pub fn $name(&self) -> u128 {
            u128::from_le_bytes(self.$field)
        }
        #[inline(always)]
        pub fn $set(&mut self, v: u128) {
            self.$field = v.to_le_bytes();
        }
    };
}

#[cfg(test)]
mod tests {
    use pinocchio::error::ProgramError;

    fn test_require_len(data: &[u8], len: usize) -> Result<(), ProgramError> {
        require_len!(data, len);
        Ok(())
    }

    fn test_validate_discriminator(data: &[u8], discriminator: u8) -> Result<(), ProgramError> {
        validate_discriminator!(data, discriminator);
        Ok(())
    }

    #[test]
    fn test_require_len_success() {
        let data = [1, 2, 3, 4, 5];
        assert!(test_require_len(&data, 5).is_ok());
        assert!(test_require_len(&data, 3).is_ok());
        assert!(test_require_len(&data, 1).is_ok());
    }

    #[test]
    fn test_require_len_too_short() {
        let data = [1, 2, 3];
        let result = test_require_len(&data, 5);
        assert_eq!(result, Err(ProgramError::InvalidInstructionData));
    }

    #[test]
    fn test_validate_discriminator_success() {
        let data = [42, 1, 2, 3];
        assert!(test_validate_discriminator(&data, 42).is_ok());
    }

    #[test]
    fn test_validate_discriminator_mismatch() {
        let data = [42, 1, 2, 3];
        let result = test_validate_discriminator(&data, 99);
        assert_eq!(result, Err(ProgramError::InvalidAccountData));
    }

    #[test]
    fn test_validate_discriminator_empty() {
        let data: [u8; 0] = [];
        let result = test_validate_discriminator(&data, 0);
        assert_eq!(result, Err(ProgramError::InvalidAccountData));
    }
}
