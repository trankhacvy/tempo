use const_crypto::ed25519;
use pinocchio::address::Address;

pub const EVENT_AUTHORITY_SEED: &[u8] = b"event_authority";

/// Event authority PDA — derived at compile time from the program id. It owns
/// no account data and exists only to sign CPI event emissions.
pub mod event_authority_pda {
    use super::*;

    const EVENT_AUTHORITY_AND_BUMP: ([u8; 32], u8) =
        ed25519::derive_program_address(&[EVENT_AUTHORITY_SEED], crate::ID.as_array());

    pub const ID: Address = Address::new_from_array(EVENT_AUTHORITY_AND_BUMP.0);
    pub const BUMP: u8 = EVENT_AUTHORITY_AND_BUMP.1;
}
