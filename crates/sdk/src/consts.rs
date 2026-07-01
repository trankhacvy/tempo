//! Protocol constants mirrored from the program. The unit tests assert the
//! mirrored values against the generated discriminator constants where possible.

pub use tempo_math::funding::FUNDING_SCALE;

/// Slots the `Collect` window stays open before accumulation may start
/// (`program/src/state/market.rs::COLLECT_WINDOW_SLOTS`).
pub const COLLECT_WINDOW_SLOTS: u64 = 2;

/// Maximum member positions in one cross-margin group
/// (`program/src/state/margin_account.rs::MAX_CROSS_POSITIONS`).
pub const MAX_CROSS_POSITIONS: usize = 8;

/// Maximum resting orders one trader may hold per auction
/// (`submit_order::MAX_ORDERS_PER_TRADER`).
pub const MAX_ORDERS_PER_TRADER: u32 = 8;

/// Maximum maker-quote ladder levels per side (`state/maker_quote.rs::MAX_LEVELS`).
pub const MAX_MAKER_LEVELS: usize = 8;

/// Default Pyth confidence bound in bps (`oracle.rs::DEFAULT_MAX_CONF_BPS`).
pub const DEFAULT_MAX_CONF_BPS: u16 = 500;

/// Maximum oracle staleness in seconds (`oracle.rs::MAX_AGE_SECS`).
pub const MAX_AGE_SECS: i64 = 120;

/// On-disk byte length of one order slab slot (`state/order.rs::ORDER_LEN`).
/// Stage B (resting orders) grew it 88 → 104 (added `worst_price` + `expires_at_auction`).
pub const ORDER_LEN: usize = 104;
