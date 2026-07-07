//! Instruction builders. The full set of low-level codama builders is re-exported
//! from the generated module; the wrappers below fill the PDAs and assemble the
//! non-trivial cross-margin account list with its `live_mask`.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::ids::TEMPO_PROGRAM_ID;
use crate::pda::{event_authority, MarketPdas};

pub use crate::generated::instructions::*;

/// System program id (`11111111111111111111111111111111`). solana-sdk dropped the
/// `system_program` module in newer releases, so name it directly.
const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");

/// One cross-margin member as supplied to `withdraw_cross` / `liquidate_cross`
/// (known-issues §2.4): a `Live` member is a `(position, market, oracle)` triple
/// priced off its raw oracle; a `Flat` (size-0) member is a bare `position`
/// account, costing one account instead of three.
#[derive(Clone, Copy, Debug)]
pub enum CrossLeg {
    Live {
        position: Pubkey,
        market: Pubkey,
        oracle: Pubkey,
    },
    Flat {
        position: Pubkey,
    },
}

/// Push the trailing cross-margin member accounts and return the `live_mask` the
/// program parses (bit `i` set ⇒ member `i` is a live triple). `live_writable`
/// makes each live leg's position+market writable (liquidation writes the target);
/// flat positions are always read-only.
pub fn push_cross_legs(metas: &mut Vec<AccountMeta>, legs: &[CrossLeg], live_writable: bool) -> u8 {
    let mut mask = 0u8;
    for (i, leg) in legs.iter().enumerate() {
        match leg {
            CrossLeg::Live {
                position,
                market,
                oracle,
            } => {
                mask |= 1u8 << i;
                if live_writable {
                    metas.push(AccountMeta::new(*position, false));
                    metas.push(AccountMeta::new(*market, false));
                } else {
                    metas.push(AccountMeta::new_readonly(*position, false));
                    metas.push(AccountMeta::new_readonly(*market, false));
                }
                metas.push(AccountMeta::new_readonly(*oracle, false));
            }
            CrossLeg::Flat { position } => {
                metas.push(AccountMeta::new_readonly(*position, false));
            }
        }
    }
    mask
}

/// ACCUMULATE (`process_chunk`): fold `[start_index, start_index+max_count)` of
/// the resting slab into the histogram.
pub fn process_chunk(
    pdas: &MarketPdas,
    cranker: Pubkey,
    shard_id: u16,
    start_index: u32,
    max_count: u32,
) -> Instruction {
    ProcessChunk {
        cranker,
        market: pdas.market,
        order_slab: pdas.slab_shard(shard_id),
        histogram: pdas.histogram,
        event_authority: pdas.event_authority,
        tempo_program: TEMPO_PROGRAM_ID,
    }
    .instruction(ProcessChunkInstructionArgs {
        start_index,
        max_count,
    })
}

/// The optional money-path accounts a `settle_fill` may carry. The program
/// REQUIRES `position` whenever the computed fill is non-zero (C1); the keeper
/// always supplies it for a non-empty order. The rest drive the margin/fee path.
#[derive(Clone, Copy, Debug, Default)]
pub struct SettleMoney {
    pub position: Option<Pubkey>,
    pub user_collateral: Option<Pubkey>,
    pub vault: Option<Pubkey>,
    pub integrator_collateral: Option<Pubkey>,
}

impl SettleMoney {
    /// Derive the full money-path set for settling `owner`'s order on a market
    /// settling in `collateral_mint` (known-issues §2.11). The mint-scoped ledger
    /// is ALWAYS attached: it is a deterministic PDA, and the program requires it
    /// exactly when the settle releases a reservation (a fully-consuming settle) —
    /// attaching it unconditionally removes the "did this settle need it?" client
    /// guesswork. Position + vault ride along for the same reason. Safe on any
    /// money-path market: `submit_order` already required position + ledger to
    /// exist for every order that can be settled there (§1.1). Use
    /// `SettleMoney::default()` only for a clearing-only (no-money-path) market.
    pub fn for_order_owner(pdas: &MarketPdas, owner: Pubkey, collateral_mint: Pubkey) -> Self {
        Self {
            position: Some(crate::pda::position(&pdas.market, &owner).0),
            user_collateral: Some(crate::pda::user_collateral(&owner, &collateral_mint).0),
            vault: Some(crate::pda::vault(&collateral_mint).0),
            integrator_collateral: None,
        }
    }
}

/// DISCOVER (`finalize_clear`): the completeness-gated single-pass cross. Design Z
/// (DDR-1): pass ALL of the market's slab shards in `shards` (read-only) — finalize scans
/// every one for completeness, so `shards.len()` must equal `Market.num_slab_shards`. The two
/// crank-fee slots hold fixed positions (program-id sentinels when `crank_fee_accounts` is
/// `None`) so the shard region starts at a deterministic offset. Built manually because Codama
/// cannot model the trailing shard list.
pub fn finalize_clear(
    pdas: &MarketPdas,
    cranker: Pubkey,
    crank_fee_accounts: Option<(Pubkey, Pubkey)>,
    shards: &[Pubkey],
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(cranker, true),
        AccountMeta::new(pdas.market, false),
        AccountMeta::new_readonly(pdas.histogram, false),
        AccountMeta::new(pdas.clearing, false),
        AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        AccountMeta::new_readonly(pdas.event_authority, false),
        AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false),
    ];
    // Crank-fee optional slots at fixed positions 7/8 (program-id sentinel = "omitted").
    match crank_fee_accounts {
        Some((cc, v)) => {
            accounts.push(AccountMeta::new(cc, false));
            accounts.push(AccountMeta::new(v, false));
        }
        None => {
            accounts.push(AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false));
            accounts.push(AccountMeta::new_readonly(TEMPO_PROGRAM_ID, false));
        }
    }
    // All slab shards, read-only.
    for shard in shards {
        accounts.push(AccountMeta::new_readonly(*shard, false));
    }
    Instruction {
        program_id: TEMPO_PROGRAM_ID,
        accounts,
        data: alloc_finalize_data(pdas.clearing_bump),
    }
}

/// `finalize_clear` instruction data: `[discriminator, clearing_bump]`.
fn alloc_finalize_data(clearing_bump: u8) -> Vec<u8> {
    vec![FINALIZE_CLEAR_DISCRIMINATOR, clearing_bump]
}

/// SETTLE (`settle_fill`): one order pulls its own fill. `slot_hint` is the slab
/// slot index (O(1) on-chain lookup, validated not trusted).
pub fn settle_fill(
    pdas: &MarketPdas,
    cranker: Pubkey,
    shard_id: u16,
    order_id: u64,
    slot_hint: u32,
    money: &SettleMoney,
) -> Instruction {
    SettleFill {
        cranker,
        market: pdas.market,
        order_slab: pdas.slab_shard(shard_id),
        clearing_result: pdas.clearing,
        event_authority: pdas.event_authority,
        tempo_program: TEMPO_PROGRAM_ID,
        position: money.position,
        user_collateral: money.user_collateral,
        vault: money.vault,
        integrator_collateral: money.integrator_collateral,
    }
    .instruction(SettleFillInstructionArgs {
        order_id,
        slot_hint,
    })
}

/// The optional money-path accounts a `submit_order` may carry. The program
/// REQUIRES both whenever the market has a money path (`maintenance_margin_bps > 0`)
/// and rejects a partial set (submit_order/accounts.rs); a clearing-only market omits
/// them. Mirrors [`SettleMoney`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SubmitMoney {
    pub position: Option<Pubkey>,
    pub user_collateral: Option<Pubkey>,
}

impl SubmitMoney {
    /// Derive the money accounts for `trader` on a market settling in `collateral_mint`.
    pub fn for_trader(pdas: &MarketPdas, trader: Pubkey, collateral_mint: Pubkey) -> Self {
        Self {
            position: Some(crate::pda::position(&pdas.market, &trader).0),
            user_collateral: Some(crate::pda::user_collateral(&trader, &collateral_mint).0),
        }
    }
}

/// SUBMIT (`submit_order`): a taker resting order. `price` must be tick-aligned and
/// in-window; `quantity != 0`. Pass `SubmitMoney::default()` for a clearing-only
/// market, or `SubmitMoney::for_trader(..)` on a money-path market.
#[allow(clippy::too_many_arguments)]
pub fn submit_order(
    pdas: &MarketPdas,
    trader: Pubkey,
    side: u8,
    price: u64,
    quantity: u64,
    reduce_only: bool,
    shard_id: u16,
    expires_at_auction: u64,
    money: &SubmitMoney,
) -> Instruction {
    SubmitOrder {
        trader,
        market: pdas.market,
        order_slab: pdas.slab_shard(shard_id),
        event_authority: pdas.event_authority,
        tempo_program: TEMPO_PROGRAM_ID,
        position: money.position,
        user_collateral: money.user_collateral,
    }
    .instruction(SubmitOrderInstructionArgs {
        side,
        price,
        quantity,
        reduce_only,
        shard_id,
        expires_at_auction,
    })
}

/// CANCEL (`cancel_order`): remove a resting order (Collect phase only). The order
/// OWNER may always cancel; ANYONE may reap an EXPIRED order (DDR-3 correction #2).
/// `signer` is the caller. On a money-path market pass the ORDER OWNER's
/// `user_collateral` so the reserved margin is released to the owner — the program
/// validates it belongs to `order.trader`, so a reaper cannot redirect margin to itself.
pub fn cancel_order(
    pdas: &MarketPdas,
    signer: Pubkey,
    shard_id: u16,
    order_id: u64,
    slot_hint: u32,
    user_collateral: Option<Pubkey>,
) -> Instruction {
    CancelOrder {
        trader: signer,
        market: pdas.market,
        order_slab: pdas.slab_shard(shard_id),
        event_authority: pdas.event_authority,
        tempo_program: TEMPO_PROGRAM_ID,
        user_collateral,
    }
    .instruction(CancelOrderInstructionArgs {
        order_id,
        slot_hint,
    })
}

/// Roll to the next round (`start_auction`). `oracle` is the market's bound oracle
/// (re-snaps the tick window); pass `MarketView::oracle`.
pub fn start_auction(pdas: &MarketPdas, cranker: Pubkey, oracle: Pubkey) -> Instruction {
    StartAuction {
        cranker,
        market: pdas.market,
        histogram: pdas.histogram,
        oracle,
    }
    .instruction()
}

/// Create one OrderSlab shard (`init_shard`, Stage A sharding). Call once per shard
/// (`[0, num_slab_shards)`) after `initialize_market`, before trading.
pub fn init_shard(pdas: &MarketPdas, payer: Pubkey, shard_id: u16) -> Instruction {
    let (shard, bump) = crate::pda::order_slab(&pdas.market, shard_id);
    InitShard {
        payer,
        market: pdas.market,
        order_slab: shard,
        system_program: SYSTEM_PROGRAM_ID,
    }
    .instruction(InitShardInstructionArgs { shard_id, bump })
}

/// Drain + re-arm one shard for the next round (`reset_shard`, Stage A sharding).
/// Call once per shard after settlement; `start_auction` rolls once all are ready.
pub fn reset_shard(pdas: &MarketPdas, cranker: Pubkey, shard_id: u16) -> Instruction {
    ResetShard {
        cranker,
        market: pdas.market,
        order_slab: pdas.slab_shard(shard_id),
    }
    .instruction()
}

/// Accrue funding from the oracle (`update_funding`; phase-independent, scheduled).
pub fn update_funding(pdas: &MarketPdas, cranker: Pubkey, oracle: Pubkey) -> Instruction {
    UpdateFunding {
        cranker,
        market: pdas.market,
        oracle,
        event_authority: pdas.event_authority,
        tempo_program: TEMPO_PROGRAM_ID,
    }
    .instruction()
}

/// Fold one maker quote into the histogram (`process_maker_quote`).
pub fn process_maker_quote(pdas: &MarketPdas, cranker: Pubkey, maker_quote: Pubkey) -> Instruction {
    ProcessMakerQuote {
        cranker,
        market: pdas.market,
        histogram: pdas.histogram,
        maker_quote,
    }
    .instruction()
}

/// Settle one maker quote against the published cross (`settle_maker_quote`).
pub fn settle_maker_quote(
    pdas: &MarketPdas,
    cranker: Pubkey,
    maker_quote: Pubkey,
    position: Pubkey,
    user_collateral: Option<Pubkey>,
    vault: Option<Pubkey>,
) -> Instruction {
    SettleMakerQuote {
        cranker,
        market: pdas.market,
        clearing_result: pdas.clearing,
        order_slab: pdas.order_slab,
        maker_quote,
        position,
        user_collateral,
        vault,
    }
    .instruction()
}

/// One rung of a maker-quote ladder: a tick offset from `mid_tick` and a
/// base-lot size. On-chain the bid tick is `mid_tick - offset` and the ask tick
/// is `mid_tick + offset` (`process_maker_quote`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub offset: u16,
    pub size: u64,
}

/// Pack up to `MAX_MAKER_LEVELS` (8) rungs into the flat `[u8; 80]` region the
/// program parses: each 10-byte rung is `u16 LE offset` then `u64 LE size`,
/// zero-padded past the supplied rungs.
pub fn encode_levels(levels: &[Level]) -> [u8; 80] {
    let mut buf = [0u8; 80];
    for (i, lvl) in levels
        .iter()
        .take(crate::consts::MAX_MAKER_LEVELS)
        .enumerate()
    {
        let b = i * 10;
        buf[b..b + 2].copy_from_slice(&lvl.offset.to_le_bytes());
        buf[b + 2..b + 10].copy_from_slice(&lvl.size.to_le_bytes());
    }
    buf
}

/// Create a maker's `MakerQuote` PDA (`init_maker_quote`). `delegate` may be the
/// zero pubkey for "maker only". The bump is the `maker_quote` PDA bump.
pub fn init_maker_quote(
    pdas: &MarketPdas,
    maker: Pubkey,
    maker_quote: Pubkey,
    maker_quote_bump: u8,
    expiry_slots: u64,
    delegate: Pubkey,
) -> Instruction {
    InitMakerQuote {
        maker,
        market: pdas.market,
        maker_quote,
        system_program: SYSTEM_PROGRAM_ID,
    }
    .instruction(InitMakerQuoteInstructionArgs {
        maker_quote_bump,
        expiry_slots,
        delegate: delegate.to_bytes(),
    })
}

/// Overwrite a maker quote's whole ladder (`update_maker_quote_levels`). Valid
/// only in the `Collect` phase; `sequence` must strictly exceed the on-chain one.
#[allow(clippy::too_many_arguments)]
pub fn update_maker_quote_levels(
    pdas: &MarketPdas,
    writer: Pubkey,
    maker_quote: Pubkey,
    sequence: u64,
    mid_tick: u32,
    bids: &[Level],
    asks: &[Level],
) -> Instruction {
    UpdateMakerQuoteLevels {
        writer,
        market: pdas.market,
        maker_quote,
    }
    .instruction(UpdateMakerQuoteLevelsInstructionArgs {
        sequence,
        mid_tick,
        num_bids: bids.len().min(crate::consts::MAX_MAKER_LEVELS) as u8,
        num_asks: asks.len().min(crate::consts::MAX_MAKER_LEVELS) as u8,
        bid_levels: encode_levels(bids),
        ask_levels: encode_levels(asks),
    })
}

/// Deactivate a maker quote (`clear_maker_quote`); `sequence` must strictly
/// exceed the on-chain one.
pub fn clear_maker_quote(
    pdas: &MarketPdas,
    writer: Pubkey,
    maker_quote: Pubkey,
    sequence: u64,
) -> Instruction {
    ClearMakerQuote {
        writer,
        market: pdas.market,
        maker_quote,
    }
    .instruction(ClearMakerQuoteInstructionArgs { sequence })
}

/// Create a trader's `Position` PDA for the market (`init_position`). The
/// position PDA + bump are derived from `[b"position", market, owner]`.
pub fn init_position(pdas: &MarketPdas, payer: Pubkey, owner: Pubkey) -> Instruction {
    let (position, position_bump) = crate::pda::position(&pdas.market, &owner);
    InitPosition {
        payer,
        owner,
        market: pdas.market,
        position,
        system_program: SYSTEM_PROGRAM_ID,
    }
    .instruction(InitPositionInstructionArgs { position_bump })
}

/// SPL Token program id (`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`).
pub const SPL_TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

/// Create a trader's mint-scoped `UserCollateral` ledger PDA (`init_collateral`,
/// CR-3). The ledger is scoped to `collateral_mint`; the per-mint `vault` is passed
/// read-only as the source of truth for which mints are valid collateral.
pub fn init_collateral(payer: Pubkey, owner: Pubkey, collateral_mint: Pubkey) -> Instruction {
    let (user_collateral, bump) = crate::pda::user_collateral(&owner, &collateral_mint);
    let (vault, _) = crate::pda::vault(&collateral_mint);
    InitCollateral {
        payer,
        owner,
        user_collateral,
        vault,
        system_program: SYSTEM_PROGRAM_ID,
    }
    .instruction(InitCollateralInstructionArgs { bump })
}

/// Deposit `amount` of collateral into the vault (`deposit`). `user_token_account`
/// is the depositor's SPL token account; `vault_token_account` is the vault's.
pub fn deposit(
    owner: Pubkey,
    collateral_mint: Pubkey,
    vault_token_account: Pubkey,
    user_token_account: Pubkey,
    token_program: Pubkey,
    amount: u64,
) -> Instruction {
    let (user_collateral, _) = crate::pda::user_collateral(&owner, &collateral_mint);
    let (vault, _) = crate::pda::vault(&collateral_mint);
    Deposit {
        owner,
        user_collateral,
        vault,
        vault_token_account,
        user_token_account,
        token_program,
    }
    .instruction(DepositInstructionArgs { amount })
}

/// Accounts for [`liquidate_cross`] (the fixed prefix; member legs are appended
/// separately as the `live_mask` account list).
#[derive(Clone, Copy, Debug)]
pub struct LiquidateCrossParams {
    pub liquidator: Pubkey,
    pub margin_account: Pubkey,
    pub user_collateral: Pubkey,
    pub vault: Pubkey,
    pub liquidator_collateral: Pubkey,
}

/// LiquidateCross with the cross-margin member legs appended and the `live_mask`
/// derived from them.
pub fn liquidate_cross(params: &LiquidateCrossParams, legs: &[CrossLeg]) -> Instruction {
    let mut remaining = Vec::new();
    let live_mask = push_cross_legs(&mut remaining, legs, true);
    LiquidateCross {
        liquidator: params.liquidator,
        margin_account: params.margin_account,
        user_collateral: params.user_collateral,
        vault: params.vault,
        liquidator_collateral: params.liquidator_collateral,
        event_authority: event_authority().0,
        tempo_program: TEMPO_PROGRAM_ID,
    }
    .instruction_with_remaining_accounts(LiquidateCrossInstructionArgs { live_mask }, &remaining)
}

/// Fixed account set for the isolated [`liquidate`] (disc 14). `oracle` is the
/// market's bound Pyth account (`MarketView::oracle`); `user_collateral` is the
/// position owner's ledger, `liquidator_collateral` the caller's (paid the penalty).
#[derive(Clone, Copy, Debug)]
pub struct LiquidateParams {
    pub liquidator: Pubkey,
    pub market: Pubkey,
    pub oracle: Pubkey,
    pub position: Pubkey,
    pub user_collateral: Pubkey,
    pub vault: Pubkey,
    pub liquidator_collateral: Pubkey,
}

/// Isolated `liquidate`: close one maintenance-breaching position, oracle-priced.
pub fn liquidate(p: &LiquidateParams) -> Instruction {
    Liquidate {
        liquidator: p.liquidator,
        market: p.market,
        oracle: p.oracle,
        position: p.position,
        user_collateral: p.user_collateral,
        vault: p.vault,
        liquidator_collateral: p.liquidator_collateral,
        event_authority: event_authority().0,
        tempo_program: TEMPO_PROGRAM_ID,
    }
    .instruction()
}

/// Accounts for [`withdraw_cross`] (the fixed prefix).
#[derive(Clone, Copy, Debug)]
pub struct WithdrawCrossParams {
    pub owner: Pubkey,
    pub margin_account: Pubkey,
    pub user_collateral: Pubkey,
    pub vault: Pubkey,
    pub vault_authority: Pubkey,
    pub vault_token_account: Pubkey,
    pub user_token_account: Pubkey,
    pub token_program: Pubkey,
}

/// WithdrawCross with the member legs appended (read-only) and `live_mask` set.
pub fn withdraw_cross(params: &WithdrawCrossParams, amount: u64, legs: &[CrossLeg]) -> Instruction {
    let mut remaining = Vec::new();
    let live_mask = push_cross_legs(&mut remaining, legs, false);
    WithdrawCross {
        owner: params.owner,
        margin_account: params.margin_account,
        user_collateral: params.user_collateral,
        vault: params.vault,
        vault_authority: params.vault_authority,
        vault_token_account: params.vault_token_account,
        user_token_account: params.user_token_account,
        token_program: params.token_program,
    }
    .instruction_with_remaining_accounts(
        WithdrawCrossInstructionArgs { amount, live_mask },
        &remaining,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_chunk_targets_program() {
        let market = Pubkey::new_unique();
        let pdas = MarketPdas::derive(market);
        let ix = process_chunk(&pdas, Pubkey::new_unique(), 0, 0, 16);
        assert_eq!(ix.program_id, TEMPO_PROGRAM_ID);
        assert_eq!(ix.data[0], PROCESS_CHUNK_DISCRIMINATOR);
        assert_eq!(ix.accounts.len(), 6);
    }

    #[test]
    fn test_finalize_clear_wrapper() {
        let pdas = MarketPdas::derive(Pubkey::new_unique());
        let cranker = Pubkey::new_unique();
        // Design Z: 7 fixed + 2 crank-fee slots (sentinels when omitted) + K shards.
        let shards = [pdas.slab_shard(0), pdas.slab_shard(1), pdas.slab_shard(2)];
        // No crank-fee accounts → 7 + 2 sentinels + 3 shards = 12.
        let bare = finalize_clear(&pdas, cranker, None, &shards);
        assert_eq!(bare.program_id, TEMPO_PROGRAM_ID);
        assert_eq!(bare.data[0], FINALIZE_CLEAR_DISCRIMINATOR);
        assert_eq!(bare.data[1], pdas.clearing_bump);
        assert_eq!(bare.accounts.len(), 12);
        // Omitted crank-fee slots are the program-id sentinel (parser treats as "not provided").
        assert_eq!(bare.accounts[7].pubkey, TEMPO_PROGRAM_ID);
        assert_eq!(bare.accounts[8].pubkey, TEMPO_PROGRAM_ID);
        // Shards are read-only.
        assert!(!bare.accounts[9].is_writable);
        // With the crank-fee pair → 7 + 2 + 3 = 12 (real fee accounts in slots 7/8).
        let fee = finalize_clear(
            &pdas,
            cranker,
            Some((Pubkey::new_unique(), Pubkey::new_unique())),
            &shards,
        );
        assert_eq!(fee.accounts.len(), 12);
        assert_ne!(fee.accounts[7].pubkey, TEMPO_PROGRAM_ID);
    }

    #[test]
    fn test_settle_fill_wrapper_account_counts() {
        let pdas = MarketPdas::derive(Pubkey::new_unique());
        let cranker = Pubkey::new_unique();
        // Bare clearing-only settle → 6 accounts.
        let bare = settle_fill(&pdas, cranker, 7, 3, 0, &SettleMoney::default());
        assert_eq!(bare.data[0], SETTLE_FILL_DISCRIMINATOR);
        assert_eq!(bare.accounts.len(), 6);
        // Full money path → 6 + position + collateral + vault + integrator = 10.
        let full = settle_fill(
            &pdas,
            cranker,
            7,
            3,
            0,
            &SettleMoney {
                position: Some(Pubkey::new_unique()),
                user_collateral: Some(Pubkey::new_unique()),
                vault: Some(Pubkey::new_unique()),
                integrator_collateral: Some(Pubkey::new_unique()),
            },
        );
        assert_eq!(full.accounts.len(), 10);
    }

    #[test]
    fn settle_builder_always_attaches_ledger() {
        // known-issues §2.11: the derivation helper must ALWAYS attach the
        // owner's mint-scoped ledger (+ position + vault) so a fully-consuming
        // settle can never fail MissingSettleAccounts from a client omission.
        let pdas = MarketPdas::derive(Pubkey::new_unique());
        let owner = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let money = SettleMoney::for_order_owner(&pdas, owner, mint);
        assert_eq!(
            money.position.unwrap(),
            crate::pda::position(&pdas.market, &owner).0
        );
        assert_eq!(
            money.user_collateral.unwrap(),
            crate::pda::user_collateral(&owner, &mint).0
        );
        assert_eq!(money.vault.unwrap(), crate::pda::vault(&mint).0);
        assert!(money.integrator_collateral.is_none());
        // And the wrapper carries them: 6 base + position + ledger + vault = 9.
        let ix = settle_fill(&pdas, Pubkey::new_unique(), 0, 1, 0, &money);
        assert_eq!(ix.accounts.len(), 9);
    }

    #[test]
    fn test_submit_order_wrapper_account_counts() {
        let pdas = MarketPdas::derive(Pubkey::new_unique());
        let trader = Pubkey::new_unique();
        // Clearing-only (no money path) → 5 accounts.
        let bare = submit_order(
            &pdas,
            trader,
            0,
            100,
            5,
            false,
            0,
            0,
            &SubmitMoney::default(),
        );
        assert_eq!(bare.program_id, TEMPO_PROGRAM_ID);
        assert_eq!(bare.data[0], SUBMIT_ORDER_DISCRIMINATOR);
        assert_eq!(bare.accounts.len(), 5);
        // Money path (position + collateral) → 7 accounts.
        let mint = Pubkey::new_unique();
        let full = submit_order(
            &pdas,
            trader,
            1,
            200,
            7,
            true,
            0,
            0,
            &SubmitMoney::for_trader(&pdas, trader, mint),
        );
        assert_eq!(full.accounts.len(), 7);
    }

    #[test]
    fn test_roll_funding_maker_wrappers_target_program() {
        let pdas = MarketPdas::derive(Pubkey::new_unique());
        let cranker = Pubkey::new_unique();
        let oracle = Pubkey::new_unique();
        let mq = Pubkey::new_unique();
        let pos = Pubkey::new_unique();

        // Stage A: start_auction dropped the order_slab account (shards are drained by
        // reset_shard first; the roll gates on shards_ready) → 4 accounts.
        let sa = start_auction(&pdas, cranker, oracle);
        assert_eq!(sa.data[0], START_AUCTION_DISCRIMINATOR);
        assert_eq!(sa.accounts.len(), 4);

        let uf = update_funding(&pdas, cranker, oracle);
        assert_eq!(uf.data[0], UPDATE_FUNDING_DISCRIMINATOR);
        assert_eq!(uf.accounts.len(), 5);

        let pmq = process_maker_quote(&pdas, cranker, mq);
        assert_eq!(pmq.data[0], PROCESS_MAKER_QUOTE_DISCRIMINATOR);
        assert_eq!(pmq.accounts.len(), 4);

        let smq = settle_maker_quote(&pdas, cranker, mq, pos, None, None);
        assert_eq!(smq.data[0], SETTLE_MAKER_QUOTE_DISCRIMINATOR);
        assert_eq!(smq.accounts.len(), 6);
        for ix in [sa, uf, pmq, smq] {
            assert_eq!(ix.program_id, TEMPO_PROGRAM_ID);
        }
    }

    #[test]
    fn test_encode_levels_byte_layout() {
        let levels = [
            Level {
                offset: 1,
                size: 100,
            },
            Level {
                offset: 2,
                size: 250,
            },
        ];
        let buf = encode_levels(&levels);
        // rung 0: offset @0..2, size @2..10
        assert_eq!(u16::from_le_bytes([buf[0], buf[1]]), 1);
        assert_eq!(u64::from_le_bytes(buf[2..10].try_into().unwrap()), 100);
        // rung 1: offset @10..12, size @12..20
        assert_eq!(u16::from_le_bytes([buf[10], buf[11]]), 2);
        assert_eq!(u64::from_le_bytes(buf[12..20].try_into().unwrap()), 250);
        // rung 2 onward zero-padded
        assert!(buf[20..].iter().all(|&b| b == 0));
    }

    #[test]
    fn test_encode_levels_caps_at_max() {
        let many = vec![Level { offset: 1, size: 1 }; 12];
        // 80 bytes only holds 8 rungs; extra rungs are silently dropped.
        let buf = encode_levels(&many);
        assert_eq!(buf.len(), 80);
    }

    #[test]
    fn test_maker_quote_and_position_wrappers() {
        let pdas = MarketPdas::derive(Pubkey::new_unique());
        let maker = Pubkey::new_unique();
        let (mq, mq_bump) = crate::pda::maker_quote(&pdas.market, &maker);

        let init = init_maker_quote(&pdas, maker, mq, mq_bump, 0, Pubkey::default());
        assert_eq!(init.data[0], INIT_MAKER_QUOTE_DISCRIMINATOR);
        assert_eq!(init.accounts.len(), 4);

        let bids = [Level {
            offset: 1,
            size: 100,
        }];
        let asks = [Level {
            offset: 1,
            size: 100,
        }];
        let upd = update_maker_quote_levels(&pdas, maker, mq, 7, 33, &bids, &asks);
        assert_eq!(upd.data[0], UPDATE_MAKER_QUOTE_LEVELS_DISCRIMINATOR);
        assert_eq!(upd.accounts.len(), 3);

        let clr = clear_maker_quote(&pdas, maker, mq, 8);
        assert_eq!(clr.data[0], CLEAR_MAKER_QUOTE_DISCRIMINATOR);
        assert_eq!(clr.accounts.len(), 3);

        let ip = init_position(&pdas, maker, maker);
        assert_eq!(ip.data[0], INIT_POSITION_DISCRIMINATOR);
        assert_eq!(ip.accounts.len(), 5);

        for ix in [init, upd, clr, ip] {
            assert_eq!(ix.program_id, TEMPO_PROGRAM_ID);
        }
    }

    #[test]
    fn test_collateral_money_wrappers() {
        let owner = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let ic = init_collateral(owner, owner, mint);
        assert_eq!(ic.data[0], INIT_COLLATERAL_DISCRIMINATOR);
        assert_eq!(ic.accounts.len(), 5);
        let dep = deposit(
            owner,
            mint,
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            SPL_TOKEN_PROGRAM_ID,
            1_000,
        );
        assert_eq!(dep.data[0], DEPOSIT_DISCRIMINATOR);
        assert_eq!(dep.accounts.len(), 6);
        for ix in [ic, dep] {
            assert_eq!(ix.program_id, TEMPO_PROGRAM_ID);
        }
    }

    #[test]
    fn test_liquidate_wrapper() {
        let ix = liquidate(&LiquidateParams {
            liquidator: Pubkey::new_unique(),
            market: Pubkey::new_unique(),
            oracle: Pubkey::new_unique(),
            position: Pubkey::new_unique(),
            user_collateral: Pubkey::new_unique(),
            vault: Pubkey::new_unique(),
            liquidator_collateral: Pubkey::new_unique(),
        });
        assert_eq!(ix.program_id, TEMPO_PROGRAM_ID);
        assert_eq!(ix.data[0], LIQUIDATE_DISCRIMINATOR);
        assert_eq!(ix.accounts.len(), 9);
    }

    #[test]
    fn test_live_mask_bits_and_account_count() {
        let mut metas = Vec::new();
        let legs = [
            CrossLeg::Live {
                position: Pubkey::new_unique(),
                market: Pubkey::new_unique(),
                oracle: Pubkey::new_unique(),
            },
            CrossLeg::Flat {
                position: Pubkey::new_unique(),
            },
            CrossLeg::Live {
                position: Pubkey::new_unique(),
                market: Pubkey::new_unique(),
                oracle: Pubkey::new_unique(),
            },
        ];
        let mask = push_cross_legs(&mut metas, &legs, true);
        // bits 0 and 2 set (live), bit 1 clear (flat).
        assert_eq!(mask, 0b101);
        // two live triples (3 each) + one flat (1) = 7 accounts.
        assert_eq!(metas.len(), 7);
    }
}
