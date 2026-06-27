//! Pure, no-float, no_std integer math shared by the on-chain program and the
//! off-chain SDK. These are byte-for-byte mirrors of the program's clearing /
//! margin / mark / funding / tick / wide-math functions, with a standalone
//! [`MathError`] in place of the program's `ProgramError`. The program's own
//! unit and conservation fuzzes are copied here verbatim (the golden guard): an
//! SDK preflight check uses the *same* arithmetic the program enforces, so they
//! cannot drift.
#![cfg_attr(not(any(test, feature = "std")), no_std)]

pub mod clearing;
pub mod error;
pub mod funding;
pub mod margin;
pub mod mark;
pub mod oracle;
pub mod tick;
pub mod wide;

pub use error::MathError;
