use codama::CodamaInstructions;

/// Instructions for the Tempo Program (clearing engine).
///
/// Uses the canonical Codama `#[codama(account(...))]` style.
/// The clearing engine has no token transfers and no CPI event emission, so there is
/// no event authority / token program plumbing here.
#[allow(clippy::large_enum_variant)]
#[repr(C, u8)]
#[derive(Clone, Debug, PartialEq, CodamaInstructions)]
pub enum TempoProgramInstruction {
    /// Create a Market plus its empty AuctionHistogram and OrderSlab.
    #[codama(account(name = "payer", docs = "Pays for account creation", signer, writable))]
    #[codama(account(name = "authority", docs = "Market authority / admin", signer))]
    #[codama(account(
        name = "market_seed",
        docs = "Random keypair seed for the market PDA",
        signer
    ))]
    #[codama(account(
        name = "market",
        docs = "Market PDA to be created",
        writable,
        default_value = pda("market", [seed("marketSeed", account("marketSeed"))])
    ))]
    #[codama(account(
        name = "histogram",
        docs = "AuctionHistogram PDA (the mailboxes) to be created",
        writable,
        default_value = pda("auctionHistogramHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "oracle",
        docs = "Oracle (Pyth PriceUpdateV2) recorded on the market; consumed by funding/liquidation"
    ))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    InitializeMarket {
        /// Bump for the market PDA
        #[codama(default_value = account_bump("market"))]
        market_bump: u8,
        /// Bump for the histogram PDA
        #[codama(default_value = account_bump("histogram"))]
        histogram_bump: u8,
        /// UNUSED (retained for wire-format stability). Stage A creates slab shards via
        /// `init_shard`, not here.
        order_slab_bump: u8,
        /// Price tick size
        tick_size: u64,
        /// Number of price ticks the histogram covers
        num_ticks: u32,
        /// Orders-per-auction cap (order slab capacity)
        orders_per_auction_cap: u32,
        /// Pyth feed id the market's oracle account must carry
        oracle_feed_id: [u8; 32],
        /// Maintenance margin requirement (bps)
        maintenance_margin_bps: u16,
        /// Liquidation penalty (bps)
        liquidation_penalty_bps: u16,
        /// Maker fee on each settled fill (bps, signed — negative = rebate)
        maker_fee_bps: i16,
        /// Taker fee on each settled fill (bps, signed — negative = rebate)
        taker_fee_bps: i16,
        /// Integrator revenue share (bps of the positive fee, 0..=10_000)
        integrator_share_bps: u16,
        /// Flat fee paid to the finalize cranker from the fee pool
        crank_fee: u64,
        /// Collateral mint this market settles in (binds it to the per-mint vault);
        /// all-zero for a market with no declared money path
        collateral_mint: [u8; 32],
        /// Meltdown-brake cap, bps per slot (0 = disabled)
        max_price_move_bps_per_slot: u16,
        /// Soft-stale window before the oracle is treated hard-stale, slots (0 = disabled)
        soft_stale_slots: u64,
        /// Initial-margin requirement (bps) — the buffer above maintenance; must be ≥ maintenance_margin_bps
        initial_margin_bps: u16,
        /// Max notional (|size|·entry) a single position may hold (0 = disabled)
        max_position_notional: u128,
        /// Number of OrderSlab shards (Stage A sharding, ≥ 1). Shards are created
        /// one-per-tx by `init_shard`, not here.
        num_slab_shards: u16,
        /// Minimum order notional `quantity·price` (anti-dust, 0 = disabled)
        min_order_notional: u64,
        /// Per-side open-interest soft cap (0 = disabled)
        max_open_interest: u128,
        /// Flat liquidator reward floor paid from insurance (0 = disabled)
        liquidation_reward_floor: u64,
        /// Partial-liquidation health buffer above maintenance, bps (0 = full close)
        liquidation_close_buffer_bps: u16,
    } = 0,

    /// Submit a resting order into the slab (phase must be Collect).
    #[codama(account(name = "trader", docs = "Order owner", signer, writable))]
    #[codama(account(
        name = "market",
        docs = "Market the order belongs to. Read-only (Design Z): submit writes only its own shard."
    ))]
    #[codama(account(
        name = "order_slab",
        docs = "OrderSlab SHARD to insert into. Stage A sharding: a market has num_slab_shards slabs at seeds [b\"order_slab\", market, shard_id.to_le_bytes()]. The client resolves the shard PDA for the chosen `shard_id` (least-full / hash) and passes it here; the processor validates the PDA against `shard_id`.",
        writable
    ))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    #[codama(account(
        name = "position",
        docs = "(Optional) trader's Position; required on a money-path market to reserve margin",
        writable,
        optional
    ))]
    #[codama(account(
        name = "user_collateral",
        docs = "(Optional) trader's collateral ledger; locks the order's worst-case initial margin",
        writable,
        optional
    ))]
    SubmitOrder {
        /// Side: 0 = buy, 1 = sell
        side: u8,
        /// Limit price (must be tick-aligned and non-zero)
        price: u64,
        /// Order quantity (must be non-zero). Taker-only: maker liquidity is
        /// posted through the MakerQuote book, not submit_order (§1.3).
        quantity: u64,
        /// Reduce-only: 1 = the order may only reduce an opposite position, so only
        /// the portion that would open new exposure reserves margin (missing-features
        /// §1.1/§2.2). 0 = a normal order (reserves the full worst case).
        reduce_only: bool,
        /// Which OrderSlab shard to insert into (`[0, num_slab_shards)`). Must match the
        /// `order_slab` account's stored shard id (Stage A sharding).
        shard_id: u16,
        /// Resting-order expiry (Stage B): 0 = good-till-cancelled; otherwise an absolute
        /// auction id at/after which the order stops resting (its leftover is consumed at
        /// settle instead of re-armed). A client typically sets `current_auction_id + N`;
        /// setting it EQUAL to the arm round (current auction id in Collect, current + 1
        /// mid-round) makes the order immediate-or-cancel — one auction, never rests.
        /// An expiry strictly before the arm round is rejected (OrderAlreadyExpired).
        expires_at_auction: u64,
    } = 1,

    /// Cancel a resting order before clearing begins.
    #[codama(account(name = "trader", docs = "Order owner", signer))]
    #[codama(account(
        name = "market",
        docs = "Market the order belongs to. Read-only (Design Z): cancel writes only its own shard."
    ))]
    #[codama(account(
        name = "order_slab",
        docs = "OrderSlab to remove from",
        writable,
        default_value = pda("orderSlabHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    #[codama(account(
        name = "user_collateral",
        docs = "(Optional) trader's collateral ledger; releases the order's reserved margin",
        writable,
        optional
    ))]
    CancelOrder {
        /// Id of the order to cancel
        order_id: u64,
        /// Slab slot index `order_id` is expected at (from the `OrderSubmitted`
        /// event); O(1) hint, validated and scan-fallback (known-issues §2.7)
        slot_hint: u32,
    } = 2,

    /// Phase 1 ACCUMULATE (permissionless): fold a bounded slice of resting
    /// orders into the histogram and mark them accumulated.
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer, writable))]
    #[codama(account(name = "market", docs = "Market being cleared", writable))]
    #[codama(account(
        name = "order_slab",
        docs = "OrderSlab to scan",
        writable,
        default_value = pda("orderSlabHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "histogram",
        docs = "AuctionHistogram to fold into",
        writable,
        default_value = pda("auctionHistogramHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    ProcessChunk {
        /// First slab slot index to process in this chunk
        start_index: u32,
        /// Maximum number of slots to process in this chunk (bounds CU)
        max_count: u32,
    } = 3,

    /// Phase 2 DISCOVER (permissionless): one pass over the buckets to find the
    /// clearing price and write the ClearingResult. Requires completeness.
    ///
    /// Design Z (DDR-1): ALL of the market's slab shards are passed as trailing remaining
    /// accounts (read-only), after the fixed accounts and the two crank-fee optional slots
    /// (program-id sentinels when omitted). finalize scans every shard for completeness; the
    /// count must equal `num_slab_shards`. Codama cannot model the variadic list, so callers
    /// append the shard metas (see the `finalize_clear` SDK builder).
    #[codama(account(
        name = "cranker",
        docs = "Permissionless caller (paid a fee)",
        signer,
        writable
    ))]
    #[codama(account(name = "market", docs = "Market being cleared", writable))]
    #[codama(account(
        name = "histogram",
        docs = "AuctionHistogram to scan",
        default_value = pda("auctionHistogramHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "clearing_result",
        docs = "ClearingResult PDA to be created/written",
        writable,
        default_value = pda("clearingResult", [seed("market", account("market"))])
    ))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    #[codama(account(
        name = "cranker_collateral",
        docs = "(Optional) cranker's collateral ledger to receive the crank fee",
        writable,
        optional
    ))]
    #[codama(account(
        name = "vault",
        docs = "(Optional) fee/insurance pool the crank fee is drawn from",
        writable,
        optional
    ))]
    FinalizeClear {
        /// Bump for the clearing result PDA
        #[codama(default_value = account_bump("clearing_result"))]
        clearing_bump: u8,
    } = 4,

    /// Phase 3 SETTLE (permissionless to trigger): caller self-computes ONE
    /// order's fill from the ClearingResult and marks it consumed.
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(name = "market", docs = "Market being settled", writable))]
    #[codama(account(
        name = "order_slab",
        docs = "OrderSlab SHARD holding the order (Stage A sharding: seeds [b\"order_slab\", market, shard_id]). The client passes the shard the order lives in (known from the OrderSubmitted event's shard_id); the processor validates its PDA.",
        writable
    ))]
    #[codama(account(
        name = "clearing_result",
        docs = "Published clearing result",
        default_value = pda("clearingResult", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    #[codama(account(
        name = "position",
        docs = "(Optional) order owner's Position to apply the fill to",
        writable,
        optional
    ))]
    #[codama(account(
        name = "user_collateral",
        docs = "(Optional) owner's collateral ledger; locks initial margin on the fill",
        writable,
        optional
    ))]
    #[codama(account(
        name = "vault",
        docs = "(Optional) supplies the maintenance-margin bps and the insurance pool that floats PnL/fees/socialized loss (mutated on a non-zero fill)",
        writable,
        optional
    ))]
    #[codama(account(
        name = "integrator_collateral",
        docs = "(Optional) integrator ledger to receive a share of a positive fee",
        writable,
        optional
    ))]
    SettleFill {
        /// Id of the order to settle
        order_id: u64,
        /// Slab slot index `order_id` is expected at (from the `OrderSubmitted`
        /// event); O(1) hint, validated and scan-fallback (known-issues §2.7)
        slot_hint: u32,
    } = 5,

    /// Roll the market into its next round (permissionless; only succeeds once
    /// the prior round is fully settled). Zeroes the histogram + slab.
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(name = "market", docs = "Market to roll forward", writable))]
    #[codama(account(
        name = "histogram",
        docs = "AuctionHistogram to zero",
        writable,
        default_value = pda("auctionHistogramHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "oracle",
        docs = "Market's bound Pyth oracle; the new round's tick window is re-snapped onto it (known-issues §2.7). Stale price carries the previous window forward. Stage A: shards are drained by reset_shard first; the roll gates on shards_ready == num_slab_shards."
    ))]
    StartAuction {} = 6,

    /// Create a trader's Position account for a market.
    #[codama(account(
        name = "payer",
        docs = "Pays for the position account",
        signer,
        writable
    ))]
    #[codama(account(name = "owner", docs = "Trader the position belongs to", signer))]
    #[codama(account(name = "market", docs = "Market the position trades"))]
    #[codama(account(
        name = "position",
        docs = "Position PDA to create",
        writable,
        default_value = pda("position", [seed("market", account("market")), seed("owner", account("owner"))])
    ))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    InitPosition {
        /// Bump for the position PDA
        #[codama(default_value = account_bump("position"))]
        position_bump: u8,
    } = 7,

    /// Read the market's bound Pyth oracle, derive the mark price, emit it.
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(name = "market", docs = "Market whose bound oracle is read"))]
    #[codama(account(
        name = "oracle",
        docs = "Pyth PriceUpdateV2 account bound to the market"
    ))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    ReadOracle {} = 8,

    /// Admin: create the global collateral `Vault` singleton.
    #[codama(account(name = "payer", docs = "Pays for the vault account", signer, writable))]
    #[codama(account(name = "admin", docs = "Vault admin", signer))]
    #[codama(account(
        name = "vault",
        docs = "Per-collateral vault PDA to create",
        writable,
        default_value = pda("vault", [seed("collateralMint", account("collateralMint"))])
    ))]
    #[codama(account(
        name = "vault_token_account",
        docs = "SPL token account owned by the vault authority PDA"
    ))]
    #[codama(account(name = "collateral_mint", docs = "Collateral mint"))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    InitVault {
        /// Bump for the vault PDA
        #[codama(default_value = account_bump("vault"))]
        vault_bump: u8,
        /// Bump for the vault authority PDA
        authority_bump: u8,
    } = 9,

    /// Trader: create their `UserCollateral` ledger (mint-scoped, CR-3).
    #[codama(account(name = "payer", docs = "Pays for the ledger account", signer, writable))]
    #[codama(account(name = "owner", docs = "Trader the ledger belongs to", signer))]
    // CR-3: the ledger PDA is `[b"collateral", owner, vault.collateral_mint]`. The mint
    // is read from the vault (not an instruction account), so it cannot be auto-derived
    // here — clients must pass the resolved mint-scoped address.
    #[codama(account(
        name = "user_collateral",
        docs = "UserCollateral PDA to create (seeds [collateral, owner, vault.collateral_mint])",
        writable
    ))]
    #[codama(account(
        name = "vault",
        docs = "Per-collateral vault whose collateral_mint scopes the ledger (CR-3)"
    ))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    InitCollateral {
        /// Bump for the user collateral PDA
        #[codama(default_value = account_bump("user_collateral"))]
        bump: u8,
    } = 10,

    /// Trader: deposit collateral into the vault.
    #[codama(account(name = "owner", docs = "Depositing trader", signer))]
    #[codama(account(
        name = "user_collateral",
        docs = "Owner's collateral ledger (mint-scoped: seeds [collateral, owner, vault.collateral_mint], CR-3)",
        writable
    ))]
    #[codama(account(
        name = "vault",
        docs = "Per-collateral vault (the total_user_balance aggregate is updated, §3.4)",
        writable
    ))]
    #[codama(account(
        name = "vault_token_account",
        docs = "Vault token account (transfer destination)",
        writable
    ))]
    #[codama(account(
        name = "user_token_account",
        docs = "Owner token account (transfer source)",
        writable
    ))]
    #[codama(account(name = "token_program", docs = "SPL token program", default_value = program("token")))]
    Deposit {
        /// Collateral base units to deposit
        amount: u64,
    } = 11,

    /// Trader: withdraw free collateral from the vault.
    #[codama(account(name = "owner", docs = "Withdrawing trader", signer))]
    #[codama(account(
        name = "user_collateral",
        docs = "Owner's collateral ledger (mint-scoped: seeds [collateral, owner, vault.collateral_mint], CR-3)",
        writable
    ))]
    #[codama(account(
        name = "vault",
        docs = "Per-collateral vault (aggregate updated + backing gate checked, §3.4/§4.2)",
        writable
    ))]
    #[codama(account(
        name = "vault_authority",
        docs = "Vault authority PDA (signs the withdrawal)"
    ))]
    #[codama(account(
        name = "vault_token_account",
        docs = "Vault token account (transfer source)",
        writable
    ))]
    #[codama(account(
        name = "user_token_account",
        docs = "Owner token account (transfer destination)",
        writable
    ))]
    #[codama(account(name = "token_program", docs = "SPL token program", default_value = program("token")))]
    Withdraw {
        /// Collateral base units to withdraw
        amount: u64,
    } = 12,

    /// Permissionless: advance the market's funding index.
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(name = "market", docs = "Market whose funding is updated", writable))]
    #[codama(account(
        name = "oracle",
        docs = "Pyth PriceUpdateV2 account bound to the market"
    ))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    UpdateFunding {} = 13,

    /// Permissionless: liquidate a position below maintenance margin.
    #[codama(account(
        name = "liquidator",
        docs = "Permissionless caller (paid the penalty)",
        signer
    ))]
    #[codama(account(
        name = "market",
        docs = "Market the position trades (writable: advances the braked mark + OI/social indices)",
        writable
    ))]
    #[codama(account(
        name = "oracle",
        docs = "Pyth PriceUpdateV2 account bound to the market"
    ))]
    #[codama(account(name = "position", docs = "Position being liquidated", writable))]
    #[codama(account(
        name = "user_collateral",
        docs = "Position owner's collateral ledger",
        writable
    ))]
    #[codama(account(
        name = "vault",
        docs = "Per-collateral vault (pass the mint-derived PDA)",
        writable
    ))]
    #[codama(account(
        name = "liquidator_collateral",
        docs = "Liquidator's collateral ledger (paid the penalty)",
        writable
    ))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    Liquidate {} = 14,

    /// Authority-gated escape hatch: abandon a wedged round and reopen `Collect`
    /// regardless of phase or unsettled orders (an operational backstop, not a
    /// normal path). Zeroes the histogram + ALL slab shards and bumps the auction id once.
    /// Stage A: ALL `num_slab_shards` shards must be passed as trailing writable accounts
    /// (after the histogram) so the reset is atomic — a partial reset would leave stale
    /// shards and desync shard auction ids. Bounded by the tx account limit.
    #[codama(account(name = "authority", docs = "Market authority / admin", signer))]
    #[codama(account(name = "market", docs = "Market to reset", writable))]
    #[codama(account(
        name = "histogram",
        docs = "AuctionHistogram to zero",
        writable,
        default_value = pda("auctionHistogramHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "order_slab",
        docs = "First OrderSlab shard to clear. ALL of the market's shards must follow as additional trailing writable accounts (shards [b\"order_slab\", market, shard_id.to_le_bytes()] for every shard_id in [0, num_slab_shards)); the processor requires shards.len() == num_slab_shards.",
        writable,
        default_value = pda("orderSlabHeader", [seed("market", account("market"))])
    ))]
    ForceReset {} = 15,

    /// Maker: create a persistent parametric quote PDA (parametric maker book).
    #[codama(account(
        name = "maker",
        docs = "Maker (pays rent, owns the quote)",
        signer,
        writable
    ))]
    #[codama(account(name = "market", docs = "Market the quote belongs to", writable))]
    #[codama(account(
        name = "maker_quote",
        docs = "MakerQuote PDA to create (client resolves [b\"maker_quote\", market, maker, quote_index] — §4.9 multi-quote seeds)",
        writable
    ))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    InitMakerQuote {
        /// Bump for the maker_quote PDA
        #[codama(default_value = account_bump("maker_quote"))]
        maker_quote_bump: u8,
        /// Slots of inactivity before the quote is skipped (0 = never)
        expiry_slots: u64,
        /// Optional delegate allowed to write the ladder (all-zero for none)
        delegate: [u8; 32],
        /// Which of the maker's concurrent quotes this is (4th PDA seed, §4.9)
        quote_index: u16,
    } = 16,

    /// Maker: re-anchor the ladder by moving its mid (the O(1) hot path).
    #[codama(account(name = "writer", docs = "Maker or its delegate", signer))]
    #[codama(account(name = "market", docs = "Market (supplies num_ticks)"))]
    #[codama(account(name = "maker_quote", docs = "MakerQuote to update", writable))]
    UpdateMakerQuoteMid {
        /// Monotonic nonce (must strictly increase)
        sequence: u64,
        /// New ladder anchor tick
        mid_tick: u32,
    } = 17,

    /// Maker: rewrite the full ladder (fixed-size padded level regions). The
    /// ladder's worst-case margin is delta-locked in the MAKER's ledger
    /// (quote-time margin, missing-features §7.1) — an unbacked ladder is
    /// rejected here, before it can ever fold and steer the clearing price.
    #[codama(account(name = "writer", docs = "Maker or its delegate", signer))]
    #[codama(account(name = "market", docs = "Market (supplies num_ticks)"))]
    #[codama(account(name = "maker_quote", docs = "MakerQuote to update", writable))]
    #[codama(account(
        name = "user_collateral",
        docs = "(Optional) the MAKER's mint-scoped collateral ledger (reservation delta-locked here); REQUIRED on a money-path market, omitted on clearing-only",
        writable,
        optional
    ))]
    UpdateMakerQuoteLevels {
        /// Monotonic nonce (must strictly increase)
        sequence: u64,
        /// New ladder anchor tick
        mid_tick: u32,
        /// Number of valid bid levels
        num_bids: u8,
        /// Number of valid ask levels
        num_asks: u8,
        /// Bid ladder: 8 × (u16 offset, u64 size), zero-padded
        bid_levels: [u8; 80],
        /// Ask ladder: 8 × (u16 offset, u64 size), zero-padded
        ask_levels: [u8; 80],
    } = 18,

    /// Maker: zero the ladder and deactivate the quote (releases the ladder's
    /// standing margin reservation back to the maker's ledger, §7.1).
    #[codama(account(name = "writer", docs = "Maker or its delegate", signer))]
    #[codama(account(name = "market", docs = "Market (decrements active count)", writable))]
    #[codama(account(name = "maker_quote", docs = "MakerQuote to clear", writable))]
    #[codama(account(
        name = "user_collateral",
        docs = "(Optional) the MAKER's ledger (the standing reservation is released here); required iff the quote carries a reservation",
        writable,
        optional
    ))]
    ClearMakerQuote {
        /// Monotonic nonce (must strictly increase)
        sequence: u64,
    } = 19,

    /// Permissionless crank: fold one active maker quote into the histogram.
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(name = "market", docs = "Market being accumulated", writable))]
    #[codama(account(
        name = "histogram",
        docs = "AuctionHistogram fold target",
        writable,
        default_value = pda("auctionHistogramHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(name = "maker_quote", docs = "MakerQuote to fold", writable))]
    ProcessMakerQuote {} = 20,

    /// Permissionless: settle one maker quote's fills into the maker's position.
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(name = "market", docs = "Market being settled", writable))]
    #[codama(account(
        name = "clearing_result",
        docs = "Published clearing result",
        default_value = pda("clearingResult", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "order_slab",
        docs = "OrderSlab (scanned for marginal-tick orders)",
        default_value = pda("orderSlabHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(name = "maker_quote", docs = "MakerQuote to settle", writable))]
    #[codama(account(name = "position", docs = "Maker's Position", writable))]
    #[codama(account(
        name = "user_collateral",
        docs = "(Optional) maker's collateral ledger",
        writable,
        optional
    ))]
    #[codama(account(
        name = "vault",
        docs = "(Optional) fee/insurance pool",
        writable,
        optional
    ))]
    SettleMakerQuote {} = 21,

    /// Create a cross-margin group for an owner.
    #[codama(account(name = "payer", docs = "Pays for the group account", signer, writable))]
    #[codama(account(name = "owner", docs = "Owner the group belongs to", signer))]
    #[codama(account(
        name = "margin_account",
        docs = "MarginAccount PDA to create",
        writable
    ))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    InitMarginAccount {
        /// Bump for the margin account PDA
        margin_bump: u8,
    } = 22,

    /// Bind a flat, owner-matched position into the cross-margin group.
    #[codama(account(name = "owner", docs = "Owner of both the group and position", signer))]
    #[codama(account(name = "margin_account", docs = "Group to extend", writable))]
    #[codama(account(
        name = "position",
        docs = "Flat position to bind (mode set to cross)",
        writable
    ))]
    #[codama(account(name = "market", docs = "The position's market"))]
    #[codama(account(
        name = "order_slab",
        docs = "OrderSlab scanned to reject an in-flight order",
        default_value = pda("orderSlabHeader", [seed("market", account("market"))])
    ))]
    AddPositionToMargin {} = 23,

    /// Cross-margin withdraw against the combined member health. One trailing entry
    /// per member follows the fixed accounts, in `live_mask` order: a *live* member is
    /// a `(position, market, oracle)` triple; a *flat* member (size 0) is a bare
    /// `position` account (no market/oracle — known-issues §2.4). Health is priced off
    /// each live leg's raw oracle, not the braked mark (§2.2).
    #[codama(account(name = "owner", docs = "Withdrawing owner", signer))]
    #[codama(account(name = "margin_account", docs = "Owner's group (member set)"))]
    #[codama(account(name = "user_collateral", docs = "Shared ledger to debit", writable))]
    #[codama(account(
        name = "vault",
        docs = "Per-collateral vault (aggregate updated + backing gate, §3.4/§4.2)",
        writable
    ))]
    #[codama(account(name = "vault_authority", docs = "Vault authority PDA (signs)"))]
    #[codama(account(
        name = "vault_token_account",
        docs = "Vault token account (source)",
        writable
    ))]
    #[codama(account(
        name = "user_token_account",
        docs = "Owner token account (dest)",
        writable
    ))]
    #[codama(account(name = "token_program", docs = "SPL token program", default_value = program("token")))]
    WithdrawCross {
        /// Collateral base units to withdraw
        amount: u64,
        /// Per-member shape bitmap: bit `i` set ⇒ member `i` is a (position, market,
        /// oracle) live triple; clear ⇒ a bare flat position account (§2.4)
        live_mask: u8,
    } = 24,

    /// Account-level liquidation — close one member of a combined-unhealthy
    /// group. One trailing entry per member follows, in `live_mask` order: a *live*
    /// member is a `(position, market, oracle)` triple, a *flat* member (size 0) is a
    /// bare `position` account (§2.4). The close target is the first non-flat member;
    /// solvency is priced off each live leg's raw oracle, not the braked mark (§2.2).
    #[codama(account(
        name = "liquidator",
        docs = "Permissionless caller (paid the penalty)",
        signer
    ))]
    #[codama(account(name = "margin_account", docs = "Owner's group"))]
    #[codama(account(name = "user_collateral", docs = "Owner's shared ledger", writable))]
    #[codama(account(name = "vault", docs = "Per-collateral vault", writable))]
    #[codama(account(name = "liquidator_collateral", docs = "Liquidator's ledger", writable))]
    #[codama(account(name = "event_authority", docs = "Event authority PDA", signer = false))]
    #[codama(account(name = "tempo_program", docs = "Tempo program (self-CPI)"))]
    LiquidateCross {
        /// Per-member shape bitmap: bit `i` set ⇒ member `i` is a (position, market,
        /// oracle) live triple; clear ⇒ a bare flat position account (§2.4)
        live_mask: u8,
    } = 25,

    /// Migrate a VERSION-4 `Market` account in place to the VERSION-5 layout
    /// (admin-gated): grows the account, zero-inits the appended risk block, and
    /// sets the two brake/soft-stale config values.
    #[codama(account(name = "authority", docs = "Market authority (admin)", signer))]
    #[codama(account(name = "market", docs = "Market account to upgrade", writable))]
    #[codama(account(
        name = "payer",
        docs = "Funds the grown-account rent",
        signer,
        writable
    ))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    MigrateMarket {
        /// Meltdown-brake cap, bps per slot (0 = disabled)
        max_price_move_bps_per_slot: u16,
        /// Soft-stale window, slots (0 = disabled)
        soft_stale_slots: u64,
    } = 26,

    /// Migrate a VERSION-1 `Position` account in place to the VERSION-2 layout
    /// (owner-gated): appends `last_social_index` and rebuilds the market's open
    /// interest by adding this position's size back.
    #[codama(account(name = "owner", docs = "Position owner (pays rent)", signer, writable))]
    #[codama(account(name = "position", docs = "Position account to upgrade", writable))]
    #[codama(account(
        name = "market",
        docs = "The position's v5 market (OI rebuilt)",
        writable
    ))]
    #[codama(account(
        name = "order_slab",
        docs = "OrderSlab; must be settled (quiescence gate for the OI rebuild)",
        default_value = pda("orderSlabHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    MigratePosition {} = 27,

    /// Unbind a flat, owner-matched member position from the cross-margin group,
    /// returning it to isolated mode and freeing its slot.
    #[codama(account(name = "owner", docs = "Owner of both the group and position", signer))]
    #[codama(account(name = "margin_account", docs = "Group to shrink", writable))]
    #[codama(account(
        name = "position",
        docs = "Flat member position to unbind (mode set to isolated)",
        writable
    ))]
    RemovePositionFromMargin {} = 28,

    /// Maker: close a cleared (inactive) quote PDA and reclaim its rent, freeing
    /// the deterministic address so the maker can re-`init_maker_quote`.
    #[codama(account(
        name = "maker",
        docs = "Quote maker; receives reclaimed rent",
        signer,
        writable
    ))]
    #[codama(account(
        name = "maker_quote",
        docs = "Inactive MakerQuote PDA to close",
        writable
    ))]
    CloseMakerQuote {} = 29,

    /// Stage A sharding: create one OrderSlab shard `[b"order_slab", market, shard_id]`.
    #[codama(account(name = "payer", docs = "Pays for the shard account", signer, writable))]
    #[codama(account(name = "market", docs = "Market the shard belongs to"))]
    #[codama(account(
        name = "order_slab",
        docs = "OrderSlab shard PDA to create (client resolves [b\"order_slab\", market, shard_id])",
        writable
    ))]
    #[codama(account(name = "system_program", docs = "System program", default_value = program("system")))]
    InitShard {
        /// Index of the shard to create (`[0, num_slab_shards)`)
        shard_id: u16,
        /// Bump for the shard's OrderSlab PDA
        #[codama(default_value = account_bump("order_slab"))]
        bump: u8,
    } = 30,

    /// Stage A sharding: reset one drained shard for the next round (permissionless).
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(name = "market", docs = "Market (increments shards_ready)", writable))]
    #[codama(account(
        name = "order_slab",
        docs = "Drained OrderSlab shard to zero for the next round",
        writable
    ))]
    ResetShard {} = 31,

    /// Authority circuit breaker (missing-features §3.2): set the market's pause
    /// bitflags. Bit 0 = pause intake (submits + maker-quote writes), bit 1 =
    /// pause the roll (market winds down quiescent). Cancels, cranks, settles,
    /// withdrawals, and liquidations are NEVER paused — a pause can't trap funds.
    #[codama(account(name = "authority", docs = "Market authority", signer))]
    #[codama(account(name = "market", docs = "Market to pause/resume", writable))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    SetPause {
        /// New pause bitflags (0 = fully resumed; unknown bits rejected)
        paused: u8,
    } = 32,

    /// Authority: update the HOT market parameter set (plan.md §3.2) — every
    /// field is read at use-time, so a change applies from the next operation
    /// and can never strand in-flight state. Risk-class params go through the
    /// staged propose/apply path; structural params are never changeable.
    #[codama(account(name = "authority", docs = "Market authority", signer))]
    #[codama(account(name = "market", docs = "Market to retune", writable))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    UpdateMarketParams {
        /// Maker fee, signed bps (negative = rebate; |x| ≤ 1000)
        maker_fee_bps: i16,
        /// Taker fee, signed bps (negative = rebate; |x| ≤ 1000)
        taker_fee_bps: i16,
        /// Integrator share of positive fees, bps (≤ 10_000)
        integrator_share_bps: u16,
        /// Flat finalize-clear crank fee
        crank_fee: u64,
        /// Per-slot effective-price move cap, bps (0 = brake off)
        max_price_move_bps_per_slot: u16,
        /// Soft-stale oracle window, slots (0 = disabled)
        soft_stale_slots: u64,
        /// Per-position notional cap (0 = disabled)
        max_position_notional: u128,
        /// Minimum order notional (0 = disabled)
        min_order_notional: u64,
        /// Per-side open-interest soft cap (0 = disabled)
        max_open_interest: u128,
        /// Flat liquidator reward floor from insurance (0 = disabled)
        liquidation_reward_floor: u64,
    } = 33,

    /// Authority: stage a risk-class change (margins/penalty/close buffer)
    /// behind the consensus-enforced delay — raising maintenance can make live
    /// positions liquidatable, so users get the window to de-risk (§3.2).
    #[codama(account(name = "authority", docs = "Market authority", signer))]
    #[codama(account(name = "market", docs = "Market (staging slot written)", writable))]
    ProposeRiskUpdate {
        /// New maintenance margin, bps (validated with the init bounds)
        maintenance_margin_bps: u16,
        /// New initial margin, bps (≥ maintenance)
        initial_margin_bps: u16,
        /// New liquidation penalty, bps
        liquidation_penalty_bps: u16,
        /// New partial-liquidation close buffer, bps
        liquidation_close_buffer_bps: u16,
    } = 34,

    /// Permissionless: apply a staged risk update once its delay elapses (the
    /// crank philosophy — even admin changes complete permissionlessly).
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(name = "market", docs = "Market to apply onto", writable))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    ApplyRiskUpdate {} = 35,

    /// Authority: stage an authority transfer. Two-step — the NEW key must sign
    /// the accept, so a typo'd dead address can never take over (§3.3).
    #[codama(account(name = "authority", docs = "Current market authority", signer))]
    #[codama(account(name = "market", docs = "Market (staging slot written)", writable))]
    ProposeAuthorityTransfer {
        /// The proposed new authority
        new_authority: [u8; 32],
    } = 36,

    /// The staged NEW authority signs to take over the market.
    #[codama(account(name = "new_authority", docs = "The staged new authority", signer))]
    #[codama(account(name = "market", docs = "Market whose authority rotates", writable))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    AcceptAuthorityTransfer {} = 37,

    /// Authority: stage an oracle repoint (the most dangerous admin power —
    /// whoever controls the oracle controls liquidation prices). Only
    /// proposable while the market is winding down (`PAUSE_ROLL`), and applies
    /// only after the delay, on a fully paused + quiescent market (§3.3).
    #[codama(account(name = "authority", docs = "Market authority", signer))]
    #[codama(account(name = "market", docs = "Market (staging slot written)", writable))]
    ProposeSetOracle {
        /// The proposed new Pyth PriceUpdateV2 account
        new_oracle: [u8; 32],
        /// The feed id that account must carry
        new_feed_id: [u8; 32],
    } = 38,

    /// Permissionless: apply a staged oracle repoint. Gates: delay elapsed;
    /// market fully paused AND quiescent (round settled, all shards reset); the
    /// staged account is live, fresh, and confidence-checked RIGHT NOW; address
    /// + feed id commit atomically. The window re-anchors at the next roll.
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(name = "market", docs = "Market to repoint", writable))]
    #[codama(account(
        name = "new_oracle",
        docs = "The STAGED Pyth account (validated live before commit)"
    ))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    ApplySetOracle {} = 39,

    /// Permissionless donation into the vault insurance pool (missing-features
    /// §4.1). Exists so a fresh money-path market's pool is not zero — an empty
    /// pool deadlocks the first profitable maker settle (`InsuranceInsolvent`;
    /// reproduced on devnet, plan.md P0.6). Conserving by construction.
    #[codama(account(name = "donor", docs = "Anyone (signs the token transfer)", signer))]
    #[codama(account(name = "vault", docs = "Vault (insurance bookkeeping)", writable))]
    #[codama(account(
        name = "vault_token_account",
        docs = "Vault SPL token account",
        writable
    ))]
    #[codama(account(
        name = "donor_token_account",
        docs = "Donor SPL token account (same mint)",
        writable
    ))]
    #[codama(account(name = "token_program", docs = "SPL token program", default_value = program("token")))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    SeedInsurance {
        /// Tokens to donate into the insurance pool (must be non-zero)
        amount: u64,
    } = 40,

    /// Vault authority: stage an insurance withdrawal behind the consensus
    /// delay (plan.md §4.4) — the only authority-controlled token OUTFLOW in
    /// the program; users get the delay window to exit before it can land.
    #[codama(account(
        name = "authority",
        docs = "Vault authority (recorded at init)",
        signer
    ))]
    #[codama(account(name = "vault", docs = "Vault (staging slot written)", writable))]
    ProposeInsuranceWithdraw {
        /// Tokens to withdraw from the pool (bounded by the pool at propose,
        /// re-clamped at apply)
        amount: u64,
    } = 41,

    /// Permissionless: apply a staged insurance withdrawal once the delay
    /// elapses. Re-clamps to the current pool, then the §4.2 FAIL-CLOSED
    /// backing gate runs post-debit, pre-transfer — tokens may only leave while
    /// the vault still covers every user balance + the remaining pool.
    #[codama(account(name = "cranker", docs = "Permissionless caller", signer))]
    #[codama(account(
        name = "vault",
        docs = "Vault (pool debited, staging cleared)",
        writable
    ))]
    #[codama(account(name = "vault_authority", docs = "Vault authority PDA (signs)"))]
    #[codama(account(
        name = "vault_token_account",
        docs = "Vault token account (source)",
        writable
    ))]
    #[codama(account(
        name = "recipient_token_account",
        docs = "Recipient token account (same mint, HS-12)",
        writable
    ))]
    #[codama(account(name = "token_program", docs = "SPL token program", default_value = program("token")))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    ApplyInsuranceWithdraw {} = 42,

    /// Cancel EVERY still-resting order the signer owns in one shard, in one
    /// transaction (missing-features §2.7 — the market maker's "flatten now"
    /// button). Owner-path only (no reaper branch: reaping strangers' expired
    /// orders stays on CancelOrder); the freed worst-case margin reservations
    /// are released as ONE summed credit; one OrderCancelled event per order;
    /// zero matches is a no-op success. Multi-shard cancel-all is a client
    /// loop over shards.
    #[codama(account(
        name = "trader",
        docs = "Order owner (only their orders cancel)",
        signer
    ))]
    #[codama(account(
        name = "market",
        docs = "Market the orders belong to. Read-only (Design Z): cancel writes only its own shard."
    ))]
    #[codama(account(
        name = "order_slab",
        docs = "The one OrderSlab shard to scan (multi-shard = one tx per shard)",
        writable,
        default_value = pda("orderSlabHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "event_authority",
        docs = "Event authority PDA for CPI event emission",
        signer = false
    ))]
    #[codama(account(
        name = "tempo_program",
        docs = "Tempo program, for self-CPI event emission"
    ))]
    #[codama(account(
        name = "user_collateral",
        docs = "(Optional) trader's collateral ledger; releases the summed reserved margin",
        writable,
        optional
    ))]
    CancelAllOrders {} = 43,

    /// Close a FLAT, fully drained Position PDA and refund its rent to the
    /// owner (missing-features §3.4). Rejected unless size == 0, collateral == 0,
    /// realized_pnl == 0, and the position is isolated (a cross-group member
    /// must RemovePositionFromMargin first).
    #[codama(account(
        name = "owner",
        docs = "Position owner; receives the reclaimed rent",
        signer,
        writable
    ))]
    #[codama(account(name = "position", docs = "The flat Position PDA to close", writable))]
    ClosePosition {} = 44,

    /// Wind down a fully QUIESCENT market: close every shard, the histogram,
    /// the clearing result, and the market itself, refunding all rent to the
    /// authority (missing-features §3.4). Gated on: fully paused, post-clearing
    /// phase with every shard reset, zero open interest both sides, zero active
    /// maker quotes, and every shard empty — else MarketNotQuiescent. Takes ALL
    /// shards as trailing accounts (force_reset-style count + dedup).
    #[codama(account(
        name = "authority",
        docs = "Market authority; receives all reclaimed rent",
        signer,
        writable
    ))]
    #[codama(account(name = "market", docs = "Market to close (closed last)", writable))]
    #[codama(account(
        name = "histogram",
        docs = "The market's AuctionHistogram (closed)",
        writable,
        default_value = pda("auctionHistogramHeader", [seed("market", account("market"))])
    ))]
    #[codama(account(
        name = "clearing_result",
        docs = "The market's ClearingResult (closed)",
        writable
    ))]
    CloseMarket {} = 45,

    /// Invoked via CPI to emit event data in instruction args (prevents log truncation).
    #[codama(skip)]
    #[codama(account(name = "event_authority", signer))]
    EmitEvent {} = 228,
}
