# Tempo — Known Issues & Missing Features: Analysis and High-Level Proposals

**Date:** 2026-07-07
**Context:** Based on deep read of `research.md`, `docs/known-issues.md`, `docs/missing-features.md`, and full source code in `program/src/`

---

## Executive Summary

The core of Tempo is in excellent shape. The clearing engine (histogram folding, `find_cross`, the telescoping-floor marginal fill, per-shard completeness scans) is correct, well-tested, fuzz-guarded, and partially Kani-proved. Almost every real money-path bug found in earlier reviews is already fixed and recorded in `known-issues.md` Part B. What remains open in `known-issues.md` Part A is not bugs: it is two coverage/operational gaps (2.12, 2.13), three deliberately deferred design items gated on benchmarks (2.14, 2.15, 4.9), and two documented, accepted limitations (2.11, 4.10). This is a healthy state for a codebase of this ambition.

The real work now lives in `missing-features.md`. The single most important open item, from a safety point of view, is **§7.1 — makers can post unbacked quote ladders**. This is the only remaining path where an attacker can move the clearing price for everyone with zero capital and push a shortfall onto the insurance pool. Everything needed to close it already exists in the codebase (the reserve-worst-case-then-release pattern from `submit_order`), so it is high value at moderate cost. The second theme is **operability**: the program is an engine, not yet an exchange — there is no pause switch, no way to retune a live market's risk parameters, no way to repoint a dying oracle, and protocol fees are economically trapped inside the insurance counter. These are mostly small, authority-gated instructions that share one design pattern and can be built as one batch.

A final note: two passages in `missing-features.md` are now **stale relative to the code** — §1.1/§2.2 still describe the old reduce-only "headroom discount" (the code now reserves the *full* worst-case margin, per DDR-3 Correction-2, confirmed at `submit_order/processor.rs:199-206`), and §1.3 says the order cap bound is 128 while the code enforces `MAX_ORDERS_PER_AUCTION_CAP = 90`. These doc fixes are free and should be done first so future work is not designed against wrong assumptions.

---

## Known Issues — Detailed Analysis & Proposals

*(Part B of `known-issues.md` is resolved history; only the seven Part-A items are analyzed here.)*

### Issue 1: §2.11 — Ledger required to release a reservation on cancel/zero-fill settle

**Severity:** Low

**Root Cause:** This is correct behavior that was described slightly wrong in an earlier version of the doc. The code gates the ledger requirement on `release_amount > 0` (`settle_fill/processor.rs:294-298`), not on `reserved_margin > 0`. A zero-fill order that simply re-rests releases nothing, so it needs no ledger account. Verified against the code: the gating and the comment block above it are exact.

**Impact:** None on-chain. The only risk is a stale client omitting the ledger on a fully-consuming settle and getting `MissingSettleAccounts` — a clean, retryable error, never a wedge. A permissionless cranker can always derive the ledger PDA from `order.trader` + the market's mint.

**High-Level Solution:** Documentation + SDK hygiene only:
1. Update the client/docs wording as the issue itself says: "the ledger is required whenever a settle *releases* a reservation (a fully-consuming settle), not on every zero-fill settle."
2. In the SDK's settle-ix builder, **always attach** the derived `user_collateral` PDA on money-path markets. It is a deterministic PDA, costs one account slot, and removes the whole class of "did I need it this time?" client logic. The keeper already learned this lesson the hard way with §2.16 (attaching a *nonexistent* position); attaching an *existing, derivable* ledger is the safe direction.

**Implementation Notes:** No program change. One doc edit, one SDK builder change, one keeper config check.

**Trade-offs:** Always attaching the ledger costs one account (32 bytes) per settle tx even when unused. Negligible.

**Recommended Priority & Phase:** Low — Phase 0 (doc/ops batch).

---

### Issue 2: §2.12 — Devnet client bundle drift after the sharding layout bump

**Severity:** High (operational blocker), trivial effort

**Root Cause:** `Market` moved to VERSION 11 and `InitializeMarketData::LEN` to 131 bytes across the Stage-A/Design-Z merges. Generated clients (`clients/typescript`, `crates/sdk`) were regenerated, but `apps/web/src/vendor/tempo-client.mjs` is a *vendored bundle* that predates the merge. This is the classic failure mode of vendored artifacts: they don't participate in the "clients-fresh" CI check.

**Impact:** A stale bundle under-encodes `initialize_market` (missing `num_slab_shards`) → the program rejects with "invalid instruction data"; or it decodes `Market` state at wrong offsets → silently wrong UI numbers. The program itself is safe (the version byte fails loudly on old *accounts*), but the wire format has no version byte, so instruction-data drift is only caught by length checks.

**High-Level Solution:**
1. Immediately: `pnpm generate-clients && pnpm bundle-client`, verify the bundle contains the sharding fields, re-provision devnet markets on the v11 layout.
2. Durable fix: extend the existing CI "clients-fresh" job to **also rebuild and diff the web vendor bundle**, so a vendored artifact can never silently lag the IDL again. This is the systemic fix; the regen alone will drift again on the next layout bump.

**Implementation Notes:** No program change. One CI workflow edit (`.github/workflows/`), one regen, one devnet re-provision.

**Trade-offs:** None meaningful.

**Recommended Priority & Phase:** High — Phase 0, do first (it blocks any devnet money-path validation, including Issue 3 below).

---

### Issue 3: §2.13 — Stage-B marketable-fill only unit-tested, never validated end-to-end

**Severity:** High (coverage gap on a subtle money path)

**Root Cause:** DDR-3's marketable/passive split has two halves. The *park* half (passive orders skip the fold, don't block finalize, return when the window returns) is covered end-to-end in LiteSVM. The *fill* half — a resting order whose fixed price the recentered window moved **through**, folding at the boundary tick and actually executing against a live counterparty — is proven only at unit level (`classify_resting_fold` tick correctness). The full chain "recenter → fold-at-boundary → cross → settle at clearing price → margin/position correct" has never run as one test.

**Impact:** This path touches three delicate mechanisms at once: the boundary-tick fold (the order fills at a *better* price than its limit), the fixed `worst_price` margin snapshot (the reservation was priced against the old window), and the re-arm/consume decision at settle. A bug here would be a real money bug (wrong fill price or wrong margin release), and it would only appear on volatile days when the window moves — exactly when you least want surprises.

**High-Level Solution:** Two tests, no program change expected:
1. **LiteSVM integration test** (`tests/integration-tests/`): create a money-path market; place a resting sell at price P inside the window; write a synthetic Pyth account (the harness already builds `PriceUpdateV2` bytes for oracle tests) whose price moves the window fully above P; roll the round (`start_auction` recenters); submit a live buy; crank the full round; assert (a) the sell folded at tick 0 (boundary), (b) it filled at the clearing price ≥ its limit, (c) `Σ buy fills == Σ sell fills`, (d) the margin released equals the reservation minus the leftover requirement, (e) position VWAP/realized PnL are exact. Mirror the test for a buy above the window top.
2. **Devnet scenario script** in `crates/sim`: the same sequence against the live deployed binary, since LiteSVM cannot catch CU or account-limit surprises.

**Implementation Notes:** Pure test work. If the test *fails*, the most likely suspects (from reading the code) are the interaction between `classify_resting_fold`'s boundary tick and `settle_fill`'s tick recovery — both call the same function with the same inputs, so drift is unlikely, but that symmetry is exactly what the test should pin.

**Trade-offs:** None. This is the cheapest insurance in the whole backlog.

**Recommended Priority & Phase:** High — Phase 0/1, immediately after Issue 2 unblocks devnet.

---

### Issue 4: §2.14 — Stage-C2 (true round-processing overlap) not built

**Severity:** Medium (deliberate deferral, correctly gated)

**Root Cause:** One `AuctionHistogram` + one `ClearingResult` + one phase machine per market. Round N+1 cannot *accumulate* while round N *settles*. This is not an oversight — C2 ("double-buffer by round parity") was explicitly deferred because it runs **two live rounds over one durable book**, the riskiest change in the scaling plan, and the plan gates it behind a benchmark that has not been run.

**Impact:** Round latency = settle time + accumulate time, serialized. With Stage A parallel settle and C1 always-open submit, the measured cost (`cu_report.md`: 160k CU finalize at 16×90) suggests the serial window is small. Whether it *matters* is exactly the unanswered benchmark question.

**High-Level Solution (if the benchmark says build it):**
- **Accounts:** two histograms per market, seeds `[b"histogram", market, parity]` where `parity = auction_id % 2` as a 1-byte seed; same for `ClearingResult` (`[b"clearing", market, parity]`). Both are already fixed-size and idempotently created, so the doubling is mechanical. All fields stay little-endian `[u8; N]`, alignment-1.
- **Phase machine:** `Market` carries two small per-round phase slots instead of one global `phase` (e.g. `phase_a: u8`, `phase_b: u8`, with `current_auction_id` selecting which is "front"). This is an append + re-interpretation → `Market` VERSION bump, re-provision.
- **The book:** this is the hard part. `arm_auction_id` (Stage C1) already tags every order with its round, and `cum_before` is per-fold — so an order is unambiguous about *which* histogram it folds into. The completeness scan needs a round parameter (it already takes `current_auction_id`). `reset_shard`'s "free Consumed slots, keep Resting" survives, but the exactly-once auction-id tag on the shard header must become parity-aware.
- **Safety patterns preserved:** commutativity is per-histogram and unaffected; completeness stays a scan (now scoped by round); settlement never-reverts is unaffected. The new invariant to prove: *an order can never fold into both parities* — `arm_auction_id` + the per-order `Accumulated` status flag give this almost for free, but it needs a dedicated adversarial test (a hostile cranker interleaving round-N settles with round-N+1 folds).

**Implementation Notes:** Do **not** start with code. The "to close" step is the benchmark: measure end-to-end round time on devnet with C1 + Stage A under a realistic order load, and compare the serial settle window against the collect window. If settle fits inside the natural collect window, C2 buys nothing.

**Trade-offs:** C2 roughly doubles the audit surface of the lifecycle. The failure mode of *not* building it is latency; the failure mode of building it wrong is cross-round contamination of the book. That asymmetry justifies the deferral.

**Recommended Priority & Phase:** Medium — Phase 5 (benchmark first; build only on evidence).

---

### Issue 5: §2.15 — Keeper does not open the next `Collect` early

**Severity:** Low

**Root Cause:** Pure off-chain scheduling. The on-chain program already allows submission in any phase (C1), and `start_auction` is permissionless; the keeper just calls it late (after full settlement) rather than optimistically scheduling the roll the moment `shards_ready` completes.

**Impact:** A slightly longer gap between rounds than necessary. No correctness impact whatsoever.

**High-Level Solution:** In `crates/keeper`'s `engine::decide`, treat `shards_ready == num_slab_shards` as an immediate roll trigger and pipeline `reset_shard` calls concurrently with the tail of `settle_fill` cranking (they are gated per shard, so a shard whose orders are all settled can reset while another shard still settles).

**Implementation Notes:** Keeper-only. Measure with the same benchmark as Issue 4 — they answer the same question ("where does round latency actually go?").

**Trade-offs:** None.

**Recommended Priority & Phase:** Low — Phase 5, paired with the C2 benchmark.

---

### Issue 6: §4.9 — One `MakerQuote` PDA per maker (single-ladder limit)

**Severity:** Medium

**Root Cause:** PDA seeds are `[b"maker_quote", market, maker]` — the address space allows exactly one quote per (market, maker). A maker cannot run two ladders, and once its single quote folds mid-round it cannot post fresh liquidity until the next round.

**Impact:** Caps posted maker depth per identity; makers can trivially work around it with multiple keypairs, which is worse for everyone (fragmented collateral, more accounts). No safety impact — purely a liquidity/UX ceiling.

**High-Level Solution:** Add a `quote_index: u16` to the seed set: `[b"maker_quote", market, maker, quote_index_le]` (little-endian 2 bytes, consistent with the shard-id seed pattern in `OrderSlab`). Details:
- `init_maker_quote` data gains a `quote_index` field; a per-market cap (`max_quotes_per_maker`, e.g. 4) bounds it. The market's `active_maker_quote_count` already counts *quotes*, not makers, so the completeness gate (`folded == active`) needs **zero changes** — this is the beauty of the counter being quote-scoped already.
- `quote_id` (the marginal-tick tie-break) is already claimed from `market.next_quote_id` per quote, so multiple quotes per maker tie-break deterministically with no change.
- The struct layout itself doesn't change → no `MakerQuote` VERSION bump strictly required, but the **address** of every existing quote changes. Migration is clean because `close_maker_quote` already exists: makers `clear` + `close` old quotes (rent refunded) and re-init at the new addresses. Update `crates/sdk/src/pda.rs` and the mm-bot.
- `settle_maker_quote` margin note: when §7.1 (quote-time margin) lands, the reservation must be per-quote, which this design already gives naturally.

**Implementation Notes:** Breaking IDL change (new data field + new seed) — batch it with the §7.1 program change so makers re-provision once, not twice. New error not needed (`MarketConfigOutOfRange` covers the index bound).

**Trade-offs:** More quotes per maker = more `process_maker_quote` cranks per round (one per active quote). Bounded by the cap; each fold is ≤ 16 bucket adds, cheap.

**Recommended Priority & Phase:** Medium — Phase 4, **bundled with §7.1's** maker-quote layout bump.

---

### Issue 7: §4.10 — Off-chain `benign()` classifier uses string matching

**Severity:** Low-Medium (off-chain robustness)

**Root Cause:** `crates/sdk/src/retry.rs` classifies crank-race errors by substring-matching the RPC error *display string*. The program already surfaces stable numeric codes (`ProgramError::Custom(n)`, 47 stable variants) — the SDK just doesn't parse them structurally.

**Impact:** If an RPC provider reformats error text, benign races (e.g. "someone settled this order first") could be misclassified as real failures (alert spam) or — worse — real failures classified benign (silent swallowing, the exact bug fixed in §4.3). A regression test currently pins the format, which contains but does not cure the fragility.

**High-Level Solution:** Parse the **structured** transaction error, not the string. Solana RPC returns `TransactionError::InstructionError(idx, InstructionError::Custom(code))` as typed JSON; map `code` through a generated `TempoProgramError` table (Codama already emits the error enum into the IDL and generated clients, so the numeric mapping exists in `crates/sdk/src/generated`). Keep the string matcher only as a last-resort fallback for transport-level errors that genuinely have no code (blockhash expiry, node behind), which is a small closed set.

**Implementation Notes:** SDK-only. The benign set becomes an explicit allowlist of codes: `OrderNotFound` (settled first), `InvalidOrderStatus` (raced), `AuctionWrongPhase`/`AuctionIdMismatch` (phase raced), `AuctionNotComplete` (crank raced), `NotLiquidatable` (liquidation raced). That list is *self-documenting* in a way substrings never are.

**Trade-offs:** None; strictly better. Keep the existing format-drift regression test for the fallback path.

**Recommended Priority & Phase:** Medium-Low — Phase 5 (any time; good "small task" filler).

---

## Missing Features — Detailed Analysis & Proposals

*(Items marked DONE in `missing-features.md` are skipped except where the doc text has drifted from the code.)*

### Feature 0 (doc drift, found during this analysis): stale reduce-only and cap text

**User Value:** Correct docs prevent future features being designed against wrong assumptions.

**Finding:** `missing-features.md` §1.1 and §2.2 say a reduce-only order "reserves only the portion that would open new exposure." The code no longer does this: `submit_order/processor.rs:199-206` deliberately discards the same-side headroom (`let _ = already_same_side;`) and reserves the **full worst-case margin**, per DDR-3 Correction-2 (also recorded in `known-issues.md` §2.10's resolution). Additionally §1.3 states `orders_per_auction_cap ∉ (0, 128]` but the code enforces `MAX_ORDERS_PER_AUCTION_CAP = 90` (the 10,240-byte CreateAccount ceiling at `ORDER_LEN = 112`).

**Recommended Priority & Phase:** High (it's free) — Phase 0. Edit the two sections; note that reduce-only's only remaining job is forcing `Consumed` at settle.

---

### Feature 1: §1.2 (remainder) — Max open-interest cap

**User Value:** Bounds the protocol's total risk per market so a single market cannot grow past what the insurance pool and ADL can plausibly absorb.

**Dependencies:** `Market.oi_long`/`oi_short` (already tracked, u128); the submit path.

**High-Level Design:** There is a real design tension here, and it should be resolved *in favor of parallel intake*:
- An **airtight** cap needs OI-headroom *reservation* at submit (mirroring margin reservation). But OI is a **global aggregate on `Market`** — reserving it at submit means `submit_order` writes `Market`, which un-does Design Z's parallel-intake win (the whole point of removing Market counters in v9/v11). That price is too high.
- Propose a **soft cap**: add `max_open_interest: u128` (little-endian `[u8;16]`, appended to `Market`, version bump) checked at **submit time, read-only**: reject if `current side OI + order qty > cap` (`PositionLimitExceeded` or a new `OpenInterestCapExceeded` error). Races within one round can overshoot, but the overshoot is strictly bounded: ≤ (orders per round) × (per-order max), and each order is *individually* bounded by `max_position_notional` and its margin reservation. So the cap is approximate at round granularity but every unit of OI is still fully margined — the cap is a risk-*sizing* tool, not a solvency tool (solvency is already carried by margin).
- Belt-and-suspenders: `start_auction` can log/emit when OI exceeds the cap so operators see sustained breaches.

**Implementation Notes:** One Market field + one submit check + one `initialize_market` data field (breaking IDL, batch with other Market-layout changes). Add to the future `update_market_params` hot-set (§3.1) so it can be tightened live. Test: a race test in LiteSVM (two submits in one round both under the cap individually, over it together → both accepted, next round's submits rejected).

**Trade-offs:** Soft vs. airtight, chosen deliberately. Document the bound on overshoot.

**Recommended Priority & Phase:** Medium — Phase 3, batched into a single `Market` version bump with other fields.

---

### Feature 2: §2.1 — No close / reduce-position instruction

**User Value:** A trader must be able to exit. Today the only exit is an opposing order into the next auction; if the book is one-sided, they are stuck holding risk they want to shed.

**Dependencies:** Order types (§2.3) — this is really the same feature; the auction *is* the execution venue.

**High-Level Design:** Be honest about what a batch auction can and cannot promise: there is **no instant fill** in DFBA by design, and "close against the protocol at oracle price" is a disguised free option against the vault (a trader closes exactly when the oracle lags — this would be a new insurance-drain vector and must be rejected). The right shape is a **market-close order**:
- `close_position` = a thin convenience wrapper that submits a **reduce-only, marketable-priced** order: a long closes with a sell priced at the window floor (tick 0), a short with a buy at the window top — i.e. "fill me at whatever the auction clears at." Reduce-only already forces `Consumed` at settle (never rests into a worse window), and the full worst-case reservation already covers it.
- Sizing: `qty = min(requested, |position.size|)` read at submit. Because position size can change between submit and settle, reduce-only's existing semantics (apply full fill, force-consume) remain the conservation-safe behavior — no settle-time clamp, as DDR-3 already established.
- The remaining true gap — "the book is one-sided, nobody will take the other side" — is a *liquidity* problem, not an instruction problem. The mm-bot + maker-quote book is the design's answer; §7.1 (backed quotes) makes that answer trustworthy.

**Implementation Notes:** Can ship as SDK-level sugar first (compose `submit_order` with side/price/reduce_only derived from the position) with **zero program change** — the program already supports every ingredient. A dedicated instruction adds only atomicity of the "read size, submit" pair; do it later if the race proves annoying.

**Trade-offs:** A market-priced close accepts the clearing price, whatever it is; the oracle-anchored window bounds how bad that can be (one window width). Document this bound to users.

**Recommended Priority & Phase:** High (user-facing) — Phase 4 for SDK sugar (cheap, immediate), program instruction only if needed.

---

### Feature 3: §2.3 (remainder) — Order types: market / IOC / FOK / post-only

**User Value:** Standard trading semantics users expect.

**Dependencies:** Stage B/C1 fields (`expires_at_auction`, `arm_auction_id`) — already shipped.

**High-Level Design:** Map each type onto the batch model rather than imitating a CLOB:
- **Market**: a limit at the window boundary tick (buy → top tick, sell → floor). Already expressible today; make it a first-class flag or SDK helper (see Feature 2). Margin reservation at the boundary price is exactly the existing worst-case rule — no new math.
- **IOC** ("this round only"): *almost* expressible via `expires_at_auction`, but not quite — the submit guard rejects `expires <= current auction id` (`OrderAlreadyExpired`), so the minimum lifetime today is fill-this-round *plus* rest-one-round. Fix: allow `expires_at_auction == arm_auction_id` (change the submit check from `<= auction_id` to `< arm_auction_id`). Then an order with `expires = arm round` participates in exactly one auction and is consumed at that round's settle (settle's own `expires <= auction_id` check already produces this). One-line semantic change + tests; the cancel-reaper's strict `<` boundary is unaffected.
- **FOK**: **do not build.** All-or-nothing at the marginal tick would require conditioning one order's fill on the whole rationing outcome, i.e. removing quantity from the histogram *after* discovery — this breaks the telescoping-floor conservation and the order-independence of settlement, two properties the whole security model rests on. Document it as fundamentally incompatible with uniform-price batch rationing.
- **Post-only**: doesn't map — slab orders are taker-only by construction (§1.3), and "maker" in Tempo means a `MakerQuote` ladder, which is inherently post-only. Document: "post-only = use the maker-quote book."

**Implementation Notes:** IOC = tiny program change (one comparison) + IDL docs. Market = SDK helper. New error codes: none. Tests: IOC order participates once and consumes; IOC + partial fill consumes remainder (no rest).

**Trade-offs:** IOC orders that miss the cross waste a round-trip; that's inherent to batching.

**Recommended Priority & Phase:** Medium-High — Phase 4 (IOC + market together; small).

---

### Feature 4: §2.6 — Minimum order size / notional

**User Value:** Blocks dust flooding: today `quantity != 0` is the only size check, so an attacker can fill shards with 1-lot orders, burning crank CU and squeezing the 90-slot shards (partially mitigated by the 8-per-trader-per-shard cap and margin locks, but a money-free benchmark market has no margin lock at all).

**Dependencies:** None.

**High-Level Design:** Add `min_order_notional: u64` to `Market` (append, LE bytes, version bump). Check at submit: `quantity × price ≥ min_order_notional` (u128 intermediate; ceil-free, plain comparison). Notional (not raw qty) is the right unit — it stays meaningful as price moves and matches how `max_position_notional` is expressed. Reject with the existing reserved `OrderBelowMinimum` error (code 29 — it already exists, currently unused; wiring it up also closes a dangling error variant). Same check in `update_maker_quote_levels` per level (a maker ladder of dust levels is the same attack at fold time).

**Implementation Notes:** One field, two checks, one `initialize_market` data field. Batch into the same Market version bump as Feature 1. Include in the §3.1 hot-set so it can be tuned live.

**Trade-offs:** A nonzero minimum excludes very small traders; default it to 0 (disabled) and let operators opt in.

**Recommended Priority & Phase:** Medium — Phase 1/3 (trivial; ride the next layout bump).

---

### Feature 5: §2.7 — Cancel-all / batch cancel

**User Value:** A trader (or their bot) pulling out of a market should not need N transactions for N orders.

**Dependencies:** None.

**High-Level Design:** `cancel_orders` — one shard per transaction (a tx can't practically touch many shards anyway), data = `count: u8` + up to ~10 `(order_id: u64, slot_hint: u32)` pairs. The processor loops the existing single-cancel logic (owner-or-reaper auth per order, `Resting`-only guard, slot freeing) and **sums the margin releases into one `release_order_reservation` call** per owner. Simpler alternative worth considering: `cancel_all_mine(shard)` with no id list — scan the shard for the signer's `Resting` orders (bounded: ≤ 8 per trader per shard by the anti-spam cap) and cancel them all; no ids needed, one instruction, self-limiting.

**Implementation Notes:** New instruction (next free discriminator, 32), `definition.rs` + `impl_instructions.rs` + entrypoint wiring per the house pattern. Emits one `OrderCancelled` event per order (indexers already understand it). No new errors.

**Trade-offs:** The scan variant costs O(capacity) reads (90 slots) — trivial CU. The id-list variant is more flexible for reapers (batch-reaping expired strangers' orders).

**Recommended Priority & Phase:** Medium-Low — Phase 4. The 8-order cap makes the pain small today.

---

### Feature 6: §3.1 — Update-market / set-risk-params

**User Value:** An operator must be able to retune a live market (fees, margins, brakes, caps) without killing and re-provisioning it — re-provisioning a market with open positions is not an option at all.

**Dependencies:** None (but §3.2 pause, §3.3 set-oracle, §3.4 transfer should share its pattern).

**High-Level Design:** One `update_market_params` instruction, authority-gated, over an explicit **hot/structural split**:
- **Hot (changeable live):** `maker_fee_bps`, `taker_fee_bps`, `integrator_share_bps`, `crank_fee`, `max_price_move_bps_per_slot`, `soft_stale_slots`, `max_position_notional`, plus future `min_order_notional`/`max_open_interest`. All are read at use-time from `Market`, so a change simply applies to the next operation; none can strand in-flight state.
- **Hot with care:** `maintenance_margin_bps` / `initial_margin_bps` / `liquidation_penalty_bps`. Raising maintenance can make existing positions instantly liquidatable — that is *sometimes exactly the point* (de-risking a market), but it must not be a rug. Mitigation: re-validate the same bounds as `initialize_market` (`maintenance ≤ initial ≤ 10000`, etc.), and make risk-parameter changes **two-step with a slot delay** (see pattern below).
- **Structural (never changeable):** `tick_size`, `num_ticks`, `num_slab_shards`, `orders_per_auction_cap`, `collateral_mint` — they size accounts and PDAs. Enforced by simply not accepting them.
- **The reusable admin pattern** (used by §3.1 risk params, §3.3 set-oracle, §3.4 transfer): a `pending_change` staging area on `Market` — `pending_kind: u8`, `pending_payload: [u8; 48]`, `pending_effective_slot: [u8; 8]` (one appended block covers all three features). `propose_*` writes it (authority); `apply_*` (permissionless!) copies it into effect once `Clock.slot ≥ effective_slot`. Permissionless apply keeps the crank philosophy: the delay is enforced by consensus, not by the authority's honesty, and anyone can complete a proposed change. Fee-only changes can skip the delay (kind-dependent delay of 0).

**Implementation Notes:** One Market layout append (batch with Features 1/4), two instructions (`propose_market_update`, `apply_market_update`), one new event (`MarketParamsUpdated` — indexers need to see retunes), one new error (`NoPendingUpdate`). Validation reuses `initialize_market/data.rs`'s bounds — extract those bounds into shared `const`s so the two paths cannot drift (single-source-of-truth pattern).

**Trade-offs:** The staging area adds ~57 bytes to `Market`. The slot-delay is a governance judgment call (recommend hours, not days, on devnet).

**Recommended Priority & Phase:** High — Phase 2 (the anchor of the admin batch).

---

### Feature 7: §3.2 — Pause / halt / resume

**User Value:** The circuit breaker. Today the authority's only emergency tool is `force_reset`, which *discards resting orders* — a wipe, not a pause.

**Dependencies:** None; `MarketPaused` (error 2) is already reserved.

**High-Level Design:** A `paused: u8` **bitflags** field on `Market` (append):
- Bit 0 `PAUSE_INTAKE`: `submit_order`, `init_maker_quote`, `update_maker_quote_mid/levels` reject with `MarketPaused`. Everything else — cancels, cranks, settles, `reset_shard`, `start_auction`, withdrawals, liquidations — **keeps running**. This is the crucial design point: pausing must let the in-flight round drain and users exit; a pause that traps funds or wedges a half-accumulated round would convert an emergency into a catastrophe. The never-revert settle design means the current round always completes.
- Bit 1 `PAUSE_ROLL` (optional, stronger): `start_auction` also rejects → the market winds down to a fully-settled, quiescent state and stays there. Correct pre-state for §3.3 oracle repoint and §3.4 close-market.
- Deliberately **no pause-withdraw bit**. Freezing user exits is a custody power the program should not have.
- `set_pause(flags)` is authority-gated and immediate (an emergency tool must not have a timelock); *unpausing* can use the §3.1 delay pattern if desired.

**Implementation Notes:** One field, one instruction, guards in 4 processors, wire up error 2 (closing that dead variant), one event (`MarketPauseChanged`). Tests: pause mid-Accumulating → round drains to Settling and rolls (or halts at roll with bit 1); submits rejected; cancel + withdraw still work; liquidation still works.

**Trade-offs:** Minimal. The main discipline is keeping the paused set *small* — every additional paused instruction is a new way to trap someone.

**Recommended Priority & Phase:** High — Phase 1 (small, high leverage, prerequisite for oracle repoint / delist).

---

### Feature 8: §3.3 — Set-oracle / repoint feed

**User Value:** Pyth feeds get deprecated. A market bound forever to a dead feed dies with it (funding halts; window recenter stops; solvency reads fail → only soft-stale wind-down remains).

**Dependencies:** §3.2 (pause), §3.1 (the propose/apply pattern).

**High-Level Design:** Repointing the oracle is the **single most dangerous admin power** in the protocol — whoever controls the oracle controls liquidation prices. Constrain it hard:
1. Only via the two-step propose/apply pattern with a **mandatory, non-zero slot delay** (users must have time to exit before a new feed takes effect).
2. Only while `PAUSE_ROLL` is set and the market is quiescent (fully settled) — no round in flight may straddle two price regimes, and the window floor must re-anchor cleanly on the new feed at the next `start_auction`.
3. `apply_set_oracle` validates the new account live before committing: owned by `PYTH_RECEIVER_ID`, parses via `read_price`, matches the proposed `oracle_feed_id`, fresh + confidence-checked. A proposal for a dead feed can never apply.
4. Both `oracle: Address` and `oracle_feed_id: [u8;32]` update atomically (they are checked as a pair by every reader).

**Implementation Notes:** Reuses the §3.1 staging block (`pending_kind = SetOracle`, payload = 32-byte address is not enough for address+feed → either widen the payload to 64 bytes or stage the feed id and pass the account at apply-time, validating consistency). One event (`OracleRepointed`). Tests: apply blocked while unpaused / while un-quiescent / before delay / with stale target feed.

**Trade-offs:** The quiescence requirement means a repoint takes at least one full paused round. Correct trade — this should be rare and deliberate.

**Recommended Priority & Phase:** Medium — Phase 2 (admin batch; the pattern makes it cheap once §3.1 exists).

---

### Feature 9: §3.4 — Close-market / delist + authority transfer

**User Value:** Markets need an end of life (reclaim rent, retire feeds); authority keys need rotation (compromise, handover).

**Dependencies:** §3.2 (pause bits) for delist.

**High-Level Design:**
- **Authority transfer**: classic two-step — `propose_authority(new)` (staged on the same pending block) + `accept_authority` signed by the **new** key. Two-step prevents transferring to a typo'd dead address. Trivial.
- **Close-market**: a `close_market` instruction gated on total quiescence: `PAUSE_INTAKE|PAUSE_ROLL` set, round fully settled, `oi_long == oi_short == 0`, every shard empty (`count == 0` per shard, passed as trailing accounts like `force_reset`), `active_maker_quote_count == 0`. Then close histogram + clearing result + shards + market via the existing `close_pda_account`, rent to the authority. Positions and `UserCollateral` are user-owned — they close via their own paths (a `close_position` for a flat position is a small companion instruction; ledgers are per-mint, market-independent, and stay).
- The genuinely hard part is *getting* to zero OI: users must close, or be closed. With §3.2 + funding still accruing + liquidation still live, a delisting market drains naturally; a stubborn last position is an operational problem (raise maintenance via §3.1 within bounds), not a program one. Do not build a force-close-at-oracle — same free-option objection as Feature 2.

**Implementation Notes:** Two small instructions + one event each. The quiescence checks reuse existing per-shard scan helpers. New error: `MarketNotQuiescent`.

**Trade-offs:** None significant; strictly additive.

**Recommended Priority & Phase:** Low-Medium — authority transfer in Phase 2 (tiny, high hygiene value); close-market in Phase 5.

---

### Feature 10: §4.1 — Insurance fund seed / withdraw

**User Value:** The insurance pool is the shock absorber for the entire risk engine — and today it can only be filled by trader losses/fees and only drained by crank fees and winners' PnL. It cannot be bootstrapped before launch (a fresh market has a zero pool: the first rebate clamps to zero, the first bad debt goes straight to ADL), and accrued protocol fees are economically trapped.

**Dependencies:** §4.2's aggregate counter makes the withdraw side safe (see below).

**High-Level Design:** Two instructions:
- `seed_insurance(amount)` — **permissionless**: SPL-transfer `amount` from the caller's token account into the vault token account and `insurance_balance += amount`. Conservation is trivially preserved (both sides of the invariant grow together). Anyone may donate; there is no attack in giving the pool money.
- `withdraw_insurance(amount)` — authority-gated, and this is where care lives. The invariant `vault_token ≥ Σ balances + insurance` is currently *not checkable on-chain* (Σ balances lives across all ledgers). Sequencing matters: **build §4.2's `total_user_balance` aggregate first**, then the withdraw check is exact and on-chain: require `amount ≤ insurance_balance` AND `vault_token_after ≥ total_user_balance + insurance_after`. Add the §3.1 slot-delay (a compromised authority draining insurance instantly is the scenario to price in) and an event.

**Implementation Notes:** Two instructions, `InsuranceSeeded`/`InsuranceWithdrawn` events, reuse HS-12 token pinning and the vault-authority signer. Tests: seed → rebate no longer clamps; withdraw blocked when it would breach the on-chain invariant.

**Trade-offs:** Withdraw introduces the first authority-controlled token outflow in the program. The delay + on-chain invariant check are the price of admission; do not ship withdraw without them.

**Recommended Priority & Phase:** Medium-High — Phase 2/3 (seed is safe immediately; withdraw after §4.2).

---

### Feature 11: §4.2 — Insurance segregation / on-chain invariant

**User Value:** Today `vault_token ≥ Σ balances + insurance` is enforced only by host tests. On-chain, a future bug in any money path could silently break backing and nobody would know until withdrawals fail.

**Dependencies:** None; enables Feature 10's withdraw.

**High-Level Design:** Two options considered:
- *Full segregation* (separate insurance token account): real isolation, but every settle/liquidate path that today moves a `u64` between bookkeeping fields would become a token CPI — heavier CU on the hottest paths, more accounts per settle tx, and it still doesn't make Σ user balances checkable. Rejected.
- **Aggregate counter (recommended)**: add `total_user_balance: [u8;16]` (u128) to `Vault`. Every `credit`/`debit`/`apply_pnl` on any `UserCollateral` adjusts it. The key question is what this does to parallelism — and the answer is: nothing that matters. Deposits, withdrawals, settles, and liquidations **already pass or need the vault** (or are inherently serial per-market money operations); the parallel-critical path is *order intake*, which never touches balances. One caveat: `settle_fill` currently only requires the vault when `balance_delta != 0 || shortfall > 0` — with the aggregate, that's exactly when the counter changes too, so the account requirement is unchanged.
- With the aggregate in place, add a **cheap invariant assert** at the two token-outflow sites (`withdraw`, `withdraw_cross`, future `withdraw_insurance`): `vault_token_account.amount ≥ total_user_balance + insurance_balance` (the vault token account is already passed there). This turns a host-test invariant into an on-chain fail-closed gate at exactly the moments money can leave.

**Implementation Notes:** `Vault` VERSION 2→3 (append), touch every `UserCollateral` mutation site (they are few and centralized: deposit, withdraw×2, settle_money, settle paths, liquidate×2), one new error (`VaultInvariantViolated`). Property test: run the whole existing money-path suite and assert the counter equals the scanned Σ after every instruction.

**Trade-offs:** A redundant counter is exactly the pattern Design Z removed from *completeness* — but there the counter guarded liveness with a scan alternative; here no scan alternative exists on-chain, and drift is caught by the outflow assert (fail-closed, funds stay safe). The asymmetry justifies the different choice; document it.

**Recommended Priority & Phase:** Medium-High — Phase 2/3, before insurance withdraw.

---

### Feature 12: §5.1 — EMA / TWAP pricing

**User Value:** A single spot print is the most manipulable possible index input; Pyth's `ema_price` is a free, already-in-the-account smoother.

**Dependencies:** None — `oracle.rs` already parses the byte region; `ema_price`/`ema_conf` sit at fixed offsets right after `publish_time` (+ `prev_publish_time`), so the reader extension is ~15 lines.

**High-Level Design:** Parse `ema_price` into `OraclePrice` (normalize with the same exponent path). Then a **policy** decision per consumer:
- **Funding**: use EMA as the index side of the gap (`mark − ema`) — funding is meant to track persistent divergence, not print-to-print noise; EMA is the natural fit and reduces the §9.2 oscillation worry.
- **Liquidation/solvency**: stay on the **raw spot** price. This is deliberate and must not change: a lagging EMA during a crash recreates exactly the anti-liquidation-brake bug that §2.2 fixed. At most, use `max(spot_conf, ema divergence)` as an extra confidence-style sanity check.
- **Window recenter**: spot is fine (it's frozen per round anyway).

**Implementation Notes:** Reader change + funding call-site change + goldens in `tempo-math::oracle` (the off-chain mirror must stay in lockstep — regenerate its golden tests). No account layout change.

**Trade-offs:** EMA lag cuts funding responsiveness to genuine fast moves; the ±1%/period cap already dominates that regime, so little is lost.

**Recommended Priority & Phase:** Low-Medium — Phase 5.

---

### Feature 13: §5.2 — Unified mark price

**User Value:** Two definitions of "mark" (funding = oracle-banded clearing midpoint; liquidation = raw oracle) confuse integrators and complicate the risk story.

**Dependencies:** None.

**High-Level Design:** After tracing both call sites (`update_funding/processor.rs:72`, `liquidate/processor.rs` via `solvency_mark`), the conclusion is that this split is **mostly correct and should be kept — but named honestly** rather than unified:
- Funding *must* see the market-vs-index gap; that is its entire job. Using the raw oracle for funding would make the rate permanently ≈ 0.
- Liquidation *must not* be steerable by the book; that is the §2.2 lesson. Using the banded clearing mid for solvency hands the book (especially pre-§7.1 unbacked makers!) a lever over liquidation prices, bounded only by the band.
- Proposal: (1) rename in code/docs — `funding_mark` vs `solvency_price` — and expose both in the API so integrators see two numbers with two names, not one ambiguous "mark"; (2) the *only* real unification worth doing: `read_oracle`'s event should publish both; (3) revisit only if §7.1 lands and simulation shows the banded mid is manipulation-resistant enough to be a shared definition.

**Implementation Notes:** Renames + docs + one event field addition. Essentially free.

**Trade-offs:** Accepting two prices is a documentation burden but the safe equilibrium.

**Recommended Priority & Phase:** Medium (as the rename/doc task) — Phase 3; a true unification stays a research question.

---

### Feature 14: §6.1 — Partial liquidation

**User Value:** A position 1% under maintenance currently loses 100% of its position (full close + penalty on full notional). That is punitive, moves more size through a thin auction-less close than needed, and increases bad-debt risk on large positions.

**Dependencies:** None hard; interacts with §6.2 (reward floor) and the min-size floor (Feature 4).

**High-Level Design:** Close the **minimum fraction that restores health plus a buffer**, integer-only:
- Closing fraction `f` of the position at mark: equity changes by the realized slice (already embedded in equity — closing at mark realizes exactly the unrealized share, so equity is *unchanged* by the close itself) minus the penalty on the closed notional; maintenance shrinks proportionally: `maint' = maint × (1−f)`.
- Solve for the smallest `close_qty` such that `equity − penalty(close_qty) ≥ maint × (|size| − close_qty) / |size| × (1 + buffer_bps/10⁴)`. Linear in `close_qty` → a closed-form integer division (`mul_div_ceil` for `close_qty`, rounding **up** = closing slightly more, against the position holder — the consistent rounding direction). No iteration, no new math primitives.
- Floors and edges: if `close_qty ≥ |size|` or the remainder would fall below `min_order_notional`, do a full close (today's path). If equity ≤ 0, full close + bad debt (unchanged). Add `liquidation_close_buffer_bps: u16` to Market (the hysteresis that prevents liquidate-every-slot loops).
- **Progress guarantee** (the existing `maintenance_deficit` "must strictly shrink" rule): a partial close with the buffer strictly reduces the deficit to zero by construction; wire `LiquidationNoProgress` (error 34, currently reserved) as the guard if a degenerate config produces `close_qty == 0`.
- `liquidate_cross`: same formula against the *target leg*, with combined equity/maintenance as inputs. The "first non-flat member" targeting stays; partial close of that member is strictly less disruptive than today's full close.

**Implementation Notes:** Changes concentrated in `liquidate`/`liquidate_cross` processors + a new pure function in `margin.rs` (`partial_close_qty(...)`) with its own fuzz (20k iters: post-close health restored, penalty conserved, rounding direction) and ideally a Kani panic-freedom harness (the formula is one `mul_div_ceil` — CBMC-tractable in the same envelope style as `unrealized_pnl`). OI update uses the existing `apply_oi_delta(old, new)` with the reduced size. Event gains a `closed_qty` field.

**Trade-offs:** Partial liquidation leaves a live position that may need liquidating again next slot in a fast crash — the buffer trades user-friendliness against liquidation-cascade chattiness. Keep full-close as the fallback whenever in doubt.

**Recommended Priority & Phase:** High — Phase 3 (the largest single improvement to user fairness in the risk engine).

---

### Feature 15: §6.2 — Keeper-reward floor

**User Value:** Liquidating a near-zero-equity position nets the liquidator ~0 (penalty caps to equity, `margin.rs:121-124` verified) while costing gas — precisely the positions that *most* need liquidating are the least incentivized. Other cranks (`process_chunk`, `settle_fill`, `update_funding`, `start_auction`) pay nothing.

**Dependencies:** Insurance seeding (Feature 10) so the pool can actually fund floors.

**High-Level Design:** Two different problems, two answers:
- **Liquidation floor**: `liquidation_reward_floor: u64` on Market. Liquidator receives `max(penalty, floor)`, the topped-up part paid **from insurance, capped at insurance** (the exact pattern of `finalize_clear`'s crank fee — conserving, fail-soft to whatever insurance has). Griefing is structurally impossible: a liquidation only executes when `equity < maintenance`, an on-chain condition an attacker cannot manufacture for free — every real liquidation the floor pays for is work the protocol wanted done.
- **General crank rewards**: **do not pay per-call.** `process_chunk`/`settle_fill` are self-serve-able and splittable — paying per call invites splitting one round into maximal calls to farm fees, and capping per round adds bookkeeping (a counter — the anti-pattern) for tiny value. Instead extend the *existing* `finalize_clear` crank-fee shape to the two other once-per-round chokepoints if needed (`start_auction`, and per-shard `reset_shard` with fee = `crank_fee / num_slab_shards`), each intrinsically once-per-round so unfarmable. Settle cranking stays economically motivated by the parties who want their fills (users, mm-bot, keeper).

**Implementation Notes:** One Market field (batch the bump), ~10 lines in `liquidate`/`liquidate_cross` (mirror the crank-fee block), optional `reset_shard`/`start_auction` fee blocks with the optional trailing `cranker_collateral`+`vault` accounts pattern. Tests: floor paid when penalty < floor; capped at insurance; conservation holds.

**Trade-offs:** Every floor is an insurance outflow; keep defaults small and tune via §3.1.

**Recommended Priority & Phase:** Medium — Phase 3, with partial liquidation.

---

### Feature 16: §7.1 — Maker collateral check at quote time ⚠️ top open safety item

**User Value / Threat:** `init_maker_quote` / `update_maker_quote_levels` take **no collateral account at all** (verified: zero references in either processor). A maker with zero deposit can post an 8×8 ladder of arbitrary size, which folds into the histogram, **moves the uniform clearing price for every participant**, and then settles into a shortfall that insurance (then ADL) absorbs — `settle_maker_quote` locks only what's available (`lock_up_to`) and socializes the rest. This is simultaneously a *price-manipulation* vector and an *insurance-drain* vector, and it undercuts the DFBA pitch itself (batch auctions are supposed to be manipulation-resistant; unbacked quotes reopen the door).

**Dependencies:** Bundle with Issue 6 (§4.9 multi-quote seeds) — one maker-book layout change instead of two.

**High-Level Design:** Apply the taker book's own medicine — **reserve worst-case, then release** (safety pattern #4):
- **Reservation formula** (pure, integer, no new primitives): both auctions can cross in the same round, so both sides can fill simultaneously and the reservation must cover the sum: `reserve = initial_margin(Σ bid qty_k, at each bid's own tick price) + initial_margin(Σ ask qty_k, at window-top price)` — buys clear at ≤ their limit (their own tick is the worst case), sells clear at ≤ window top (identical to `submit_order`'s `worst_price` rule). ≤ 16 `mul_div_ceil` calls per update; trivial CU.
- **Where it locks:** `update_maker_quote_levels` (and `init`, whose ladder is empty → reserve 0) gains a required `user_collateral` account (owned by the **maker** — a delegate can reshape the ladder only within the maker's already-locked reservation, or the ix re-locks against the maker's ledger; delegate still never moves funds *out*). Compute new reservation, `lock` the delta (or `release` if smaller); reject with `InsufficientCollateral` if free balance can't cover — the same clean at-submit rejection takers get.
- **Mid moves:** here is the subtle part. `update_maker_quote_mid` must stay O(1) and collateral-free — that's the product. Achieved by making the reservation **mid-independent**: bids reserve at their *own tick* price which moves with mid… so instead reserve bids at the **window-top price too** (a buy's fill price is ≤ its limit ≤ window top). Slightly over-reserves bids, but makes the reservation a pure function of the *ladder shape* (sizes only), so mid moves need no re-margining. Over-reservation against a tight oracle-anchored window is small — the same accepted trade as taker sells (§1.1's note).
- **Storage:** `reserved_margin: [u8;8]` appended to `MakerQuote` (VERSION 3→4, batched with the §4.9 seed change). Released on `clear_maker_quote` (full) and adjusted at `settle_maker_quote` exactly like `settle_fill` does for orders: release the filled slice's share, keep the leftover. With reservations in place, `settle_maker_quote`'s `lock_up_to` fallback becomes a true backstop instead of the primary path — restoring "settlement never reverts *and never under-locks*" for makers.
- **Window recenters:** reservation priced at window top of the round it was set; a recenter *upward* raises the top. Same solution as taker resting orders: snapshot a `worst_price` at reservation time (mid-independent = shape × snapshot price) — stable across recenters, exactly the `Order.worst_price` pattern.

**Implementation Notes:** `MakerQuote` v4 (append `reserved_margin` + `worst_price`), required `user_collateral` on `update_maker_quote_levels`, release in `clear_maker_quote`/`close_maker_quote` (must be zero to close), settle-side netting in `settle_maker_quote`. Reuse `settle_money::release_order_reservation` (it validates owner — exactly the reaper-safety property needed). New tests: unbacked ladder rejected; ladder shrink releases; fold→settle→release conserves; recenter doesn't break the reservation; the existing `two_makers_share_marginal_tick_and_conserve_oi` regression re-run under reservations. Breaking change for the mm-bot (must hold collateral before quoting) — update `crates/mm-bot` in the same PR.

**Trade-offs:** Capital efficiency: makers now post margin for the *whole ladder* worst case even though at most the crossed portion fills — this is the DFBA price of unconditional solvency, identical to the taker-side trade already accepted in §1.1. An operator knob (`maker_reserve_bps` discount) was considered and rejected: any discount reintroduces a bounded version of the drain.

**Recommended Priority & Phase:** **Critical — Phase 1. This is the top open item in the entire backlog.**

---

### Feature 17: §7.2 — Inventory / skew management

**User Value:** N/A — correctly absent **by design**. The quote is static between explicit updates; skewing to inventory is the maker's off-chain job (`crates/mm-bot::strategy::build_quote` already does oracle-anchored, inventory-skewed ladders).

**Recommendation:** Keep as-is. On-chain auto-skew would add state and CU to the fold path for something the off-chain loop does better. Close the item as "won't build" rather than leaving it looking like debt.

**Recommended Priority & Phase:** None (document the decision).

---

## Cross-Cutting Observations & Recommendations

**1. The docs have drifted behind the code in three places — fix before designing against them.** Reduce-only margin (full reservation now, not headroom-discounted), the order-cap bound (90, not 128), and §2.11's original overstatement (already corrected in-place). The codebase's own "single source of truth" discipline should extend to docs: where a doc states a bound the code enforces (caps, bps ranges), cite the `const` by name so grep finds the pair.

**2. One missing abstraction unlocks the whole admin backlog: the staged-change block.** §3.1 update-params, §3.3 set-oracle, §3.4 authority transfer, and §4.1 insurance-withdraw all want the same primitive: *authority proposes → delay elapses → anyone applies*. Building it once (a `pending_kind/payload/effective_slot` block on `Market`/`Vault`, with permissionless `apply`) keeps the crank philosophy intact — even admin changes complete permissionlessly — and turns four features into one pattern plus four thin payloads.

**3. The reservation pattern is the house style for solvency — extend it, don't invent alternatives.** §7.1 (maker margin) is a direct transplant of `Order.reserved_margin`. The one place it *cannot* be transplanted is the max-OI cap (Feature 1), because OI is a Market-global aggregate and reserving it at submit would re-serialize intake — the exact cost Design Z paid v9→v11 to remove. The soft-cap compromise is the right call; write the bounded-overshoot argument down so it isn't re-litigated.

**4. Counters vs. scans: the codebase's hard-won rule has a legitimate exception.** Design Z's lesson is "prove by scanning, not by mirrored counters" — for *liveness gates with a scan alternative*. Feature 11's `total_user_balance` is a counter with **no** on-chain scan alternative, guarding a *fail-closed money gate*: drift there blocks withdrawals (safe) rather than wedging rounds (the counter failure mode Design Z suffered). Naming this distinction explicitly (liveness-counter = forbidden; conservation-counter with fail-closed check = allowed) will save a future reviewer from either extreme.

**5. Insurance is the universal shock absorber — every new flow must route through the two existing choke points.** Crank fees, rebates, floors (Feature 15), seeds/withdraws (Feature 10), and socialization all touch `insurance_balance`. The discipline that fixed §1.1/§1.2 (one shared `conserve_and_socialize`) should hold: no new instruction mutates `insurance_balance` inline; extend `settle_money.rs` instead.

**6. What is already excellent and must be preserved at all costs:** the division-free two-pass `find_cross`; the telescoping-floor marginal fill (order-independent, zero-dust — FOK died on this altar, correctly); scan-based completeness with the per-shard de-dup mask; `Market` read-only on submit/cancel (parallel intake); never-revert settlement backed by worst-case reservations; raw-oracle solvency pricing (never braked, never banded); and the fail-closed `InsuranceInsolvent` stance. Several proposals above were shaped specifically to avoid touching these (soft OI cap, mid-independent maker reservations, no force-close-at-oracle, no per-call crank fees).

---

## Prioritized Roadmap (solo developer, Claude Code + Pinocchio)

Grouping principle: **batch every `Market`/`MakerQuote` layout change** (each bump forces a devnet re-provision, so pay that cost as few times as possible), ship the safety-critical program change first, and keep test/ops work flowing between program milestones.

**Phase 0 — Hygiene (days):**
regen the web vendor bundle + CI guard (Issue 2) · doc fixes (Issue 1, Feature 0, close §7.2 as won't-build) · Stage-B marketable-fill LiteSVM + devnet test (Issue 3) · SDK always-attach-ledger (Issue 1).

**Phase 1 — The safety release (1–2 weeks):** one maker-book layout bump: **maker quote-time margin (Feature 16)** + multi-quote seeds (Issue 6) + `MakerQuote` v4; **pause bitflags (Feature 7)**; **min order notional (Feature 4)** riding the same `Market` bump. mm-bot updated in lockstep. Devnet re-provision once.

**Phase 2 — The admin release (1–2 weeks):** the staged-change block + `update_market_params` (Feature 6) · authority transfer (Feature 9a) · set-oracle (Feature 8) · `total_user_balance` aggregate + on-chain outflow invariant (Feature 11) · insurance **seed** (Feature 10a). One `Market` + one `Vault` bump, batched.

**Phase 3 — The risk-depth release (2–3 weeks):** partial liquidation (Feature 14, with fuzz + Kani harness) · liquidation reward floor (Feature 15) · soft max-OI cap (Feature 1) · insurance **withdraw** with delay + invariant (Feature 10b) · mark-price renames/docs (Feature 13).

**Phase 4 — The trading-UX release:** IOC via the one-comparison expiry change + market-order SDK sugar (Features 2, 3) · batch cancel (Feature 5).

**Phase 5 — Benchmark-gated & polish:** the round-latency benchmark → C2 only on evidence (Issue 4) + keeper early-roll (Issue 5) · structured error codes in `benign()` (Issue 7) · EMA for funding (Feature 12) · close-market (Feature 9b).

---

## Appendix: Files Read During Analysis

**Input documents:**
- `docs/known-issues.md` (full)
- `docs/missing-features.md` (full)
- `research.md` (authored from the prior deep read; used as the grounding model)

**Source files read in full during the deep read that grounds this analysis** (all of `program/src/`, ~18,800 lines):
- Core math: `clearing.rs`, `mark.rs`, `funding.rs`, `margin.rs`, `cross_margin.rs`, `oracle.rs`, `wide_math.rs`, `settle_money.rs`, `kani_proofs.rs`
- State: `state/mod.rs`, `state/market.rs`, `state/histogram.rs`, `state/order.rs`, `state/clearing_result.rs`, `state/position.rs`, `state/vault.rs`, `state/user_collateral.rs`, `state/maker_quote.rs`, `state/margin_account.rs`
- Clearing-path instructions (accounts/data/processor each): `initialize_market/`, `init_shard/`, `submit_order/`, `cancel_order/`, `process_chunk/`, `finalize_clear/`, `settle_fill/`, `start_auction/`, `reset_shard/`, `force_reset/`, `instructions/round.rs`
- Money/risk instructions: `init_vault/`, `init_collateral/`, `deposit/`, `withdraw/`, `init_position/`, `update_funding/`, `liquidate/`, `read_oracle/`, `init_margin_account/`, `add_position_to_margin/`, `remove_position_from_margin/`, `withdraw_cross/`, `liquidate_cross/`, `migrate_market/`, `migrate_position/`
- Maker-quote instructions: `init_maker_quote/`, `update_maker_quote_mid/`, `update_maker_quote_levels/`, `process_maker_quote/`, `settle_maker_quote/`, `clear_maker_quote/`, `close_maker_quote/`
- Plumbing: `Cargo.toml`, `build.rs`, `lib.rs`, `entrypoint.rs`, `errors.rs`, `traits/` (account, instruction, pda, event), `utils/` (macros, account_utils, program_utils, pda_utils, event_utils), `events/` (all), `instructions/mod.rs`, `instructions/definition.rs`, `instructions/impl_instructions.rs`, `instructions/emit_event/`

**Spot-verified again during this analysis (line-level checks of doc claims):**
- `instructions/settle_fill/processor.rs:280-300` (release gated on `release_amount > 0` — confirms §2.11's corrected statement)
- `instructions/submit_order/processor.rs:36-38, 116-151, 199-286` (reduce-only reserves full worst-case margin; per-shard trader cap — confirms Feature 0's doc-drift finding)
- `instructions/liquidate/processor.rs:205-220` (full-position zeroing + ADL socialization — confirms §6.1)
- `margin.rs:110-130` (penalty capped to equity — confirms §6.2)
- `instructions/finalize_clear/processor.rs:205-230` (idempotent canonical ClearingResult PDA; crank-fee block)
- `instructions/update_funding/processor.rs:72` and `instructions/read_oracle/processor.rs:64` (funding mark = banded clearing midpoint — confirms §5.2's two-mark finding)
- `instructions/update_maker_quote_levels/processor.rs`, `instructions/init_maker_quote/processor.rs` (zero `collateral` references — confirms §7.1)
