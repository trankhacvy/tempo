//! # Tempo Program
//!
//! An open-source Dual Flow Batch Auction (DFBA) perpetuals DEX on Solana L1.
//!
//! ## Scope (this crate)
//! The on-chain **clearing engine** only:
//! - an order slab (resting orders for a market),
//! - a price histogram (the "mailboxes") of demand/supply per tick,
//! - the three-phase clearing protocol: ACCUMULATE → DISCOVER → SETTLE.
//!
//! There is **no** collateral movement, **no** SPL token transfers, and
//! **no** margin / funding / liquidation yet. See `docs/system-design.md`
//! (build order) and `docs/tempo-clearing-protocol.md`.
//!
//! ## The clearing breakthrough (see clearing-protocol §2)
//! A uniform-price clearing price is recoverable from cumulative sums alone.
//! The book is represented as a fixed-size histogram over price ticks; folding
//! an order into a bucket is commutative integer addition, so the final price
//! is independent of which crank processes which orders, in which order. That
//! is the property that lets clearing be decomposed into many cheap,
//! permissionless transactions whose cost is O(ticks), not O(orders).
//!
//! ## Architecture
//! Built with Pinocchio (`no_std`), zero-copy `#[repr(C)]` state. Clients are
//! auto-generated via Codama. The repo follows the canonical Pinocchio
//! per-instruction layout.

#![no_std]

extern crate alloc;

use pinocchio::address::declare_id;

pub mod clearing;
pub mod cross_margin;
pub mod errors;
pub mod funding;
pub mod margin;
pub mod mark;
pub mod oracle;
pub mod settle_money;
pub mod traits;
pub mod utils;
pub mod wide_math;

#[cfg(kani)]
pub mod kani_proofs;

pub mod events;
pub mod instructions;
pub mod state;

#[cfg(not(feature = "no-entrypoint"))]
pub mod entrypoint;

// Program id — keypair at `target/deploy/tempo_program-keypair.json`, deployed
// to devnet. Regenerate clients after any change (the event-authority PDA is
// derived from this id at compile time).
declare_id!("8gpzMDNnKNz422jW3hs54TRmZK2H5uEwgfEQbjWAwnJD");

#[cfg(not(feature = "no-entrypoint"))]
use solana_security_txt::security_txt;

#[cfg(not(feature = "no-entrypoint"))]
security_txt! {
    name: "Tempo Program",
    project_url: "https://github.com/tempo-dex/tempo",
    contacts: "link:https://github.com/tempo-dex/tempo/security/advisories/new",
    policy: "https://github.com/tempo-dex/tempo/security/policy",
    source_code: "https://github.com/tempo-dex/tempo"
}
