//! Integration-test support for the Tempo clearing-engine program.
//!
//! [`TestContext`] wraps a [`LiteSVM`] instance with the Tempo program loaded,
//! a funded payer, and helpers that build + send real transactions for every
//! instruction. Account state is decoded with minimal hand-rolled little-endian
//! readers that mirror the on-disk byte layouts documented in `program/src`
//! (disc + version prefix at offset 0..2, then the `#[repr(C)]` fields). The
//! readers deliberately do NOT depend on the program crate's internals.
//!
//! Test-harness clippy allowances: the instruction builders take many positional
//! params by design (they mirror the wire encoding), and the `send*` helpers return
//! `Result<_, FailedTransactionMetadata>` whose `Err` variant is large — boxing it in
//! test code adds nothing. These lints are not gated by CI (`just services-check` does
//! not include this crate); the allowances keep `cargo clippy --workspace -D warnings`
//! clean without churning test-only signatures.
#![allow(
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

use std::collections::HashMap;
use std::path::PathBuf;

use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
use solana_sdk::account::Account;
use solana_sdk::clock::Clock;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;

/// Re-export so tests using `use tempo_integration_tests::*` get `.pubkey()`.
pub use solana_sdk::signature::Signer as _Signer;

/// The System program id (`11111111111111111111111111111111`). solana-sdk 4.0
/// dropped the `system_program` module, so we name the constant directly.
const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");

// ---------------------------------------------------------------------------
// Program id + constants (mirrors program/src/lib.rs + state layouts).
// ---------------------------------------------------------------------------

/// The Tempo program id (`declare_id!`), exactly as in `program/src/lib.rs`.
pub const TEMPO_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("8gpzMDNnKNz422jW3hs54TRmZK2H5uEwgfEQbjWAwnJD");

// Instruction discriminators (definition.rs).
const IX_INITIALIZE_MARKET: u8 = 0;
const IX_SUBMIT_ORDER: u8 = 1;
const IX_CANCEL_ORDER: u8 = 2;
const IX_PROCESS_CHUNK: u8 = 3;
const IX_FINALIZE_CLEAR: u8 = 4;
const IX_SETTLE_FILL: u8 = 5;
const IX_START_AUCTION: u8 = 6;
const IX_INIT_POSITION: u8 = 7;
const IX_INIT_VAULT: u8 = 9;
const IX_INIT_COLLATERAL: u8 = 10;
const IX_DEPOSIT: u8 = 11;
const IX_WITHDRAW: u8 = 12;
const IX_UPDATE_FUNDING: u8 = 13;
const IX_LIQUIDATE: u8 = 14;
const IX_FORCE_RESET: u8 = 15;
const IX_INIT_MAKER_QUOTE: u8 = 16;
const IX_UPDATE_MAKER_QUOTE_MID: u8 = 17;
const IX_UPDATE_MAKER_QUOTE_LEVELS: u8 = 18;
const IX_CLEAR_MAKER_QUOTE: u8 = 19;
const IX_PROCESS_MAKER_QUOTE: u8 = 20;
const IX_SETTLE_MAKER_QUOTE: u8 = 21;
const IX_INIT_MARGIN_ACCOUNT: u8 = 22;
const IX_ADD_POSITION_TO_MARGIN: u8 = 23;
const IX_WITHDRAW_CROSS: u8 = 24;
const IX_LIQUIDATE_CROSS: u8 = 25;
const IX_MIGRATE_MARKET: u8 = 26;
const IX_MIGRATE_POSITION: u8 = 27;
const IX_REMOVE_POSITION_FROM_MARGIN: u8 = 28;
const IX_CLOSE_MAKER_QUOTE: u8 = 29;
const IX_INIT_SHARD: u8 = 30;
const IX_RESET_SHARD: u8 = 31;

/// One cross-margin member as supplied to `withdraw_cross` / `liquidate_cross`
/// (known-issues §2.4): a `Live` member is a `(position, market, oracle)` triple;
/// a `Flat` member (size 0) is a bare `position` account that needs no market or
/// oracle, so it costs one account instead of three.
#[derive(Clone, Copy, Debug)]
pub enum CrossLeg {
    /// `(position, market, oracle)` — a live (open) leg, priced off its raw oracle.
    Live(Pubkey, Pubkey, Pubkey),
    /// A flat (size-0) leg: just its position account.
    Flat(Pubkey),
}

/// Push the trailing cross-margin member accounts and return the `live_mask` the
/// program parses (bit `i` set ⇒ member `i` is a live triple). `live_writable`
/// makes each live leg's position+market writable (liquidation writes the target);
/// flat positions are always read-only.
fn push_cross_legs(accounts: &mut Vec<AccountMeta>, legs: &[CrossLeg], live_writable: bool) -> u8 {
    let mut mask = 0u8;
    for (i, leg) in legs.iter().enumerate() {
        match leg {
            CrossLeg::Live(pos, market, oracle) => {
                mask |= 1u8 << i;
                if live_writable {
                    accounts.push(AccountMeta::new(*pos, false));
                    accounts.push(AccountMeta::new(*market, false));
                } else {
                    accounts.push(AccountMeta::new_readonly(*pos, false));
                    accounts.push(AccountMeta::new_readonly(*market, false));
                }
                accounts.push(AccountMeta::new_readonly(*oracle, false));
            }
            CrossLeg::Flat(pos) => {
                accounts.push(AccountMeta::new_readonly(*pos, false));
            }
        }
    }
    mask
}

/// SPL Token program id (`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`), loaded
/// by `LiteSVM::new()` via `with_default_programs()`.
pub const SPL_TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

/// Pyth receiver program id (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`) — the
/// owner the program requires for any `PriceUpdateV2` oracle account.
pub const PYTH_RECEIVER_ID: Pubkey = Pubkey::new_from_array([
    12, 183, 250, 187, 82, 247, 166, 72, 187, 91, 49, 125, 154, 1, 139, 144, 87, 203, 2, 71, 116,
    250, 254, 1, 230, 196, 223, 152, 204, 56, 88, 129,
]);

/// SOL/USD feed id (`0xef0d8b…b56d`) the program expects in the oracle account.
pub const SOL_USD_FEED_ID: [u8; 32] = [
    239, 13, 139, 111, 218, 44, 235, 164, 29, 161, 93, 64, 149, 209, 218, 57, 42, 13, 47, 142, 208,
    198, 199, 188, 15, 76, 250, 200, 194, 128, 181, 109,
];

/// `OrderSlabHeader` slot byte length (order.rs `ORDER_LEN`): 4×u64 + Address +
/// 3×u8 + 5 pad + u64 `cum_before` (known-issues §2.7) + u64 `reserved_margin`
/// (missing-features §1.1).
const ORDER_LEN: usize = 88;

/// Account data prefix: 1 byte discriminator + 1 byte version.
const PREFIX: usize = 2;

// Auction phases (state/market.rs).
pub const PHASE_COLLECT: u8 = 0;
pub const PHASE_ACCUMULATING: u8 = 1;
pub const PHASE_DISCOVERED: u8 = 2;
pub const PHASE_SETTLING: u8 = 3;

// Order sides.
pub const SIDE_BUY: u8 = 0;
pub const SIDE_SELL: u8 = 1;

/// Histogram tick for a tick-aligned `price` (mirrors the program's
/// `price_to_tick_raw`: `tick = price / tick_size - 1`). Used to anchor a maker
/// quote's `mid_tick` at a given price in [`TestContext::post_maker_order`].
pub fn price_to_tick(tick_size: u64, price: u64) -> u32 {
    (price / tick_size - 1) as u32
}

// Order status (order.rs).
pub const STATUS_EMPTY: u8 = 0;
pub const STATUS_RESTING: u8 = 1;
pub const STATUS_ACCUMULATED: u8 = 2;
pub const STATUS_CONSUMED: u8 = 3;

// ---------------------------------------------------------------------------
// PDA bundle.
// ---------------------------------------------------------------------------

/// The set of PDAs (and their bumps) that hang off a single market.
#[derive(Clone, Copy, Debug)]
pub struct MarketPdas {
    pub market_seed: Pubkey,
    pub market: Pubkey,
    pub market_bump: u8,
    pub histogram: Pubkey,
    pub histogram_bump: u8,
    /// Shard 0's `OrderSlab` PDA (Stage A sharding). Kept as `order_slab` for the
    /// single-shard default path; use [`MarketPdas::slab_shard`] for other shards.
    pub order_slab: Pubkey,
    pub order_slab_bump: u8,
    pub clearing: Pubkey,
    pub clearing_bump: u8,
    /// Number of `OrderSlab` shards this market was initialized with (Stage A
    /// sharding). Defaults to 1; `init_market` sets it and creates every shard via
    /// `init_shard`. `start_auction` must `reset_shard` all of them before rolling.
    pub num_slab_shards: u16,
    /// Oracle the market is bound to (set by `init_market*`; `start_auction` must
    /// pass it so the new round's tick window re-snaps onto it — known-issues §2.7).
    /// `Pubkey::default()` until the market is initialized.
    pub oracle: Pubkey,
}

impl MarketPdas {
    /// Derive shard `shard_id`'s `OrderSlab` PDA `[b"order_slab", market, shard_id]`.
    pub fn slab_shard(&self, shard_id: u16) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"order_slab", self.market.as_ref(), &shard_id.to_le_bytes()],
            &TEMPO_PROGRAM_ID,
        )
    }
}

/// Decoded fields of a `MakerQuote` account (Phase 2 harness reader).
pub struct MakerQuoteState {
    pub quote_id: u64,
    pub sequence: u64,
    pub mid_tick: u32,
    pub num_bids: u8,
    pub num_asks: u8,
    pub status: u8,
}

/// Event authority PDA + bump (`[b"event_authority"]`).
pub fn event_authority() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"event_authority"], &TEMPO_PROGRAM_ID)
}

// ---------------------------------------------------------------------------
// Decoded account snapshots.
// ---------------------------------------------------------------------------

/// Decoded `Market` account (state/market.rs layout, after the 2-byte prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketState {
    pub current_auction_id: u64,
    pub tick_size: u64,
    pub last_bid_fill_price: u64,
    pub last_ask_fill_price: u64,
    pub orders_per_auction_cap: u32,
    pub num_ticks: u32,
    pub authority: Pubkey,
    pub market_seed: Pubkey,
    pub phase: u8,
    pub bump: u8,
    pub oi_long: u128,
    pub oi_short: u128,
    pub social_loss_index_long: i128,
    pub social_loss_index_short: i128,
    pub effective_price_1e8: u64,
    pub last_good_oracle_slot: u64,
}

/// Decoded `OrderSlabHeader` (state/order.rs layout, after the 2-byte prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderSlabState {
    pub auction_id: u64,
    pub next_order_id: u64,
    pub capacity: u32,
    pub count: u32,
    pub market: Pubkey,
    pub bump: u8,
}

/// Decoded `AuctionHistogramHeader` (state/histogram.rs layout).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistogramState {
    pub auction_id: u64,
    pub accumulated_count: u64,
    pub num_ticks: u32,
    pub market: Pubkey,
    pub bump: u8,
}

/// Decoded `ClearingResult` (state/clearing_result.rs layout). The full raw
/// bytes are kept too so determinism tests can compare byte-for-byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClearingState {
    pub auction_id: u64,
    pub bid_clearing_price: u64,
    pub ask_clearing_price: u64,
    pub bid_matched_volume: u64,
    pub ask_matched_volume: u64,
    pub bid_volume_allocated_to_marginal_tick: u64,
    pub bid_total_qty_at_marginal_tick: u64,
    pub ask_volume_allocated_to_marginal_tick: u64,
    pub ask_total_qty_at_marginal_tick: u64,
    pub bid_marginal_tick: u32,
    pub ask_marginal_tick: u32,
    pub market: Pubkey,
    pub bump: u8,
    /// Full account data including the 2-byte prefix.
    pub raw: Vec<u8>,
}

/// One decoded order slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderRecord {
    pub price: u64,
    pub quantity: u64,
    pub remaining: u64,
    pub order_id: u64,
    pub trader: Pubkey,
    pub side: u8,
    pub is_maker: u8,
    pub status: u8,
}

// ---------------------------------------------------------------------------
// Little-endian readers.
// ---------------------------------------------------------------------------

fn read_u64(d: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(d[off..off + 8].try_into().unwrap())
}
fn read_u32(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(d[off..off + 4].try_into().unwrap())
}
fn read_pubkey(d: &[u8], off: usize) -> Pubkey {
    let bytes: [u8; 32] = d[off..off + 32].try_into().unwrap();
    Pubkey::new_from_array(bytes)
}
fn read_i64(d: &[u8], off: usize) -> i64 {
    i64::from_le_bytes(d[off..off + 8].try_into().unwrap())
}
fn read_i128(d: &[u8], off: usize) -> i128 {
    i128::from_le_bytes(d[off..off + 16].try_into().unwrap())
}

/// Decoded `Position` account (state/position.rs layout, after the prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionState {
    pub owner: Pubkey,
    pub market: Pubkey,
    pub size: i64,
    pub entry_price: u64,
    pub collateral: u64,
    pub realized_pnl: i128,
    pub last_funding_index: i128,
    pub bump: u8,
}

/// Decoded `Vault` account (state/vault.rs layout, after the 2-byte prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultState {
    pub collateral_mint: Pubkey,
    pub vault_token_account: Pubkey,
    pub insurance_balance: u64,
    pub authority_bump: u8,
    pub bump: u8,
}

/// Decoded `UserCollateral` account (state/user_collateral.rs, after the prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserCollateralState {
    pub owner: Pubkey,
    pub balance: u64,
    pub locked: u64,
    pub bump: u8,
}

impl UserCollateralState {
    /// Free (withdrawable) collateral = balance − locked.
    pub fn free(&self) -> u64 {
        self.balance.saturating_sub(self.locked)
    }
}

// ---------------------------------------------------------------------------
// TestContext.
// ---------------------------------------------------------------------------

/// A drivable Tempo program harness over LiteSVM.
pub struct TestContext {
    pub svm: LiteSVM,
    pub payer: Keypair,
    event_authority: Pubkey,
    /// Risk/fee params written by `init_market_with_oracle` (override before
    /// init to exercise per-market config). `market_maint_bps` defaults to 0 (a
    /// no-margin clearing market, so a non-zero fill settles position-only);
    /// money-path tests set it to a real bps to require + lock margin.
    pub market_maint_bps: u16,
    /// Taker fee in bps written at init (`market_fee_bps`); defaults to 0.
    pub market_fee_bps: u16,
    pub market_crank_fee: u64,
    /// Maker fee in bps, signed, written at init; defaults to 0.
    pub market_maker_fee_bps: i16,
    /// Integrator revenue share in bps written at init; defaults to 0.
    pub market_integrator_share_bps: u16,
    /// Max per-slot effective-price move in bps; defaults to 0 (cap disabled).
    pub market_max_price_move_bps: u16,
    /// Soft-stale oracle window in slots; defaults to 0 (disabled).
    pub market_soft_stale_slots: u64,
    /// Initial-margin bps written at init (missing-features §1.2). `None` mirrors
    /// `market_maint_bps`, preserving the pre-buffer behaviour (initial == maintenance)
    /// so existing money-path margin assertions are unchanged; set to exercise the buffer.
    pub market_initial_margin_bps: Option<u16>,
    /// Per-position notional cap written at init; defaults to 0 (disabled).
    pub market_max_position_notional: u128,
    /// Number of `OrderSlab` shards a market is created with (Stage A sharding);
    /// defaults to 1 (single-shard path). Set before `init_market` for multi-shard tests.
    pub market_num_slab_shards: u16,
    /// Collateral mint written onto the `Market` at init (binds it to a vault);
    /// `None` → a zero mint, so the vault-binding check is skipped (legacy/clearing
    /// markets). Set before `init_market` to exercise the binding.
    pub market_collateral_mint: Option<Pubkey>,
    /// Collateral mint recorded by `init_vault`, used to derive the per-mint vault PDA.
    pub vault_mint: Option<Pubkey>,
    /// Trader keypairs by pubkey, recorded on `submit_order`, so the plain
    /// `settle_fill` helper can lazily create + attach the order owner's Position
    /// (mandatory for a non-zero fill).
    signers: HashMap<Pubkey, Keypair>,
    /// Market authority keypairs by market pubkey, recorded on init, so the
    /// `force_reset` helper can sign as the authority.
    market_authority: HashMap<Pubkey, Keypair>,
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
}

impl TestContext {
    /// Locate `target/deploy/tempo_program.so`, relative to this crate, walking
    /// up to the workspace root (the SBF build emits at the workspace target).
    fn program_so_path() -> PathBuf {
        // CARGO_MANIFEST_DIR = <ws>/tests/integration-tests
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let ws_root = manifest
            .parent() // tests/
            .and_then(|p| p.parent()) // <ws>
            .expect("workspace root")
            .to_path_buf();
        let candidates = [
            ws_root.join("target/deploy/tempo_program.so"),
            ws_root.join("program/target/deploy/tempo_program.so"),
        ];
        for c in candidates {
            if c.exists() {
                return c;
            }
        }
        panic!(
            "tempo_program.so not found; run `cd program && cargo-build-sbf` first \
             (searched <ws>/target/deploy and <ws>/program/target/deploy)"
        );
    }

    /// Construct a fresh context: load the program, fund a payer.
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        let so = std::fs::read(Self::program_so_path()).expect("read tempo_program.so");
        svm.add_program(TEMPO_PROGRAM_ID, &so)
            .expect("load tempo program");

        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 100_000_000_000)
            .expect("airdrop payer");

        let (event_authority, _) = event_authority();
        Self {
            svm,
            payer,
            event_authority,
            market_maint_bps: 0,
            market_fee_bps: 0,
            market_crank_fee: 0,
            market_maker_fee_bps: 0,
            market_integrator_share_bps: 0,
            market_max_price_move_bps: 0,
            market_soft_stale_slots: 0,
            market_initial_margin_bps: None,
            market_max_position_notional: 0,
            market_num_slab_shards: 1,
            market_collateral_mint: None,
            vault_mint: None,
            signers: HashMap::new(),
            market_authority: HashMap::new(),
        }
    }

    /// Create a fresh, funded keypair (a trader / cranker / signer).
    pub fn new_funded_signer(&mut self) -> Keypair {
        let kp = Keypair::new();
        self.svm
            .airdrop(&kp.pubkey(), 10_000_000_000)
            .expect("airdrop signer");
        kp
    }

    /// Derive all PDAs for a market seed.
    pub fn derive_pdas(&self, market_seed: Pubkey) -> MarketPdas {
        let (market, market_bump) =
            Pubkey::find_program_address(&[b"market", market_seed.as_ref()], &TEMPO_PROGRAM_ID);
        let (histogram, histogram_bump) =
            Pubkey::find_program_address(&[b"histogram", market.as_ref()], &TEMPO_PROGRAM_ID);
        // Stage A: the slab PDA gained a `shard_id` seed; `order_slab` is shard 0.
        let (order_slab, order_slab_bump) = Pubkey::find_program_address(
            &[b"order_slab", market.as_ref(), &0u16.to_le_bytes()],
            &TEMPO_PROGRAM_ID,
        );
        let (clearing, clearing_bump) =
            Pubkey::find_program_address(&[b"clearing", market.as_ref()], &TEMPO_PROGRAM_ID);
        MarketPdas {
            market_seed,
            market,
            market_bump,
            histogram,
            histogram_bump,
            order_slab,
            order_slab_bump,
            clearing,
            clearing_bump,
            num_slab_shards: 1,
            oracle: Pubkey::default(),
        }
    }

    /// Send a single-instruction transaction signed by `payer` + extra signers.
    fn send(
        &mut self,
        ix: Instruction,
        signers: &[&Keypair],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let payer_pk = self.payer.pubkey();
        let msg = Message::new(&[ix], Some(&payer_pk));
        // Advance the blockhash so two structurally-identical transactions (e.g.
        // a finalize that is expected to fail followed by one that succeeds) get
        // distinct signatures instead of tripping LiteSVM's AlreadyProcessed dedup.
        self.svm.expire_blockhash();
        let blockhash = self.svm.latest_blockhash();
        // The payer always signs; chain any explicit extra signers after it.
        let mut all: Vec<&Keypair> = Vec::with_capacity(signers.len() + 1);
        all.push(&self.payer);
        for s in signers {
            all.push(s);
        }
        let tx = Transaction::new(&all, msg, blockhash);
        self.svm.send_transaction(tx)
    }

    /// Submit an externally-built instruction (e.g. a `tempo_sdk::ix` builder)
    /// signed by `signers` after the payer. Lets service-crate tests drive the real
    /// SDK instruction builders against the program.
    pub fn send_ix(
        &mut self,
        ix: Instruction,
        signers: &[&Keypair],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        self.send(ix, signers)
    }

    // -- instruction: InitializeMarket --------------------------------------

    /// Initialize a market with a freshly generated seed keypair and a throwaway
    /// (unused-by-clearing) oracle. Returns the derived PDA bundle.
    pub fn init_market(&mut self, tick_size: u64, num_ticks: u32, cap: u32) -> MarketPdas {
        let oracle = Pubkey::new_unique();
        self.init_market_with_oracle(tick_size, num_ticks, cap, oracle)
    }

    /// Initialize a market bound to a specific `oracle` pubkey (funding /
    /// liquidation drive a crafted Pyth account at this address).
    pub fn init_market_with_oracle(
        &mut self,
        tick_size: u64,
        num_ticks: u32,
        cap: u32,
        oracle: Pubkey,
    ) -> MarketPdas {
        let authority = self.new_funded_signer();
        let market_seed = Keypair::new();
        let mut pdas = self.derive_pdas(market_seed.pubkey());
        pdas.oracle = oracle;
        let num_slab_shards = self.market_num_slab_shards.max(1);
        pdas.num_slab_shards = num_slab_shards;

        // Risk config must satisfy the program's init validation (missing-features
        // §1.3): a no-money-path market (maint == 0) carries zero penalty + zero
        // initial margin; a money market carries a penalty and initial ≥ maintenance
        // (default: initial == maintenance, preserving the pre-buffer margin math).
        let (penalty_bps, initial_bps) = if self.market_maint_bps == 0 {
            (0u16, 0u16)
        } else {
            (
                100u16,
                self.market_initial_margin_bps
                    .unwrap_or(self.market_maint_bps),
            )
        };

        let mut data = Vec::with_capacity(1 + 131);
        data.push(IX_INITIALIZE_MARKET);
        data.push(pdas.market_bump);
        data.push(pdas.histogram_bump);
        data.push(pdas.order_slab_bump);
        data.extend_from_slice(&tick_size.to_le_bytes());
        data.extend_from_slice(&num_ticks.to_le_bytes());
        data.extend_from_slice(&cap.to_le_bytes());
        data.extend_from_slice(&SOL_USD_FEED_ID);
        data.extend_from_slice(&self.market_maint_bps.to_le_bytes());
        data.extend_from_slice(&penalty_bps.to_le_bytes());
        data.extend_from_slice(&self.market_maker_fee_bps.to_le_bytes()); // maker_fee_bps
        data.extend_from_slice(&(self.market_fee_bps as i16).to_le_bytes()); // taker_fee_bps
        data.extend_from_slice(&self.market_integrator_share_bps.to_le_bytes()); // integrator_share_bps
        data.extend_from_slice(&self.market_crank_fee.to_le_bytes());
        data.extend_from_slice(self.market_collateral_mint.unwrap_or_default().as_ref());
        data.extend_from_slice(&self.market_max_price_move_bps.to_le_bytes());
        data.extend_from_slice(&self.market_soft_stale_slots.to_le_bytes());
        data.extend_from_slice(&initial_bps.to_le_bytes());
        data.extend_from_slice(&self.market_max_position_notional.to_le_bytes());
        // Stage A sharding: number of OrderSlab shards (created below via init_shard).
        data.extend_from_slice(&num_slab_shards.to_le_bytes());

        // initialize_market no longer creates the slab (shards are made one-per-tx by
        // init_shard), so there is no `order_slab` account here.
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(authority.pubkey(), true),
                AccountMeta::new_readonly(market_seed.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.histogram, false),
                AccountMeta::new_readonly(oracle, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
            ],
            data,
        };

        self.send(ix, &[&authority, &market_seed])
            .expect("init_market");
        self.market_authority
            .insert(pdas.market, authority.insecure_clone());
        // Create every slab shard (Stage A: init_shard is a separate, per-shard ix).
        for shard_id in 0..num_slab_shards {
            self.init_shard(&pdas, shard_id);
        }
        pdas
    }

    // -- instruction: InitShard (Stage A sharding) --------------------------

    /// Create one `OrderSlab` shard `[b"order_slab", market, shard_id]`.
    fn init_shard(&mut self, pdas: &MarketPdas, shard_id: u16) {
        let (slab, bump) = pdas.slab_shard(shard_id);
        let mut data = Vec::with_capacity(1 + 3);
        data.push(IX_INIT_SHARD);
        data.extend_from_slice(&shard_id.to_le_bytes());
        data.push(bump);
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(pdas.market, false),
                AccountMeta::new(slab, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data,
        };
        self.send(ix, &[]).expect("init_shard");
    }

    // -- instruction: ResetShard (Stage A sharding) -------------------------

    /// Zero + re-arm one drained shard for the next round (permissionless).
    fn reset_shard(&mut self, pdas: &MarketPdas, shard_id: u16) {
        let (slab, _) = pdas.slab_shard(shard_id);
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(slab, false),
            ],
            data: vec![IX_RESET_SHARD],
        };
        self.send(ix, &[]).expect("reset_shard");
    }

    // -- instruction: SubmitOrder -------------------------------------------

    /// Whether an account exists (is initialized) in the SVM.
    fn account_exists(&self, key: &Pubkey) -> bool {
        self.svm
            .get_account(key)
            .is_some_and(|a| !a.data.is_empty())
    }

    fn submit_order_ix(
        &self,
        pdas: &MarketPdas,
        trader: &Pubkey,
        side: u8,
        price: u64,
        qty: u64,
        reduce_only: bool,
        shard_id: u16,
    ) -> Instruction {
        // Taker-only (§1.3). Wire = [disc, side, price(8), qty(8), reduce_only(1), shard_id(2)].
        let mut data = Vec::with_capacity(1 + 20);
        data.push(IX_SUBMIT_ORDER);
        data.push(side);
        data.extend_from_slice(&price.to_le_bytes());
        data.extend_from_slice(&qty.to_le_bytes());
        data.push(reduce_only as u8);
        data.extend_from_slice(&shard_id.to_le_bytes());

        let mut accounts = vec![
            AccountMeta::new(*trader, true),
            AccountMeta::new(pdas.market, false),
            AccountMeta::new(pdas.slab_shard(shard_id).0, false),
            AccountMeta::new_readonly(self.event_authority, false),
            AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
        ];
        // Attach the trader's position + collateral ledger when they exist, so a
        // money-path market reserves the order's worst-case margin (missing-features
        // §1.1). A no-money clearing market (trader has no ledger) omits them.
        let position = self.position_pda(pdas, trader).0;
        let collateral = self.collateral_pda(trader).0;
        if self.account_exists(&position) && self.account_exists(&collateral) {
            accounts.push(AccountMeta::new_readonly(position, false));
            accounts.push(AccountMeta::new(collateral, false));
        }
        Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts,
            data,
        }
    }

    /// Submit a resting (taker) order; returns the assigned order id (read from the
    /// slab `next_order_id` before submission, which is the id the program assigns).
    /// Taker-only (§1.3) — for maker liquidity use [`Self::post_maker_order`].
    pub fn submit_order(
        &mut self,
        pdas: &MarketPdas,
        trader: &Keypair,
        side: u8,
        price: u64,
        qty: u64,
    ) -> u64 {
        let order_id = self.order_slab(pdas).next_order_id;
        let ix = self.submit_order_ix(pdas, &trader.pubkey(), side, price, qty, false, 0);
        self.send(ix, &[trader]).expect("submit_order");
        self.signers
            .entry(trader.pubkey())
            .or_insert_with(|| trader.insecure_clone());
        order_id
    }

    /// Submit a reduce-only (taker) order (missing-features §1.1/§2.2): only the
    /// portion that would open new exposure reserves margin. Returns the order id.
    pub fn submit_order_reduce_only(
        &mut self,
        pdas: &MarketPdas,
        trader: &Keypair,
        side: u8,
        price: u64,
        qty: u64,
    ) -> u64 {
        let order_id = self.order_slab(pdas).next_order_id;
        let ix = self.submit_order_ix(pdas, &trader.pubkey(), side, price, qty, true, 0);
        self.send(ix, &[trader]).expect("submit_order_reduce_only");
        self.signers
            .entry(trader.pubkey())
            .or_insert_with(|| trader.insecure_clone());
        order_id
    }

    /// Try submitting a (taker) order, returning the raw result (for negative tests).
    pub fn try_submit_order(
        &mut self,
        pdas: &MarketPdas,
        trader: &Keypair,
        side: u8,
        price: u64,
        qty: u64,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let ix = self.submit_order_ix(pdas, &trader.pubkey(), side, price, qty, false, 0);
        self.send(ix, &[trader])
    }

    /// Submit a taker order into a specific shard (Stage A multi-shard tests). Returns
    /// the order id (read from that shard's `next_order_id`).
    pub fn submit_order_to_shard(
        &mut self,
        pdas: &MarketPdas,
        trader: &Keypair,
        side: u8,
        price: u64,
        qty: u64,
        shard_id: u16,
    ) -> u64 {
        let order_id = self
            .order_slab_shard(pdas, shard_id)
            .expect("shard slab exists")
            .next_order_id;
        let ix = self.submit_order_ix(pdas, &trader.pubkey(), side, price, qty, false, shard_id);
        self.send(ix, &[trader]).expect("submit_order_to_shard");
        self.signers
            .entry(trader.pubkey())
            .or_insert_with(|| trader.insecure_clone());
        order_id
    }

    /// Post a single maker order at `price` via the MakerQuote book (a one-level
    /// ladder at offset 0 around `mid_tick = price_tick`). `submit_order` is
    /// taker-only (§1.3), so maker liquidity must come from the quote book; this
    /// mirrors the old `submit_order(.., is_maker = 1, ..)` ergonomics.
    ///
    /// A maker **buy** rests in `BidDemand`, a **sell** in `AskSupply`. The maker
    /// must already hold a `Position` (+ collateral when the market is
    /// margin-enabled) — settle the resulting fill with
    /// `settle_maker_quote(pdas, &maker.pubkey())`, not `settle_fill`.
    pub fn post_maker_order(
        &mut self,
        pdas: &MarketPdas,
        maker: &Keypair,
        side: u8,
        price: u64,
        qty: u64,
    ) {
        let tick = price_to_tick(self.market(pdas).tick_size, price);
        self.init_maker_quote(pdas, maker, None, 0);
        let (bids, asks): (&[(u16, u64)], &[(u16, u64)]) = if side == SIDE_BUY {
            (&[(0, qty)], &[])
        } else {
            (&[], &[(0, qty)])
        };
        self.update_maker_quote_levels(pdas, maker, 1, tick, bids, asks);
        self.signers
            .entry(maker.pubkey())
            .or_insert_with(|| maker.insecure_clone());
    }

    // -- instruction: CancelOrder -------------------------------------------

    /// Cancel a resting order owned by `trader` (scan path: `slot_hint` sentinel).
    pub fn cancel_order(
        &mut self,
        pdas: &MarketPdas,
        trader: &Keypair,
        order_id: u64,
    ) -> TransactionMetadata {
        self.cancel_order_hinted(pdas, trader, order_id, u32::MAX)
    }

    /// Cancel with an explicit `slot_hint` (known-issues §2.7): the correct slot
    /// exercises the O(1) validated-hint path; a wrong/out-of-range hint exercises
    /// the scan fallback. Either way the result must be identical.
    pub fn cancel_order_hinted(
        &mut self,
        pdas: &MarketPdas,
        trader: &Keypair,
        order_id: u64,
        slot_hint: u32,
    ) -> TransactionMetadata {
        let mut data = Vec::with_capacity(1 + 8 + 4);
        data.push(IX_CANCEL_ORDER);
        data.extend_from_slice(&order_id.to_le_bytes());
        data.extend_from_slice(&slot_hint.to_le_bytes());
        let mut accounts = vec![
            AccountMeta::new_readonly(trader.pubkey(), true),
            AccountMeta::new(pdas.market, false),
            AccountMeta::new(pdas.order_slab, false),
            AccountMeta::new_readonly(self.event_authority, false),
            AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
        ];
        // Attach the collateral ledger when it exists so a reserved order's margin is
        // released on cancel (missing-features §1.1).
        let collateral = self.collateral_pda(&trader.pubkey()).0;
        if self.account_exists(&collateral) {
            accounts.push(AccountMeta::new(collateral, false));
        }
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts,
            data,
        };
        self.send(ix, &[trader]).expect("cancel_order")
    }

    // -- instruction: ProcessChunk ------------------------------------------

    fn process_chunk_ix(
        &self,
        pdas: &MarketPdas,
        cranker: &Pubkey,
        start: u32,
        max: u32,
    ) -> Instruction {
        let mut data = Vec::with_capacity(1 + 8);
        data.push(IX_PROCESS_CHUNK);
        data.extend_from_slice(&start.to_le_bytes());
        data.extend_from_slice(&max.to_le_bytes());
        Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(*cranker, true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.order_slab, false),
                AccountMeta::new(pdas.histogram, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
            ],
            data,
        }
    }

    /// Process a chunk with the payer as the cranker.
    pub fn process_chunk(
        &mut self,
        pdas: &MarketPdas,
        start: u32,
        max: u32,
    ) -> TransactionMetadata {
        self.ensure_collect_window_closed(pdas);
        let payer = self.payer.pubkey();
        let ix = self.process_chunk_ix(pdas, &payer, start, max);
        self.send(ix, &[]).expect("process_chunk")
    }

    /// Fold a chunk of a specific shard (Stage A multi-shard tests).
    pub fn process_chunk_shard(
        &mut self,
        pdas: &MarketPdas,
        shard_id: u16,
        start: u32,
        max: u32,
    ) -> TransactionMetadata {
        self.ensure_collect_window_closed(pdas);
        let ix = self.process_chunk_shard_ix(pdas, shard_id, start, max);
        self.send(ix, &[]).expect("process_chunk_shard")
    }

    /// Try to fold a chunk of a specific shard (raw result, for negative/completeness tests).
    pub fn try_process_chunk_shard(
        &mut self,
        pdas: &MarketPdas,
        shard_id: u16,
        start: u32,
        max: u32,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        self.ensure_collect_window_closed(pdas);
        let ix = self.process_chunk_shard_ix(pdas, shard_id, start, max);
        self.send(ix, &[])
    }

    fn process_chunk_shard_ix(
        &self,
        pdas: &MarketPdas,
        shard_id: u16,
        start: u32,
        max: u32,
    ) -> Instruction {
        let mut data = Vec::with_capacity(1 + 8);
        data.push(IX_PROCESS_CHUNK);
        data.extend_from_slice(&start.to_le_bytes());
        data.extend_from_slice(&max.to_le_bytes());
        Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.slab_shard(shard_id).0, false),
                AccountMeta::new(pdas.histogram, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
            ],
            data,
        }
    }

    /// Process a chunk with an arbitrary cranker signer.
    pub fn process_chunk_by(
        &mut self,
        pdas: &MarketPdas,
        cranker: &Keypair,
        start: u32,
        max: u32,
    ) -> TransactionMetadata {
        self.ensure_collect_window_closed(pdas);
        let ix = self.process_chunk_ix(pdas, &cranker.pubkey(), start, max);
        self.send(ix, &[cranker]).expect("process_chunk_by")
    }

    /// Process a chunk WITHOUT advancing the clock past the collection window
    /// (returns the raw result, for the window-enforcement negative test).
    pub fn try_process_chunk(
        &mut self,
        pdas: &MarketPdas,
        start: u32,
        max: u32,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let payer = self.payer.pubkey();
        let ix = self.process_chunk_ix(pdas, &payer, start, max);
        self.send(ix, &[])
    }

    // -- instruction: FinalizeClear -----------------------------------------

    fn finalize_clear_ix(&self, pdas: &MarketPdas, cranker: &Pubkey) -> Instruction {
        let data = vec![IX_FINALIZE_CLEAR, pdas.clearing_bump];
        Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(*cranker, true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(pdas.histogram, false),
                AccountMeta::new(pdas.clearing, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
            ],
            data,
        }
    }

    /// Finalize clearing with `cranker` as a signer that is paid the crank fee
    /// into its collateral ledger (appends cranker_collateral + vault).
    pub fn finalize_clear_with_fee(
        &mut self,
        pdas: &MarketPdas,
        cranker: &Keypair,
    ) -> TransactionMetadata {
        let (cranker_collateral, _) = self.collateral_pda(&cranker.pubkey());
        let (vault, _) = self.vault_pda();
        let data = vec![IX_FINALIZE_CLEAR, pdas.clearing_bump];
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(cranker.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(pdas.histogram, false),
                AccountMeta::new(pdas.clearing, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
                AccountMeta::new(cranker_collateral, false),
                AccountMeta::new(vault, false),
            ],
            data,
        };
        self.send(ix, &[cranker]).expect("finalize_clear_with_fee")
    }

    /// Finalize with the crank-fee accounts but an explicit (possibly foreign)
    /// vault, for the vault-binding negative test. Returns the raw result.
    pub fn try_finalize_clear_with_fee_vault(
        &mut self,
        pdas: &MarketPdas,
        cranker: &Keypair,
        vault: Pubkey,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (cranker_collateral, _) = self.collateral_pda(&cranker.pubkey());
        let data = vec![IX_FINALIZE_CLEAR, pdas.clearing_bump];
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(cranker.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(pdas.histogram, false),
                AccountMeta::new(pdas.clearing, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
                AccountMeta::new(cranker_collateral, false),
                AccountMeta::new(vault, false),
            ],
            data,
        };
        self.send(ix, &[cranker])
    }

    /// Finalize with explicit crank-fee accounts (cranker ledger + vault), for the
    /// foreign-ledger / foreign-vault negative tests. Returns the raw result.
    pub fn try_finalize_clear_fee_accounts(
        &mut self,
        pdas: &MarketPdas,
        cranker: &Keypair,
        cranker_collateral: Pubkey,
        vault: Pubkey,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let data = vec![IX_FINALIZE_CLEAR, pdas.clearing_bump];
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(cranker.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(pdas.histogram, false),
                AccountMeta::new(pdas.clearing, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
                AccountMeta::new(cranker_collateral, false),
                AccountMeta::new(vault, false),
            ],
            data,
        };
        self.send(ix, &[cranker])
    }

    /// Finalize clearing with the payer as the cranker (panics on failure).
    pub fn finalize_clear(&mut self, pdas: &MarketPdas) -> TransactionMetadata {
        let payer = self.payer.pubkey();
        let ix = self.finalize_clear_ix(pdas, &payer);
        self.send(ix, &[]).expect("finalize_clear")
    }

    /// Try finalizing (for negative / completeness tests).
    pub fn try_finalize_clear(
        &mut self,
        pdas: &MarketPdas,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let payer = self.payer.pubkey();
        let ix = self.finalize_clear_ix(pdas, &payer);
        self.send(ix, &[])
    }

    /// Try finalizing with an attacker-supplied `clearing_bump` in instruction
    /// data (the canonical clearing account is still passed). Used by the
    /// non-canonical-bump regression test: the program derives the bump canonically and must reject
    /// a mismatched one rather than create the result at a non-canonical PDA.
    pub fn try_finalize_clear_with_bump(
        &mut self,
        pdas: &MarketPdas,
        bump: u8,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let mut ix = self.finalize_clear_ix(pdas, &self.payer.pubkey());
        ix.data = vec![IX_FINALIZE_CLEAR, bump];
        self.send(ix, &[])
    }

    // -- instruction: SettleFill --------------------------------------------

    /// Settle one order; returns `(metadata, fill_logged)` where `fill_logged`
    /// is the `fill=` value parsed from the program log.
    ///
    /// A non-zero fill requires the order owner's Position, so
    /// this attaches it — lazily creating it from the keypair recorded at
    /// `submit_order` — keeping clearing-focused tests (no explicit money path)
    /// working unchanged. The collateral/vault accounts stay opt-in (use
    /// `settle_fill_with_margin` to exercise the full money path).
    pub fn settle_fill(&mut self, pdas: &MarketPdas, order_id: u64) -> (TransactionMetadata, u64) {
        let trader = self
            .orders(pdas)
            .into_iter()
            .find(|o| o.order_id == order_id)
            .map(|o| o.trader);
        let owner = trader.and_then(|t| self.signers.get(&t).map(|kp| kp.insecure_clone()));
        let position = owner.map(|kp| self.ensure_position(pdas, &kp));

        let mut data = Vec::with_capacity(1 + 8 + 4);
        data.push(IX_SETTLE_FILL);
        data.extend_from_slice(&order_id.to_le_bytes());
        // slot_hint: u32::MAX forces the scan fallback (known-issues §2.7).
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut accounts = vec![
            AccountMeta::new_readonly(self.payer.pubkey(), true),
            AccountMeta::new(pdas.market, false),
            AccountMeta::new(pdas.order_slab, false),
            AccountMeta::new_readonly(pdas.clearing, false),
            AccountMeta::new_readonly(self.event_authority, false),
            AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
        ];
        if let Some(position) = position {
            accounts.push(AccountMeta::new(position, false));
        }
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts,
            data,
        };
        let meta = self.send(ix, &[]).expect("settle_fill");
        let fill = self.order_fill(pdas, order_id);
        (meta, fill)
    }

    /// Settle one order WITHOUT attaching any optional account (raw 6-account
    /// form). Returns the raw result so negative tests can assert that a non-zero
    /// fill is rejected when the Position is omitted.
    pub fn try_settle_fill_no_position(
        &mut self,
        pdas: &MarketPdas,
        order_id: u64,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let mut data = Vec::with_capacity(1 + 8 + 4);
        data.push(IX_SETTLE_FILL);
        data.extend_from_slice(&order_id.to_le_bytes());
        // slot_hint: u32::MAX forces the scan fallback (known-issues §2.7).
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.order_slab, false),
                AccountMeta::new_readonly(pdas.clearing, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
            ],
            data,
        };
        self.send(ix, &[])
    }

    /// Settle one order with the owner's Position attached but NO collateral
    /// ledger (7-account form). Returns the raw result so the margin-gate test can
    /// assert a non-zero fill is rejected on a margin-enabled market.
    pub fn try_settle_fill_with_position(
        &mut self,
        pdas: &MarketPdas,
        order_id: u64,
        owner: &Keypair,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let position = self.ensure_position(pdas, owner);
        let mut data = Vec::with_capacity(1 + 8 + 4);
        data.push(IX_SETTLE_FILL);
        data.extend_from_slice(&order_id.to_le_bytes());
        // slot_hint: u32::MAX forces the scan fallback (known-issues §2.7).
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.order_slab, false),
                AccountMeta::new_readonly(pdas.clearing, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
                AccountMeta::new(position, false),
            ],
            data,
        };
        self.send(ix, &[])
    }

    /// Ensure `owner` has a Position for this market, creating it if absent.
    pub fn ensure_position(&mut self, pdas: &MarketPdas, owner: &Keypair) -> Pubkey {
        let (position, _) = self.position_pda(pdas, &owner.pubkey());
        let exists = self
            .svm
            .get_account(&position)
            .map(|a| !a.data.is_empty())
            .unwrap_or(false);
        if !exists {
            self.init_position(pdas, owner);
        }
        position
    }

    // -- instruction: InitPosition / Position-aware settle -------------------

    /// Derive a Position PDA for `(market, owner)`.
    pub fn position_pda(&self, pdas: &MarketPdas, owner: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"position", pdas.market.as_ref(), owner.as_ref()],
            &TEMPO_PROGRAM_ID,
        )
    }

    /// Create a Position account for `owner` on this market.
    pub fn init_position(&mut self, pdas: &MarketPdas, owner: &Keypair) -> Pubkey {
        let (position, bump) = self.position_pda(pdas, &owner.pubkey());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(owner.pubkey(), true),
                AccountMeta::new_readonly(pdas.market, false),
                AccountMeta::new(position, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data: vec![IX_INIT_POSITION, bump],
        };
        self.send(ix, &[owner]).expect("init_position");
        position
    }

    /// PDA of `owner`'s cross-margin group.
    pub fn margin_pda(&self, owner: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"margin", owner.as_ref()], &TEMPO_PROGRAM_ID)
    }

    /// Create `owner`'s cross-margin group account.
    pub fn init_margin_account(&mut self, owner: &Keypair) -> Pubkey {
        let (margin, bump) = self.margin_pda(&owner.pubkey());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(owner.pubkey(), true),
                AccountMeta::new(margin, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data: vec![IX_INIT_MARGIN_ACCOUNT, bump],
        };
        self.send(ix, &[owner]).expect("init_margin_account");
        margin
    }

    /// Bind a flat `position` into `owner`'s cross-margin group.
    pub fn add_position_to_margin(
        &mut self,
        pdas: &MarketPdas,
        owner: &Keypair,
        position: &Pubkey,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (margin, _) = self.margin_pda(&owner.pubkey());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(owner.pubkey(), true),
                AccountMeta::new(margin, false),
                AccountMeta::new(*position, false),
                AccountMeta::new_readonly(pdas.market, false),
                AccountMeta::new_readonly(pdas.order_slab, false),
            ],
            data: vec![IX_ADD_POSITION_TO_MARGIN],
        };
        self.send(ix, &[owner])
    }

    /// Unbind a flat member `position` from `owner`'s cross-margin group.
    pub fn remove_position_from_margin(
        &mut self,
        owner: &Keypair,
        position: &Pubkey,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (margin, _) = self.margin_pda(&owner.pubkey());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(owner.pubkey(), true),
                AccountMeta::new(margin, false),
                AccountMeta::new(*position, false),
            ],
            data: vec![IX_REMOVE_POSITION_FROM_MARGIN],
        };
        self.send(ix, &[owner])
    }

    /// Decode a `MarginAccount`'s `(position_count, member position keys)`.
    pub fn margin_account(&self, owner: &Pubkey) -> (u8, Vec<Pubkey>) {
        let (margin, _) = self.margin_pda(owner);
        let d = self.account_data(&margin);
        let b = &d[PREFIX..];
        let count = b[32];
        let members = (0..count as usize)
            .map(|i| read_pubkey(b, 34 + i * 32))
            .collect();
        (count, members)
    }

    // -- instruction: MakerQuote CRUD (Phase 2) -----------------------------

    /// Derive a MakerQuote PDA for `(market, maker)`.
    pub fn maker_quote_pda(&self, pdas: &MarketPdas, maker: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"maker_quote", pdas.market.as_ref(), maker.as_ref()],
            &TEMPO_PROGRAM_ID,
        )
    }

    /// Create a MakerQuote for `maker` (delegate optional; `expiry_slots` 0 = never).
    pub fn init_maker_quote(
        &mut self,
        pdas: &MarketPdas,
        maker: &Keypair,
        delegate: Option<Pubkey>,
        expiry_slots: u64,
    ) -> Pubkey {
        let (quote, bump) = self.maker_quote_pda(pdas, &maker.pubkey());
        let mut data = Vec::with_capacity(1 + 41);
        data.push(IX_INIT_MAKER_QUOTE);
        data.push(bump);
        data.extend_from_slice(&expiry_slots.to_le_bytes());
        data.extend_from_slice(delegate.unwrap_or_default().as_ref());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(quote, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data,
        };
        self.send(ix, &[maker]).expect("init_maker_quote");
        quote
    }

    /// UpdateMakerQuoteMid signed by `writer` (maker or delegate); raw result.
    pub fn try_update_maker_quote_mid(
        &mut self,
        pdas: &MarketPdas,
        maker: &Pubkey,
        writer: &Keypair,
        sequence: u64,
        mid_tick: u32,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (quote, _) = self.maker_quote_pda(pdas, maker);
        let mut data = Vec::with_capacity(1 + 12);
        data.push(IX_UPDATE_MAKER_QUOTE_MID);
        data.extend_from_slice(&sequence.to_le_bytes());
        data.extend_from_slice(&mid_tick.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(writer.pubkey(), true),
                AccountMeta::new_readonly(pdas.market, false),
                AccountMeta::new(quote, false),
            ],
            data,
        };
        self.send(ix, &[writer])
    }

    /// UpdateMakerQuoteMid that must succeed.
    pub fn update_maker_quote_mid(
        &mut self,
        pdas: &MarketPdas,
        maker: &Pubkey,
        writer: &Keypair,
        sequence: u64,
        mid_tick: u32,
    ) -> TransactionMetadata {
        self.try_update_maker_quote_mid(pdas, maker, writer, sequence, mid_tick)
            .expect("update_maker_quote_mid")
    }

    /// UpdateMakerQuoteLevels signed by the maker. `bids`/`asks` are `(offset, size)`.
    pub fn update_maker_quote_levels(
        &mut self,
        pdas: &MarketPdas,
        maker: &Keypair,
        sequence: u64,
        mid_tick: u32,
        bids: &[(u16, u64)],
        asks: &[(u16, u64)],
    ) -> TransactionMetadata {
        let (quote, _) = self.maker_quote_pda(pdas, &maker.pubkey());
        let mut bid_region = [0u8; 80];
        let mut ask_region = [0u8; 80];
        for (i, (o, s)) in bids.iter().enumerate() {
            bid_region[i * 10..i * 10 + 2].copy_from_slice(&o.to_le_bytes());
            bid_region[i * 10 + 2..i * 10 + 10].copy_from_slice(&s.to_le_bytes());
        }
        for (i, (o, s)) in asks.iter().enumerate() {
            ask_region[i * 10..i * 10 + 2].copy_from_slice(&o.to_le_bytes());
            ask_region[i * 10 + 2..i * 10 + 10].copy_from_slice(&s.to_le_bytes());
        }
        let mut data = Vec::with_capacity(1 + 174);
        data.push(IX_UPDATE_MAKER_QUOTE_LEVELS);
        data.extend_from_slice(&sequence.to_le_bytes());
        data.extend_from_slice(&mid_tick.to_le_bytes());
        data.push(bids.len() as u8);
        data.push(asks.len() as u8);
        data.extend_from_slice(&bid_region);
        data.extend_from_slice(&ask_region);
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(maker.pubkey(), true),
                AccountMeta::new_readonly(pdas.market, false),
                AccountMeta::new(quote, false),
            ],
            data,
        };
        self.send(ix, &[maker]).expect("update_maker_quote_levels")
    }

    /// ClearMakerQuote signed by the maker (zero ladder + deactivate); raw result.
    pub fn try_clear_maker_quote(
        &mut self,
        pdas: &MarketPdas,
        maker: &Keypair,
        sequence: u64,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (quote, _) = self.maker_quote_pda(pdas, &maker.pubkey());
        let mut data = Vec::with_capacity(1 + 8);
        data.push(IX_CLEAR_MAKER_QUOTE);
        data.extend_from_slice(&sequence.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(maker.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(quote, false),
            ],
            data,
        };
        self.send(ix, &[maker])
    }

    /// ClearMakerQuote signed by the maker (zero ladder + deactivate).
    pub fn clear_maker_quote(
        &mut self,
        pdas: &MarketPdas,
        maker: &Keypair,
        sequence: u64,
    ) -> TransactionMetadata {
        self.try_clear_maker_quote(pdas, maker, sequence)
            .expect("clear_maker_quote")
    }

    /// CloseMakerQuote signed by the maker (reclaim a cleared quote's rent); raw result.
    pub fn try_close_maker_quote(
        &mut self,
        pdas: &MarketPdas,
        maker: &Keypair,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (quote, _) = self.maker_quote_pda(pdas, &maker.pubkey());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(quote, false),
            ],
            data: vec![IX_CLOSE_MAKER_QUOTE],
        };
        self.send(ix, &[maker])
    }

    /// CloseMakerQuote signed by the maker (reclaim a cleared quote's rent).
    pub fn close_maker_quote(&mut self, pdas: &MarketPdas, maker: &Keypair) -> TransactionMetadata {
        self.try_close_maker_quote(pdas, maker)
            .expect("close_maker_quote")
    }

    /// Decode a MakerQuote account.
    pub fn maker_quote(&self, pdas: &MarketPdas, maker: &Pubkey) -> MakerQuoteState {
        let (quote, _) = self.maker_quote_pda(pdas, maker);
        let d = self.account_data(&quote);
        let b = &d[PREFIX..];
        MakerQuoteState {
            quote_id: read_u64(b, 96),
            sequence: read_u64(b, 104),
            mid_tick: read_u32(b, 112),
            num_bids: b[148],
            num_asks: b[149],
            status: b[150],
        }
    }

    /// Read the market's `active_maker_quote_count` (Market v9, PERF-1 shifted −16).
    pub fn active_maker_quote_count(&self, pdas: &MarketPdas) -> u64 {
        read_u64(&self.account_data(&pdas.market)[PREFIX..], 260)
    }

    /// Read the market's `folded_maker_quote_count` (Market v9, PERF-1 shifted −16).
    pub fn folded_maker_quote_count(&self, pdas: &MarketPdas) -> u64 {
        read_u64(&self.account_data(&pdas.market)[PREFIX..], 268)
    }

    /// Crank: fold one maker quote into the histogram (advances past the window).
    pub fn process_maker_quote(
        &mut self,
        pdas: &MarketPdas,
        maker: &Pubkey,
    ) -> TransactionMetadata {
        self.ensure_collect_window_closed(pdas);
        let (quote, _) = self.maker_quote_pda(pdas, maker);
        let payer = self.payer.pubkey();
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(payer, true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.histogram, false),
                AccountMeta::new(quote, false),
            ],
            data: vec![IX_PROCESS_MAKER_QUOTE],
        };
        self.send(ix, &[]).expect("process_maker_quote")
    }

    /// Crank: settle a maker quote's fills into the maker's position (margin path).
    pub fn settle_maker_quote(&mut self, pdas: &MarketPdas, maker: &Pubkey) -> TransactionMetadata {
        let (quote, _) = self.maker_quote_pda(pdas, maker);
        let (position, _) = self.position_pda(pdas, maker);
        let (user_collateral, _) = self.collateral_pda(maker);
        let (vault, _) = self.vault_pda();
        let payer = self.payer.pubkey();
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(payer, true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(pdas.clearing, false),
                AccountMeta::new_readonly(pdas.order_slab, false),
                AccountMeta::new(quote, false),
                AccountMeta::new(position, false),
                AccountMeta::new(user_collateral, false),
                AccountMeta::new(vault, false),
            ],
            data: vec![IX_SETTLE_MAKER_QUOTE],
        };
        self.send(ix, &[]).expect("settle_maker_quote")
    }

    /// Settle a maker quote on a **clearing-only** (no-margin) market: attaches the
    /// maker's Position (lazily created from the keypair recorded by
    /// `post_maker_order`) but no collateral/vault — the maker-quote analogue of
    /// `settle_fill`. Use on markets with `maintenance_margin_bps == 0`.
    pub fn settle_maker_quote_clearing(
        &mut self,
        pdas: &MarketPdas,
        maker: &Pubkey,
    ) -> TransactionMetadata {
        let kp = self
            .signers
            .get(maker)
            .map(|k| k.insecure_clone())
            .expect("maker keypair recorded by post_maker_order");
        let position = self.ensure_position(pdas, &kp);
        let (quote, _) = self.maker_quote_pda(pdas, maker);
        let payer = self.payer.pubkey();
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(payer, true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(pdas.clearing, false),
                AccountMeta::new_readonly(pdas.order_slab, false),
                AccountMeta::new(quote, false),
                AccountMeta::new(position, false),
            ],
            data: vec![IX_SETTLE_MAKER_QUOTE],
        };
        self.send(ix, &[]).expect("settle_maker_quote_clearing")
    }

    /// Raw bytes of any account (for isolation snapshots).
    pub fn account_raw(&self, key: &Pubkey) -> Vec<u8> {
        self.account_data(key)
    }

    /// Settle one order, applying the fill to `owner`'s Position (appends the
    /// optional position account). Returns `(metadata, fill_logged)`.
    pub fn settle_fill_with_position(
        &mut self,
        pdas: &MarketPdas,
        order_id: u64,
        owner: &Pubkey,
    ) -> (TransactionMetadata, u64) {
        let (position, _) = self.position_pda(pdas, owner);
        let mut data = Vec::with_capacity(1 + 8 + 4);
        data.push(IX_SETTLE_FILL);
        data.extend_from_slice(&order_id.to_le_bytes());
        // slot_hint: u32::MAX forces the scan fallback (known-issues §2.7).
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.order_slab, false),
                AccountMeta::new_readonly(pdas.clearing, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
                AccountMeta::new(position, false),
            ],
            data,
        };
        let meta = self.send(ix, &[]).expect("settle_fill_with_position");
        let fill = self.order_fill(pdas, order_id);
        (meta, fill)
    }

    /// Decode a Position account.
    pub fn position(&self, position: &Pubkey) -> PositionState {
        let d = self.account_data(position);
        let b = &d[PREFIX..];
        PositionState {
            owner: read_pubkey(b, 0),
            market: read_pubkey(b, 32),
            size: read_i64(b, 64),
            entry_price: read_u64(b, 72),
            collateral: read_u64(b, 80),
            realized_pnl: read_i128(b, 88),
            last_funding_index: read_i128(b, 104),
            bump: b[120],
        }
    }

    // -- instruction: StartAuction ------------------------------------------

    fn start_auction_ix(&self, pdas: &MarketPdas) -> Instruction {
        // Stage A: the slab is zeroed per-shard by `reset_shard`, so `start_auction`
        // no longer takes an `order_slab` account (the roll gate is `shards_ready`).
        Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.histogram, false),
                // Oracle for the per-round window recenter (known-issues §2.7). A
                // dummy/non-Pyth oracle just skips the recenter (carry forward).
                AccountMeta::new_readonly(pdas.oracle, false),
            ],
            data: vec![IX_START_AUCTION],
        }
    }

    /// Roll the market into its next round (payer as cranker). Stage A: every drained
    /// shard is `reset_shard`'d first (so `shards_ready == num_slab_shards`), then rolled.
    pub fn start_auction(&mut self, pdas: &MarketPdas) -> TransactionMetadata {
        for shard_id in 0..pdas.num_slab_shards {
            self.reset_shard(pdas, shard_id);
        }
        let ix = self.start_auction_ix(pdas);
        self.send(ix, &[]).expect("start_auction")
    }

    /// Try to roll the market (for negative tests, e.g. a still-populated round).
    pub fn try_start_auction(
        &mut self,
        pdas: &MarketPdas,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let ix = self.start_auction_ix(pdas);
        self.send(ix, &[])
    }

    fn force_reset_ix(&self, pdas: &MarketPdas, authority: &Pubkey) -> Instruction {
        // Stage A: force_reset resets the round atomically, so ALL shards are passed as
        // trailing writable accounts (the processor requires shards.len() == num_slab_shards).
        let mut accounts = vec![
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new(pdas.market, false),
            AccountMeta::new(pdas.histogram, false),
        ];
        for shard_id in 0..pdas.num_slab_shards.max(1) {
            accounts.push(AccountMeta::new(pdas.slab_shard(shard_id).0, false));
        }
        Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts,
            data: vec![IX_FORCE_RESET],
        }
    }

    /// Force-reset a wedged round, signed by the market authority.
    pub fn force_reset(&mut self, pdas: &MarketPdas) -> TransactionMetadata {
        let authority = self
            .market_authority
            .get(&pdas.market)
            .expect("market authority tracked")
            .insecure_clone();
        let ix = self.force_reset_ix(pdas, &authority.pubkey());
        self.send(ix, &[&authority]).expect("force_reset")
    }

    /// Try a force-reset with an arbitrary signer (for the authority negative test).
    pub fn try_force_reset_by(
        &mut self,
        pdas: &MarketPdas,
        signer: &Keypair,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let ix = self.force_reset_ix(pdas, &signer.pubkey());
        self.send(ix, &[signer])
    }

    // -- vault / collateral / funding / liquidation -------------------------

    /// Per-collateral vault PDA + bump (`[b"vault", collateral_mint]`). Uses the
    /// mint recorded by `init_vault`.
    pub fn vault_pda(&self) -> (Pubkey, u8) {
        let mint = self
            .vault_mint
            .expect("init_vault must run before vault_pda");
        Pubkey::find_program_address(&[b"vault", mint.as_ref()], &TEMPO_PROGRAM_ID)
    }

    /// Vault-authority PDA + bump (`[b"vault_authority"]`) — owns the vault token
    /// account and signs withdrawals.
    pub fn vault_authority_pda(&self) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"vault_authority"], &TEMPO_PROGRAM_ID)
    }

    /// UserCollateral PDA + bump (`[b"collateral", owner, collateral_mint]` — CR-3
    /// mint-scoped). Uses the mint recorded by `init_vault`; on a clearing-only
    /// market (no vault) the default mint is used — the ledger is never created or
    /// read there, so the exact address is immaterial, it just must not panic.
    pub fn collateral_pda(&self, owner: &Pubkey) -> (Pubkey, u8) {
        let mint = self.vault_mint.unwrap_or_default();
        Pubkey::find_program_address(
            &[b"collateral", owner.as_ref(), mint.as_ref()],
            &TEMPO_PROGRAM_ID,
        )
    }

    /// Create an SPL mint (0 decimals so amounts are raw base units), mint
    /// authority = payer. Returns the mint pubkey.
    pub fn create_mint(&mut self) -> Pubkey {
        let payer = self.payer.insecure_clone();
        litesvm_token::CreateMint::new(&mut self.svm, &payer)
            .decimals(0)
            .send()
            .expect("create_mint")
    }

    /// Create a plain SPL token account for `mint` owned by `owner` (may be an
    /// off-curve PDA such as the vault authority). Returns the account pubkey.
    pub fn create_token_account(&mut self, mint: &Pubkey, owner: &Pubkey) -> Pubkey {
        let payer = self.payer.insecure_clone();
        litesvm_token::CreateAccount::new(&mut self.svm, &payer, mint)
            .owner(owner)
            .send()
            .expect("create_token_account")
    }

    /// Mint `amount` of `mint` to `dest` (payer is the mint authority).
    pub fn mint_to(&mut self, mint: &Pubkey, dest: &Pubkey, amount: u64) {
        let payer = self.payer.insecure_clone();
        litesvm_token::MintTo::new(&mut self.svm, &payer, mint, dest, amount)
            .send()
            .expect("mint_to");
    }

    /// Read the SPL token-account balance (raw base units).
    pub fn token_balance(&self, account: &Pubkey) -> u64 {
        let acct: litesvm_token::spl_token::state::Account =
            litesvm_token::get_spl_account(&self.svm, account).expect("token account");
        acct.amount
    }

    // -- instruction: InitVault ---------------------------------------------

    /// Create the global `Vault` singleton. The `vault_token_account` must be an
    /// SPL token account owned by the vault-authority PDA. Returns the vault PDA.
    pub fn init_vault(
        &mut self,
        admin: &Keypair,
        collateral_mint: &Pubkey,
        vault_token_account: &Pubkey,
    ) -> Pubkey {
        self.vault_mint = Some(*collateral_mint);
        let (vault, vault_bump) = self.vault_pda();
        let (_authority, authority_bump) = self.vault_authority_pda();

        let data = vec![IX_INIT_VAULT, vault_bump, authority_bump];

        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(*vault_token_account, false),
                AccountMeta::new_readonly(*collateral_mint, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data,
        };
        self.send(ix, &[admin]).expect("init_vault");
        vault
    }

    // -- instruction: InitCollateral ----------------------------------------

    /// Create `owner`'s `UserCollateral` ledger. Returns the ledger PDA.
    pub fn init_collateral(&mut self, owner: &Keypair) -> Pubkey {
        let (user_collateral, bump) = self.collateral_pda(&owner.pubkey());
        let (vault, _) = self.vault_pda();
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(owner.pubkey(), true),
                AccountMeta::new(user_collateral, false),
                AccountMeta::new_readonly(vault, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data: vec![IX_INIT_COLLATERAL, bump],
        };
        self.send(ix, &[owner]).expect("init_collateral");
        user_collateral
    }

    // -- instruction: Deposit -----------------------------------------------

    /// Deposit `amount` from `owner`'s token account into the vault token account.
    pub fn deposit(
        &mut self,
        owner: &Keypair,
        vault_token_account: &Pubkey,
        user_token_account: &Pubkey,
        amount: u64,
    ) -> TransactionMetadata {
        let (user_collateral, _) = self.collateral_pda(&owner.pubkey());
        let (vault, _) = self.vault_pda();
        let mut data = Vec::with_capacity(1 + 8);
        data.push(IX_DEPOSIT);
        data.extend_from_slice(&amount.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(owner.pubkey(), true),
                AccountMeta::new(user_collateral, false),
                AccountMeta::new_readonly(vault, false),
                AccountMeta::new(*vault_token_account, false),
                AccountMeta::new(*user_token_account, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            ],
            data,
        };
        self.send(ix, &[owner]).expect("deposit")
    }

    // -- instruction: Withdraw ----------------------------------------------

    /// Cross-margin withdraw: supplies the group + every member (position,
    /// market, oracle) triple, returning the raw result for assertions.
    pub fn try_withdraw_cross(
        &mut self,
        owner: &Keypair,
        vault_token_account: &Pubkey,
        user_token_account: &Pubkey,
        amount: u64,
        members: &[(Pubkey, Pubkey, Pubkey)],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let legs: Vec<CrossLeg> = members
            .iter()
            .map(|&(p, m, o)| CrossLeg::Live(p, m, o))
            .collect();
        self.try_withdraw_cross_mixed(
            owner,
            vault_token_account,
            user_token_account,
            amount,
            &legs,
        )
    }

    /// WithdrawCross with mixed live/flat members (known-issues §2.4): a `Flat`
    /// member is supplied as a bare position account (no market/oracle).
    pub fn try_withdraw_cross_mixed(
        &mut self,
        owner: &Keypair,
        vault_token_account: &Pubkey,
        user_token_account: &Pubkey,
        amount: u64,
        legs: &[CrossLeg],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (margin, _) = self.margin_pda(&owner.pubkey());
        let (user_collateral, _) = self.collateral_pda(&owner.pubkey());
        let (vault, _) = self.vault_pda();
        let (vault_authority, _) = self.vault_authority_pda();
        let mut accounts = vec![
            AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(margin, false),
            AccountMeta::new(user_collateral, false),
            AccountMeta::new_readonly(vault, false),
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new(*vault_token_account, false),
            AccountMeta::new(*user_token_account, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
        ];
        let live_mask = push_cross_legs(&mut accounts, legs, false);
        let mut data = Vec::with_capacity(1 + 8 + 1);
        data.push(IX_WITHDRAW_CROSS);
        data.extend_from_slice(&amount.to_le_bytes());
        data.push(live_mask);
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts,
            data,
        };
        self.send(ix, &[owner])
    }

    /// WithdrawCross with a caller-supplied `vault` account (to exercise the vault
    /// validation): everything else is derived as usual.
    pub fn try_withdraw_cross_with_vault(
        &mut self,
        owner: &Keypair,
        vault: &Pubkey,
        vault_token_account: &Pubkey,
        user_token_account: &Pubkey,
        amount: u64,
        members: &[(Pubkey, Pubkey, Pubkey)],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (margin, _) = self.margin_pda(&owner.pubkey());
        let (user_collateral, _) = self.collateral_pda(&owner.pubkey());
        let (vault_authority, _) = self.vault_authority_pda();
        let mut accounts = vec![
            AccountMeta::new_readonly(owner.pubkey(), true),
            AccountMeta::new_readonly(margin, false),
            AccountMeta::new(user_collateral, false),
            AccountMeta::new_readonly(*vault, false),
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new(*vault_token_account, false),
            AccountMeta::new(*user_token_account, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
        ];
        let legs: Vec<CrossLeg> = members
            .iter()
            .map(|&(p, m, o)| CrossLeg::Live(p, m, o))
            .collect();
        let live_mask = push_cross_legs(&mut accounts, &legs, false);
        let mut data = Vec::with_capacity(1 + 8 + 1);
        data.push(IX_WITHDRAW_CROSS);
        data.extend_from_slice(&amount.to_le_bytes());
        data.push(live_mask);
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts,
            data,
        };
        self.send(ix, &[owner])
    }

    /// Account-level cross-margin liquidation: closes the first non-flat supplied
    /// member of a combined-unhealthy group. Members are `(position, market,
    /// oracle)` triples (target first).
    pub fn try_liquidate_cross(
        &mut self,
        liquidator: &Keypair,
        owner: &Pubkey,
        members: &[(Pubkey, Pubkey, Pubkey)],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let legs: Vec<CrossLeg> = members
            .iter()
            .map(|&(p, m, o)| CrossLeg::Live(p, m, o))
            .collect();
        self.try_liquidate_cross_mixed(liquidator, owner, &legs)
    }

    /// LiquidateCross with mixed live/flat members (known-issues §2.4): a `Flat`
    /// member is supplied as a bare position account (no market/oracle). The close
    /// target is the first non-flat member.
    pub fn try_liquidate_cross_mixed(
        &mut self,
        liquidator: &Keypair,
        owner: &Pubkey,
        legs: &[CrossLeg],
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (margin, _) = self.margin_pda(owner);
        let (user_collateral, _) = self.collateral_pda(owner);
        let (vault, _) = self.vault_pda();
        let (liq_collateral, _) = self.collateral_pda(&liquidator.pubkey());
        let mut accounts = vec![
            AccountMeta::new_readonly(liquidator.pubkey(), true),
            AccountMeta::new_readonly(margin, false),
            AccountMeta::new(user_collateral, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(liq_collateral, false),
            AccountMeta::new_readonly(self.event_authority, false),
            AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
        ];
        let live_mask = push_cross_legs(&mut accounts, legs, true);
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts,
            data: vec![IX_LIQUIDATE_CROSS, live_mask],
        };
        self.send(ix, &[liquidator])
    }

    fn withdraw_ix(
        &self,
        owner: &Pubkey,
        vault_token_account: &Pubkey,
        user_token_account: &Pubkey,
        amount: u64,
    ) -> Instruction {
        let (user_collateral, _) = self.collateral_pda(owner);
        let (vault, _) = self.vault_pda();
        let (vault_authority, _) = self.vault_authority_pda();
        let mut data = Vec::with_capacity(1 + 8);
        data.push(IX_WITHDRAW);
        data.extend_from_slice(&amount.to_le_bytes());
        Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(*owner, true),
                AccountMeta::new(user_collateral, false),
                AccountMeta::new_readonly(vault, false),
                AccountMeta::new_readonly(vault_authority, false),
                AccountMeta::new(*vault_token_account, false),
                AccountMeta::new(*user_token_account, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            ],
            data,
        }
    }

    /// Withdraw `amount` of free collateral back to `owner`'s token account.
    pub fn withdraw(
        &mut self,
        owner: &Keypair,
        vault_token_account: &Pubkey,
        user_token_account: &Pubkey,
        amount: u64,
    ) -> TransactionMetadata {
        let ix = self.withdraw_ix(
            &owner.pubkey(),
            vault_token_account,
            user_token_account,
            amount,
        );
        self.send(ix, &[owner]).expect("withdraw")
    }

    /// Try to withdraw (for negative tests).
    pub fn try_withdraw(
        &mut self,
        owner: &Keypair,
        vault_token_account: &Pubkey,
        user_token_account: &Pubkey,
        amount: u64,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let ix = self.withdraw_ix(
            &owner.pubkey(),
            vault_token_account,
            user_token_account,
            amount,
        );
        self.send(ix, &[owner])
    }

    // -- instruction: SettleFill with margin (money path) -------------------

    /// Settle one order, applying the fill to `owner`'s Position AND locking
    /// initial margin (appends position + user_collateral + vault). Returns
    /// `(metadata, fill_logged)`.
    pub fn settle_fill_with_margin(
        &mut self,
        pdas: &MarketPdas,
        order_id: u64,
        owner: &Pubkey,
    ) -> (TransactionMetadata, u64) {
        let (position, _) = self.position_pda(pdas, owner);
        let (user_collateral, _) = self.collateral_pda(owner);
        let (vault, _) = self.vault_pda();
        let mut data = Vec::with_capacity(1 + 8 + 4);
        data.push(IX_SETTLE_FILL);
        data.extend_from_slice(&order_id.to_le_bytes());
        // slot_hint: u32::MAX forces the scan fallback (known-issues §2.7).
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.order_slab, false),
                AccountMeta::new_readonly(pdas.clearing, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
                AccountMeta::new(position, false),
                AccountMeta::new(user_collateral, false),
                AccountMeta::new(vault, false),
            ],
            data,
        };
        let meta = self.send(ix, &[]).expect("settle_fill_with_margin");
        let fill = self.order_fill(pdas, order_id);
        (meta, fill)
    }

    /// Fallible `settle_fill_with_margin` (position + collateral + vault) for
    /// negative tests such as the insurance-insolvent gate.
    pub fn try_settle_fill_with_margin(
        &mut self,
        pdas: &MarketPdas,
        order_id: u64,
        owner: &Pubkey,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let (position, _) = self.position_pda(pdas, owner);
        let (user_collateral, _) = self.collateral_pda(owner);
        let (vault, _) = self.vault_pda();
        let mut data = Vec::with_capacity(1 + 8 + 4);
        data.push(IX_SETTLE_FILL);
        data.extend_from_slice(&order_id.to_le_bytes());
        // slot_hint: u32::MAX forces the scan fallback (known-issues §2.7).
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.order_slab, false),
                AccountMeta::new_readonly(pdas.clearing, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
                AccountMeta::new(position, false),
                AccountMeta::new(user_collateral, false),
                AccountMeta::new(vault, false),
            ],
            data,
        };
        self.send(ix, &[])
    }

    /// Like `settle_fill_with_margin` but also passes an `integrator_collateral`
    /// ledger (10th account) so a positive fee's integrator share is paid out.
    pub fn settle_fill_with_integrator(
        &mut self,
        pdas: &MarketPdas,
        order_id: u64,
        owner: &Pubkey,
        integrator: &Pubkey,
    ) -> (TransactionMetadata, u64) {
        let (position, _) = self.position_pda(pdas, owner);
        let (user_collateral, _) = self.collateral_pda(owner);
        let (vault, _) = self.vault_pda();
        let (integrator_collateral, _) = self.collateral_pda(integrator);
        let mut data = Vec::with_capacity(1 + 8 + 4);
        data.push(IX_SETTLE_FILL);
        data.extend_from_slice(&order_id.to_le_bytes());
        // slot_hint: u32::MAX forces the scan fallback (known-issues §2.7).
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(pdas.order_slab, false),
                AccountMeta::new_readonly(pdas.clearing, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
                AccountMeta::new(position, false),
                AccountMeta::new(user_collateral, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(integrator_collateral, false),
            ],
            data,
        };
        let meta = self.send(ix, &[]).expect("settle_fill_with_integrator");
        let fill = self.order_fill(pdas, order_id);
        (meta, fill)
    }

    // -- instruction: UpdateFunding -----------------------------------------

    /// Advance the market's funding index off its bound oracle (payer cranks).
    pub fn update_funding(&mut self, pdas: &MarketPdas, oracle: &Pubkey) -> TransactionMetadata {
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(*oracle, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
            ],
            data: vec![IX_UPDATE_FUNDING],
        };
        self.send(ix, &[]).expect("update_funding")
    }

    /// Try update_funding, returning the raw result (for negative tests, e.g.
    /// the M7 oracle-confidence gate).
    pub fn try_update_funding(
        &mut self,
        pdas: &MarketPdas,
        oracle: &Pubkey,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.payer.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(*oracle, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
            ],
            data: vec![IX_UPDATE_FUNDING],
        };
        self.send(ix, &[])
    }

    // -- instruction: Liquidate ---------------------------------------------

    fn liquidate_ix(
        &self,
        pdas: &MarketPdas,
        oracle: &Pubkey,
        liquidator: &Pubkey,
        owner: &Pubkey,
    ) -> Instruction {
        let (position, _) = self.position_pda(pdas, owner);
        let (user_collateral, _) = self.collateral_pda(owner);
        let (liquidator_collateral, _) = self.collateral_pda(liquidator);
        let (vault, _) = self.vault_pda();
        Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(*liquidator, true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(*oracle, false),
                AccountMeta::new(position, false),
                AccountMeta::new(user_collateral, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(liquidator_collateral, false),
                AccountMeta::new_readonly(self.event_authority, false),
                AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
            ],
            data: vec![IX_LIQUIDATE],
        }
    }

    /// Liquidate `owner`'s position; `liquidator` signs and is paid the penalty.
    pub fn liquidate(
        &mut self,
        pdas: &MarketPdas,
        oracle: &Pubkey,
        liquidator: &Keypair,
        owner: &Pubkey,
    ) -> TransactionMetadata {
        let ix = self.liquidate_ix(pdas, oracle, &liquidator.pubkey(), owner);
        self.send(ix, &[liquidator]).expect("liquidate")
    }

    /// Try to liquidate (for negative tests, e.g. a healthy position).
    pub fn try_liquidate(
        &mut self,
        pdas: &MarketPdas,
        oracle: &Pubkey,
        liquidator: &Keypair,
        owner: &Pubkey,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let ix = self.liquidate_ix(pdas, oracle, &liquidator.pubkey(), owner);
        self.send(ix, &[liquidator])
    }

    /// Set the LiteSVM clock's `unix_timestamp` (everything else preserved).
    /// Useful so funding accrual has a non-zero elapsed window and crafted oracle
    /// `publish_time`s land inside the staleness check.
    pub fn set_clock_ts(&mut self, unix_timestamp: i64) {
        let mut clock = self.svm.get_sysvar::<Clock>();
        clock.unix_timestamp = unix_timestamp;
        self.svm.set_sysvar::<Clock>(&clock);
    }

    /// Current LiteSVM clock `unix_timestamp`.
    pub fn clock_ts(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    /// Set the LiteSVM clock's `slot` (everything else preserved).
    pub fn warp_slot(&mut self, slot: u64) {
        let mut clock = self.svm.get_sysvar::<Clock>();
        clock.slot = slot;
        self.svm.set_sysvar::<Clock>(&clock);
    }

    /// The market's `phase_deadline_slot` (the slot accumulation may start at).
    pub fn phase_deadline_slot(&self, pdas: &MarketPdas) -> u64 {
        let d = self.account_data(&pdas.market);
        read_u64(&d[PREFIX..], 8)
    }

    /// Current LiteSVM clock `slot` (for driving the keeper's `decide`).
    pub fn current_slot(&self) -> u64 {
        self.svm.get_sysvar::<Clock>().slot
    }

    /// Raw account bytes including the disc+version prefix, or `None` if the account
    /// is absent/empty (used by the keeper-driven snapshot decoders).
    pub fn raw_account(&self, key: &Pubkey) -> Option<Vec<u8>> {
        self.svm
            .get_account(key)
            .filter(|a| !a.data.is_empty())
            .map(|a| a.data.to_vec())
    }

    /// Advance the clock to the market's collection-window deadline if not past
    /// it, so a crank may start accumulating (mirrors a real crank waiting).
    pub fn ensure_collect_window_closed(&mut self, pdas: &MarketPdas) {
        let deadline = self.phase_deadline_slot(pdas);
        let slot = self.svm.get_sysvar::<Clock>().slot;
        if slot < deadline {
            self.warp_slot(deadline);
        }
    }

    // -- crafted Pyth oracle account ----------------------------------------

    /// Place a fresh, Pyth-receiver-owned `PriceUpdateV2` (Full verification) at
    /// `oracle` with the given `(price, exponent)` and `publish_time` set to the
    /// LiteSVM clock's current `unix_timestamp` (so it parses as recent).
    pub fn set_oracle(&mut self, oracle: &Pubkey, price: i64, exponent: i32) {
        // conf 0 → perfectly confident; passes the M7 confidence gate for any price.
        self.set_oracle_with_conf(oracle, price, exponent, 0);
    }

    /// Like `set_oracle` but with an explicit confidence interval (raw price
    /// units) — used to exercise the M7 confidence gate.
    pub fn set_oracle_with_conf(&mut self, oracle: &Pubkey, price: i64, exponent: i32, conf: u64) {
        const MIN_LEN: usize = 134;
        const VL_OFFSET: usize = 40;
        let now_ts = self.svm.get_sysvar::<Clock>().unix_timestamp;

        let mut data = vec![0u8; MIN_LEN];
        data[VL_OFFSET] = 1; // Full verification level
        let base = VL_OFFSET + 1; // 41
        data[base..base + 32].copy_from_slice(&SOL_USD_FEED_ID);
        data[base + 32..base + 40].copy_from_slice(&price.to_le_bytes()); // 73..81
        data[base + 40..base + 48].copy_from_slice(&conf.to_le_bytes()); // conf 81..89
        data[base + 48..base + 52].copy_from_slice(&exponent.to_le_bytes()); // 89..93
        data[base + 52..base + 60].copy_from_slice(&now_ts.to_le_bytes()); // publish_time 93..101

        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        let account = Account {
            lamports,
            data,
            owner: PYTH_RECEIVER_ID,
            executable: false,
            rent_epoch: 0,
        };
        self.svm.set_account(*oracle, account).expect("set_oracle");
    }

    // -- migration helpers (layout upgrade tests) ---------------------------

    /// Test-only: rewrite a current `Market` account back to the prior VERSION-4
    /// layout by dropping the 106-byte appended tail (98-byte risk block + the
    /// 8-byte §2.7 `window_floor_price`) and setting the version byte to 4, so the
    /// migration path can be exercised against a realistic old account.
    pub fn downgrade_market_to_v4(&mut self, market: &Pubkey) {
        let mut acct = self.svm.get_account(market).expect("market exists");
        // v4 → current appended region: v5 risk block (98) + v7 window_floor (8) +
        // v8 initial_margin_bps/max_position_notional (18) = 124.
        let new_len = acct.data.len() - 124;
        acct.data.truncate(new_len);
        acct.data[1] = 4;
        self.svm
            .set_account(*market, acct)
            .expect("downgrade market");
    }

    /// Test-only: rewrite a v3 `Position` back to the VERSION-1 layout (drop the
    /// 16-byte `last_social_index` + 1-byte `margin_mode`, set the version byte to 1).
    pub fn downgrade_position_to_v1(&mut self, position: &Pubkey) {
        let mut acct = self.svm.get_account(position).expect("position exists");
        let new_len = acct.data.len() - 17;
        acct.data.truncate(new_len);
        acct.data[1] = 1;
        self.svm
            .set_account(*position, acct)
            .expect("downgrade position");
    }

    /// Test-only: rewrite a v3 `Position` back to the VERSION-2 layout (drop the
    /// 1-byte `margin_mode`, set the version byte to 2).
    pub fn downgrade_position_to_v2(&mut self, position: &Pubkey) {
        let mut acct = self.svm.get_account(position).expect("position exists");
        let new_len = acct.data.len() - 1;
        acct.data.truncate(new_len);
        acct.data[1] = 2;
        self.svm
            .set_account(*position, acct)
            .expect("downgrade position");
    }

    pub fn try_migrate_market(
        &mut self,
        pdas: &MarketPdas,
        max_price_move_bps: u16,
        soft_stale_slots: u64,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let authority = self
            .market_authority
            .get(&pdas.market)
            .expect("authority recorded")
            .insecure_clone();
        let mut data = vec![IX_MIGRATE_MARKET];
        data.extend_from_slice(&max_price_move_bps.to_le_bytes());
        data.extend_from_slice(&soft_stale_slots.to_le_bytes());
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(authority.pubkey(), true),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data,
        };
        self.send(ix, &[&authority])
    }

    pub fn migrate_market(
        &mut self,
        pdas: &MarketPdas,
        max_price_move_bps: u16,
        soft_stale_slots: u64,
    ) -> TransactionMetadata {
        self.try_migrate_market(pdas, max_price_move_bps, soft_stale_slots)
            .expect("migrate_market")
    }

    pub fn try_migrate_position(
        &mut self,
        pdas: &MarketPdas,
        owner: &Keypair,
        position: &Pubkey,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: TEMPO_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(owner.pubkey(), true),
                AccountMeta::new(*position, false),
                AccountMeta::new(pdas.market, false),
                AccountMeta::new_readonly(pdas.order_slab, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data: vec![IX_MIGRATE_POSITION],
        };
        self.send(ix, &[owner])
    }

    pub fn migrate_position(
        &mut self,
        pdas: &MarketPdas,
        owner: &Keypair,
        position: &Pubkey,
    ) -> TransactionMetadata {
        self.try_migrate_position(pdas, owner, position)
            .expect("migrate_position")
    }

    // -- account fetchers ----------------------------------------------------

    fn account_data(&self, key: &Pubkey) -> Vec<u8> {
        self.svm
            .get_account(key)
            .expect("account exists")
            .data
            .to_vec()
    }

    /// Decode the `Market` account.
    pub fn market(&self, pdas: &MarketPdas) -> MarketState {
        let d = self.account_data(&pdas.market);
        let b = &d[PREFIX..];
        // layout (Market v9, PERF-1): 5*u64, 2*u32, 3*Address, phase(u8), bump(u8), …
        // The two order-count mirrors that used to sit at b-offsets 40/48 were
        // removed, so every field after `last_ask_fill_price` is 16 bytes lower.
        MarketState {
            current_auction_id: read_u64(b, 0),
            // phase_deadline_slot at 8 (unused in assertions)
            tick_size: read_u64(b, 16),
            last_bid_fill_price: read_u64(b, 24),
            last_ask_fill_price: read_u64(b, 32),
            orders_per_auction_cap: read_u32(b, 40),
            num_ticks: read_u32(b, 44),
            authority: read_pubkey(b, 48),
            market_seed: read_pubkey(b, 80),
            // oracle at 112
            phase: b[144],
            bump: b[145],
            oi_long: u128::from_le_bytes(b[276..292].try_into().unwrap()),
            oi_short: u128::from_le_bytes(b[292..308].try_into().unwrap()),
            social_loss_index_long: i128::from_le_bytes(b[308..324].try_into().unwrap()),
            social_loss_index_short: i128::from_le_bytes(b[324..340].try_into().unwrap()),
            effective_price_1e8: read_u64(b, 340),
            last_good_oracle_slot: read_u64(b, 348),
        }
    }

    /// Decode the `OrderSlabHeader`.
    pub fn order_slab(&self, pdas: &MarketPdas) -> OrderSlabState {
        let d = self.account_data(&pdas.order_slab);
        let b = &d[PREFIX..];
        // layout: u64 auction_id, u64 next_order_id, u32 capacity, u32 count, Address market, u8 bump
        OrderSlabState {
            auction_id: read_u64(b, 0),
            next_order_id: read_u64(b, 8),
            capacity: read_u32(b, 16),
            count: read_u32(b, 20),
            market: read_pubkey(b, 24),
            bump: b[56],
        }
    }

    /// Decode a specific shard's `OrderSlabHeader` (Stage A), or `None` if the shard
    /// PDA has not been created.
    pub fn order_slab_shard(&self, pdas: &MarketPdas, shard_id: u16) -> Option<OrderSlabState> {
        let slab = pdas.slab_shard(shard_id).0;
        let acct = self.svm.get_account(&slab)?;
        if acct.data.is_empty() {
            return None;
        }
        let b = &acct.data[PREFIX..];
        Some(OrderSlabState {
            auction_id: read_u64(b, 0),
            next_order_id: read_u64(b, 8),
            capacity: read_u32(b, 16),
            count: read_u32(b, 20),
            market: read_pubkey(b, 24),
            bump: b[56],
        })
    }

    /// Decode the `AuctionHistogramHeader`.
    pub fn histogram(&self, pdas: &MarketPdas) -> HistogramState {
        let d = self.account_data(&pdas.histogram);
        let b = &d[PREFIX..];
        // layout: u64 auction_id, u64 accumulated_count, u32 num_ticks, Address market, u8 bump
        HistogramState {
            auction_id: read_u64(b, 0),
            accumulated_count: read_u64(b, 8),
            num_ticks: read_u32(b, 16),
            market: read_pubkey(b, 20),
            bump: b[52],
        }
    }

    /// Decode the `ClearingResult` (or `None` if the PDA does not yet exist).
    pub fn clearing(&self, pdas: &MarketPdas) -> Option<ClearingState> {
        let acct = self.svm.get_account(&pdas.clearing)?;
        if acct.data.is_empty() {
            return None;
        }
        let raw = acct.data.to_vec();
        let b = &raw[PREFIX..];
        // layout: 9*u64, 2*u32, Address market, u8 bump
        Some(ClearingState {
            auction_id: read_u64(b, 0),
            bid_clearing_price: read_u64(b, 8),
            ask_clearing_price: read_u64(b, 16),
            bid_matched_volume: read_u64(b, 24),
            ask_matched_volume: read_u64(b, 32),
            bid_volume_allocated_to_marginal_tick: read_u64(b, 40),
            bid_total_qty_at_marginal_tick: read_u64(b, 48),
            ask_volume_allocated_to_marginal_tick: read_u64(b, 56),
            ask_total_qty_at_marginal_tick: read_u64(b, 64),
            bid_marginal_tick: read_u32(b, 72),
            ask_marginal_tick: read_u32(b, 76),
            market: read_pubkey(b, 80),
            bump: b[112],
            raw,
        })
    }

    /// Decode the `Vault` account.
    pub fn vault(&self) -> VaultState {
        let (vault, _) = self.vault_pda();
        let d = self.account_data(&vault);
        let b = &d[PREFIX..];
        // layout: 2*Address(64), u64 insurance, u8 auth_bump, u8 bump
        VaultState {
            collateral_mint: read_pubkey(b, 0),
            vault_token_account: read_pubkey(b, 32),
            insurance_balance: read_u64(b, 64),
            authority_bump: b[72],
            bump: b[73],
        }
    }

    /// Decode a `UserCollateral` ledger for `owner`.
    pub fn user_collateral(&self, owner: &Pubkey) -> UserCollateralState {
        let (uc, _) = self.collateral_pda(owner);
        let d = self.account_data(&uc);
        let b = &d[PREFIX..];
        // layout (CR-3 mint-scoped): owner Address(32), collateral_mint Address(32),
        // u64 balance, u64 locked, u8 bump
        UserCollateralState {
            owner: read_pubkey(b, 0),
            balance: read_u64(b, 64),
            locked: read_u64(b, 72),
            bump: b[80],
        }
    }

    /// Read the market's bound funding index (Market v9 layout, PERF-1 shifted −16:
    /// funding_index at b-offset 146, 16 bytes; last_funding_ts at 162).
    pub fn market_funding(&self, pdas: &MarketPdas) -> (i128, u64) {
        let d = self.account_data(&pdas.market);
        let b = &d[PREFIX..];
        (read_i128(b, 146), read_u64(b, 162))
    }

    /// Read all non-empty order slots from the slab.
    pub fn orders(&self, pdas: &MarketPdas) -> Vec<OrderRecord> {
        let d = self.account_data(&pdas.order_slab);
        let slab = self.order_slab(pdas);
        // header data len = 75 (Stage A appended shard_id u16 + resting_count u32 +
        // folded_auction_id u64 = 14 bytes to the pre-shard 61).
        let slots_off = PREFIX + 75;
        let mut out = Vec::new();
        for i in 0..slab.capacity as usize {
            let base = slots_off + i * ORDER_LEN;
            let s = &d[base..base + ORDER_LEN];
            let status = s[66];
            if status == STATUS_EMPTY {
                continue;
            }
            out.push(OrderRecord {
                price: read_u64(s, 0),
                quantity: read_u64(s, 8),
                remaining: read_u64(s, 16),
                order_id: read_u64(s, 24),
                trader: read_pubkey(s, 32),
                side: s[64],
                is_maker: s[65],
                status,
            });
        }
        out
    }

    /// Fill applied to `order_id`, read from on-chain state after settle:
    /// `quantity - remaining` (orders start unfilled, so this is the settled
    /// fill). Reads the slab rather than a program log, so it is robust to the
    /// processors no longer logging on the hot path.
    pub fn order_fill(&self, pdas: &MarketPdas, order_id: u64) -> u64 {
        self.orders(pdas)
            .into_iter()
            .find(|o| o.order_id == order_id)
            .map(|o| o.quantity.saturating_sub(o.remaining))
            .unwrap_or(0)
    }
}

/// Count the number of self-CPI program invocations (depth >= 2) for the Tempo
/// program in a transaction's logs. Each `emit_event` self-CPI shows up as
/// `Program <id> invoke [2]`.
pub fn count_self_cpi_invocations(logs: &[String]) -> usize {
    let needle = format!("Program {} invoke [2]", TEMPO_PROGRAM_ID);
    logs.iter().filter(|l| l.contains(&needle)).count()
}
