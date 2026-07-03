# Tempo — Known Issues

This document lists defects in **code that already exists** — things that are
broken, dead/redundant, or wrongly designed. It is deliberately separate from
`missing-features.md` (which lists functionality not yet built).

Every item cites a real `file:line`. Items are classified:

- **[bug]** — produces a wrong result or breaks an invariant.
- **[design]** — works today but the design will bite as the program scales or
  is used adversarially.
- **[dead]** — written but never read, or duplicated; safe to delete or wire up.

The clearing arithmetic itself (`find_cross` / `compute_fill` /
`compute_marginal_fill`) is correct and well-tested. The defects found so far
clustered in the **money/settlement glue** and the **lifecycle bookkeeping**
around it. Nearly all of them are fixed; this document leads with the small
number that remain, then keeps a compact record of what was found and closed —
not a full changelog of the deliberation that went into each fix.

**Status legend:** fixed · partial · deferred (blocked on a product decision
and/or a devnet re-provision) · not started/unverified.

---

## Part A — Currently open

This is the section that matters day to day. Part B below is resolved
history, kept intentionally short.

| Item | Area | Status |
| --- | --- | --- |
| 2.11 Ledger required to release a reservation on cancel/zero-fill settle | design | By-design; corrected below (the doc previously overstated when the ledger is required) |
| 2.12 Devnet client bundle drift after the sharding layout bump | operational | Mostly resolved; one vendor bundle needs a regen check |
| 2.13 Stage-B marketable-fill: not yet validated end-to-end on a live chain | design | Deferred — a real coverage gap |
| 2.14 Stage-C2 (true round-processing overlap) not built | design | Deferred — gated on a benchmark |
| 2.15 Keeper doesn't open the next `Collect` early | design | Deferred — scheduling optimization only |
| 4.9 One `MakerQuote` PDA per maker | design | Deferred — needs a program change |
| 4.10 Off-chain `benign()` error classifier uses string matching | design | Accepted limitation |

### 2.11 Releasing a reservation on cancel/zero-fill settle requires the collateral ledger — [design]

`cancel_order/processor.rs` requires the owner's `user_collateral` whenever
the order being cancelled carries `reserved_margin > 0` — that part is exact.

For `settle_fill`, the actual gating condition is `release_amount > 0`
(`processor.rs:294-298`), not `reserved_margin > 0` — a zero-fill order that
simply re-rests into the next round releases nothing and needs no ledger
account (`processor.rs:290-293`). The ledger is only required when a
zero-fill order leaves the book entirely — expired, or force-consumed because
it's `reduce_only` — since that's the only case that actually releases the
reservation.

Not a wedge risk: a permissionless cranker always knows the order's owner and
can attach the PDA-derived ledger account. A stale client that omits it
simply gets `MissingSettleAccounts` and must add the account.

**To close:** update any client/doc reference to say "the ledger is required
whenever a settle releases a reservation (a fully-consuming settle), not on
every zero-fill settle."

### 2.12 Devnet client bundle drift after the sharding layout bump — [design] · operational

`Market` is now **VERSION 11** and `InitializeMarketData::LEN` is **131
bytes**. What changed: v10 added Stage-A sharding (`num_slab_shards: u16`
appended to both the instruction data and `Market`, plus a
`shards_pending`/`shards_ready` counter pair); v11 then removed
`shards_pending` (Design Z — completeness is proven by `finalize_clear`
scanning every shard live, not by a `Market`-level counter), keeping
`shards_ready` for the roll gate. `InitializeMarket`'s byte length is
unaffected by v11 — only `Market`'s own state layout shrank.

Both `clients/typescript/src/generated/` and `crates/sdk/src/generated/`
already reflect the sharded layout (generated the same day as the sharding
merge). `apps/web/src/vendor/tempo-client.mjs` predates that merge and should
be checked and regenerated before any devnet money-path run against the
current layout.

**To close:** `pnpm generate-clients && pnpm bundle-client`, confirm the web
vendor bundle picks up the sharding fields, and re-provision any devnet
market against the v11 layout (old accounts fail the version check by
design — re-provision, not migrate).

### 2.13 Stage-B marketable-fill is only unit-tested, never validated on a live chain — [design]

Not a known defect — a **coverage gap** flagged during the DDR-3 code
reviews. When the oracle-anchored tick window recenters past a fixed-price
resting order, `classify_resting_fold` folds a *marketable* order at the
boundary tick so it fills (DDR-3). The park/no-wedge half is fully tested
end-to-end (LiteSVM: passive orders skip, don't block finalize, fold once the
window returns). The **fill** half — a marketable order actually executing
against a live counterparty after a recenter — is proven only at the unit
level (`classify_resting_fold`'s tick correctness, and fold-then-settle with
no counterparty); no integration test yet combines a recenter with an actual
live counterparty fill.

**To close:** run this end-to-end on devnet (the sharded-book design is on
`main` now, not a separate feature branch): place a resting order, let the
window move through its price, and confirm it fills at the clearing price
with correct margin/position. See DDR-3 in `docs/design-decisions.md`.

### 2.14 Stage-C2 (true round-processing overlap) not built — [design]

Stage C1 (always-open submit) removed the *submit* dead-time, but round
*processing* is still serial: one histogram, so round N+1 can't accumulate
while round N settles. C2 would double-buffer the histogram + `ClearingResult`
by round parity (`docs/plan.md §4.2`) for true overlap.

Deliberately not built: C2 runs two live rounds over one durable book — the
riskiest change in the whole scaling effort. The plan explicitly gates it
behind a benchmark proving C1 + Stage-A parallel settle is
throughput-insufficient. `docs/bench/cu_report.md` measures Stage-A
throughput but doesn't yet answer that specific question.

**To close:** run the O(ticks)/throughput benchmark; if C1 is the bottleneck,
implement C2 per `docs/plan.md §4.2`.

### 2.15 Keeper does not open the next `Collect` early — [design]

Plan task C1.6: the keeper could schedule the next `Collect` to open as soon
as `Discovered`/`Settling` begins rather than after full settlement,
shortening the round gap. A keeper scheduling change only
(`crates/keeper`); correctness does not depend on it, and C1 already lets
users submit at any time.

**To close:** adjust the keeper cadence once C1 latency is measured (pairs
naturally with the §2.14 benchmark).

### 4.9 One MakerQuote PDA per maker — single-ladder limit — [design]

PDA seeds are `[b"maker_quote", market, maker]` — one active quote per maker
per market. The mm-bot cannot widen its posted depth by running two ladders,
and if the single quote is folded mid-round the bot cannot re-quote until
`clear_maker_quote` + next round. A multi-level PDA (e.g. an added
`quote_id` seed) would allow multiple concurrent quotes per maker. Interim
workaround documented in `ops/README.md` and `crates/sdk/src/pda.rs`.
Requires a program change; deferred.

### 4.10 `benign()` uses fragile string-matching — [design]

`crates/sdk/src/retry.rs` classifies errors by matching substrings ("custom
program error", "already", "wrong phase", "not found") in the RPC error
string. If Solana or an RPC provider changes the error message format, the
classifier silently changes behavior. The durable fix is for the program to
surface structured, numeric error codes the SDK can match on directly; until
then this is an accepted, documented fragility, with a regression test in
place to catch format drift.

---

## Part B — Resolved (compact record)

Everything below this line is fixed. Kept short and grouped by area, as a
record of what was found and how it was closed — not as a live task list.

### 1. Money-path bugs (settlement glue)

- **1.1 Maker settle could mint money; taker settle could not.** Both settle
  paths now route through one shared `settle_money::conserve_and_socialize`,
  which fails closed (`InsuranceInsolvent`) on an underfunded gain instead of
  minting.
- **1.2 `settle_fill` bad debt was logged, never absorbed.** Both settle
  paths now socialize uncovered bad debt the same way `liquidate` does, via
  `market.socialize_bad_debt`.
- **1.3 `is_maker` was an unvalidated client flag that steered price
  formation.** `submit_order` is now **taker-only** — the `is_maker` byte was
  removed from its wire format. Maker liquidity comes exclusively from the
  on-chain `MakerQuote` book, where "maker" is a verifiable fact (a standing,
  foldable quote) rather than a self-asserted byte. Breaking IDL change;
  clients regenerated, the web maker/taker toggle removed, the integration
  suite rewritten to source maker liquidity from the quote book.
- **1.4 Cross-margin health ignored unsettled funding/social-loss on
  read-only legs.** A pure `pending_social_loss` (mirroring
  `settle_social_loss`) now docks unsettled funding + social loss on every
  member leg in both `withdraw_cross` and `liquidate_cross`.
- **1.5 Socialized-loss (ADL) under-charged a flipped position.**
  `Position::apply_fill` now re-snapshots the social-loss checkpoint whenever
  a fill opens from flat **or** flips sign, making "the checkpoint matches
  the current side" a type invariant instead of a hand-maintained one.
- **1.6 Maker-quote marginal-tick rationing mis-allocated when two or more
  makers shared the tick.** Each maker quote now carries a per-level
  fold-time `cum_before` snapshot, so makers at a shared marginal tick tile
  the bucket exactly and fills sum to the allocated volume. Regression test
  `two_makers_share_marginal_tick_and_conserve_oi`. Breaking:
  `MakerQuote::VERSION` 2→3.
- **2.10 Reduce-only settle race.** Resolved by a later, unrelated design
  change rather than by the conservation fuzz this item originally called
  for: reduce-only orders now reserve the **full** worst-case margin like any
  other order (no headroom discount — see `missing-features.md §2.2` and
  DDR-3), so the described race — opening more exposure than reserved — can
  no longer occur. Confirmed by
  `tests/integration-tests/tests/pretrade_safety.rs::reduce_only_reserves_full_margin_no_discount`.
- **2.16 Keeper attached an uninitialized position on a zero-fill settle,
  wedging the round.** Off-chain fix (`crates/keeper`, commit `73efc63`): the
  keeper now checks position existence on-chain before attaching it to a
  settle call. The on-chain program's zero-fill-without-a-position path was
  already correct; the bug was the keeper unconditionally attaching an
  account that didn't exist. Found by a devnet flood-load run.

### 2. Design fixes (lifecycle & bookkeeping)

- **2.1 Three hand-maintained order counters, no derived invariant.**
  `finalize_clear` now derives completeness from the slab itself
  (`all_active_orders_accumulated`); the redundant `Market`-level order
  counters were removed entirely (PERF-1).
- **2.2 The per-slot price brake could delay liquidations during a crash.**
  All three solvency-pricing paths (`liquidate`, `liquidate_cross`,
  `withdraw_cross`) now price off the raw, confidence-checked per-leg oracle
  via one shared resolver, `oracle::solvency_mark`; the braked effective
  price remains only the funding/soft-stale anchor, never a liquidation gate.
- **2.3 Mint-agnostic collateral ledger vs. per-mint vaults.**
  `UserCollateral` is now mint-scoped: seeds `[b"collateral", owner,
  collateral_mint]`, `VERSION` 1→2. Closes the cross-mint contamination
  vector at the data model. (`risk-model.md` and `missing-features.md` still
  cite this section as the reason collateral is single-mint by design — that
  reference stays valid.)
- **2.4 Cross-margin was reconstruct-per-call, with a funds-stuck trap.**
  `RemovePositionFromMargin` frees a flat member's slot so a churned group is
  never permanently full; a flat member now rides as a single bare account
  (no market/oracle needed) instead of the full live triple, cutting a
  fully-live `liquidate_cross` from ~31 accounts toward ~17 for a mostly-flat
  group. "Maintain combined equity as persisted state" was evaluated and
  dropped as infeasible — both equity and maintenance are functions of each
  leg's live oracle price, so there is no static scalar to cache without
  reintroducing §2.2's stale-pricing bug; the remaining live-leg account
  ceiling is a transaction-layer concern, solved off-chain with an Address
  Lookup Table.
- **2.5 `margin_mode` was mutable mid-auction.** `add_position_to_margin` now
  rejects the bind while the owner has any in-flight `Resting`/`Accumulated`
  order this round.
- **2.6 `migrate_position`'s OI rebuild depended on an unenforced
  ordering.** Gated on an empty slab (the same quiescence condition
  `start_auction` uses), so no in-flight settle can race the OI counters.
- **2.7 Tick window pinned at genesis / O(n²) settle scans / hardcoded
  oracle feed.** Three fixes in one breaking layout bump (`Market` 6→7,
  `OrderSlabHeader` 1→2, `Order` 72→80 bytes): the tick window now re-anchors
  to the oracle every round roll and is frozen for the round in between; a
  bump cursor + validated slot hint + fold-time `cum_before` snapshot bring
  settle down to O(1) with zero slab scans on the happy path; `read_oracle`
  reads the market's own `oracle_feed_id` instead of a hardcoded feed.
- **2.8 `withdraw_cross` masked collateral-ledger drift with
  `saturating_sub`.** Changed to `checked_sub` with a hard
  `CollateralLedgerDrift` error, so a member-set/ledger drift surfaces loudly
  instead of silently clamping to zero.
- **2.9 Low-severity batch** (future-timestamp fallback, hand-inlined health
  math, wrong error class on an all-flat group, dead error variant). Four of
  five fixed together; the fifth (per-leg market re-parsing in
  `liquidate_cross`) is an unmeasured micro-optimization intentionally left,
  bounded by `MAX_CROSS_POSITIONS = 8`.

### 3. Dead / redundant code

All items closed. A triplicated `price→tick` mapping was extracted to one
function; `MarketPaused` is kept, documented as reserved for the unbuilt
pause feature (`missing-features.md §3.2`); `AuctionHistogramHeader.
accumulated_count` was confirmed *not* dead (asserted by integration tests as
an indexer-observable field) and kept as-is. The two real duplications —
`compute_fill`'s dead floor branch and `settle_maker_quote`'s separate
`classify_level` function — were closed at the root by one new shared
primitive, `clearing::fill_against_cross`, now the single fill classifier
both `settle_fill` and `settle_maker_quote` call (see
`tempo-clearing-protocol.md`). `clear_maker_quote` deactivated a quote but
never freed its rent or its PDA for re-init; a new `close_maker_quote`
instruction closes it properly. Three genuinely dead struct fields (`Vault`'s
duplicate margin/penalty bps, `Market.sync_fee_multiplier`,
`MakerQuote.sync_spread_ticks`) were deleted outright rather than kept as
unused placeholders.

### 4. Off-chain service fixes

- **4.1 No priority fees** — keeper/liquidator transactions weren't reliably
  landing on a busy cluster. Fixed: `TEMPO_PRIORITY_FEE_MICRO_LAMPORTS` wired
  into the shared tx sender across all services.
- **4.2 API watcher ran a full `getProgramAccounts` position scan on every
  400ms poll.** Moved to a separate, lower-cadence task (default 5s).
- **4.3 All `TxFailed` errors were classified benign, swallowing real
  failures.** Now inspects error content the same way the RPC-error arm
  does before deciding.
- **4.4 `liquidation_outcome` used unchecked `u128 * u128`, panicking on
  large notional.** Changed to `.saturating_mul`, matching
  `maintenance_margin`.
- **4.5 mm-bot silently swallowed all `init_position` errors.** Now uses the
  same `benign`-checked warn pattern as `init_maker_quote`.
- **4.6 mm-bot marked a quote as posted even when `post_quote` failed
  non-benignly.** `post_quote` now returns a `Result`; the "last quoted"
  marker is only set on success.
- **4.7 Liquidator cross-account resolution was fully serial**, blocking
  isolated liquidations queued behind it. Now fanned out with the same
  bounded concurrency as isolated liquidations.
- **4.8 MakerQuote sequence silently fell back to 0 on any RPC error**,
  causing a stale-sequence rejection loop. Now propagates the fetch error
  instead of defaulting.

### A note on a false alarm

An earlier review flagged `liquidate/processor.rs`'s owner payout
(`returned_to_owner = equity − penalty`) as double-debiting the owner's
collateral. It isn't: that value is already the correct absolute residual
after seizing all collateral, proven by `fuzz_liquidation_outcome_conserves`
(`returned + penalty == equity`). Not every reported issue is a real one —
this one was checked and correctly left alone.
