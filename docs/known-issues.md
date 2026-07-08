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

Five items remain. None is a bug in a normal-operation path: two are accepted
limitations of the authority-only emergency/edge paths, one is a recorded
design decision, one is a live-validation coverage gap, and one is a
regenerate-the-vendored-bundle operational chore. Everything else has been
fixed — see Part B.

| Item | Area | Status |
| --- | --- | --- |
| 2.12 Web vendor client bundle lags the on-chain layout | operational | Regen `apps/web/src/vendor/tempo-client.mjs` for the current Market v13 before any web-driven devnet run |
| 2.13 Stage-B marketable-fill not yet validated on a LIVE chain | design | LiteSVM-covered (`marketable_fill.rs`); devnet scenario still pending |
| 2.14 Stage-C2 (true round-processing overlap) not built | design | **Decided NO-GO** — recorded in `docs/bench/round_latency.md`; re-open trigger stated |
| 5.1 `force_reset` strands reserved margin + orphans folded maker quotes | design | Accepted — authority-only emergency escape hatch; documented fund-destructive |
| 5.2 Cross-liquidation can revert when the only closable leg is profitable | design | Accepted — liquidator picks member ordering; hard wedge only if every leg wins AND insurance is dry |

### 2.12 Web vendor client bundle can lag the on-chain layout — [operational]

The generated clients (`clients/typescript/src/generated/`,
`crates/sdk/src/generated/`) are regenerated in lockstep with every layout
change and checked by CI. But `apps/web/src/vendor/tempo-client.mjs` is a
**vendored bundle** that does not participate in the "clients-fresh" check, so
it can silently lag — under-encoding `initialize_market` or decoding `Market`
at stale offsets. The current on-chain layout is **Market v13**.

**To close:** `pnpm generate-clients && pnpm bundle-client`, confirm the vendor
bundle carries the current fields, and (durably) extend the CI clients-fresh
job to also rebuild + diff the web vendor bundle so it can never drift again.
The web UI is otherwise deferred, so this only blocks a web-driven devnet run.

### 2.13 Stage-B marketable-fill not yet validated on a LIVE chain — [design]

A **coverage gap**, not a defect. When the oracle-anchored tick window
recenters past a fixed-price resting order, `classify_resting_fold` folds a
*marketable* order at the boundary tick so it fills (DDR-3). Both halves are
now covered in LiteSVM — the passive-park half in `resting_orders.rs` and the
**fill** half (recenter → fold-at-boundary → cross → settle at the clearing
price, exact margin/position/OI) in `marketable_fill.rs`. What remains is a
**devnet** scenario run against the deployed binary (LiteSVM cannot catch CU or
account-limit surprises).

**To close:** a `crates/sim` scenario that recenters the window through a live
resting order and confirms the fill on-chain. See DDR-3 in
`docs/design-decisions.md`.

### 2.14 Stage-C2 (true round-processing overlap) not built — [design] — DECIDED: NO-GO

Stage C1 (always-open submit) removed the *submit* dead-time, but round
*processing* is still serial: one histogram, so round N+1 can't accumulate
while round N settles. C2 would double-buffer the histogram + `ClearingResult`
by round parity for true overlap.

**The benchmark ran (P5.1) and the decision is recorded in
`docs/bench/round_latency.md`: do not build C2 now.** Over 48 live devnet
rounds the settle+reset+roll tail is 55 % of the ~105 s round — but the CU
model shows that even at 1,440 orders/round the whole tail is ~2.5
Market-write blocks (~1 s of chain capacity): the measured tail is
per-transaction **confirmation latency at trivial load**, not throughput. C2
(the riskiest change in the scaling plan — two live rounds over one durable
book) would buy at best a -35 % cadence gain that keeper-side settle batching
can match with zero on-chain risk. **Re-open trigger:** a loaded run showing
the tail CU-bound (wall-clock tracking `Σ settle CU / 12M-per-block`, not
`orders × confirm-latency / concurrency`) after keeper batching exists.

### 5.1 `force_reset` strands reserved margin + orphans folded maker quotes — [design]

Surfaced in the adversarial review. `force_reset` (authority-only, disc 15) is
the emergency escape hatch for a genuinely wedged round: it force-zeroes every
shard's slot region and re-opens `Collect`. It does **not** release any order's
`reserved_margin` back to `UserCollateral.locked`, so an order that was
`Resting` with a live reservation is erased with no accounting release — the
owner's locked collateral is stranded (there is no order left to `cancel_order`
against). Symmetrically, it zeroes the histogram without requiring folded maker
quotes to settle, orphaning their counter-positions. Both are inherent to the
"nuke a stuck round" semantics; the instruction is authority-gated and
documented "NOT a normal path." **Accepted** as a fund-destructive last resort;
a future version could summed-release reservations (it would need every affected
ledger passed as accounts). The normal roll path is fully safe — the v13
maker-quote settle gate and the per-order reservation accounting cover it.

### 5.2 Cross-liquidation can revert when the only closable leg is profitable — [design]

Surfaced in the adversarial review. `liquidate_cross` closes the *first
non-flat* supplied member. Closing a winning leg realizes a positive PnL, which
`conserve_and_socialize` draws from insurance — returning `InsuranceInsolvent`
if the pool is short. An account can be combined-unhealthy with *every* leg
profitable (high leverage: `Σ maintenance > equity` despite gains), so if the
pool is also dry, every member ordering reverts and the account is temporarily
un-liquidatable. **Accepted** (LOW): the liquidator supplies the member
ordering and can almost always pick a losing leg first; a hard wedge needs the
rare conjunction of an all-winning underwater book AND an empty insurance pool.
Fund-conserving (reverts, no bad settle). A future version could pick the
worst leg on-chain rather than trusting caller ordering.

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

### 5. Lifecycle / scaling items closed during the plan

- **2.11 The collateral ledger requirement on a releasing settle was
  under-documented.** `SettleMoney::for_order_owner` (`crates/sdk/src/ix.rs`)
  now derives and attaches the owner's mint-scoped ledger unconditionally on
  money-path markets, so a fully-consuming settle can never fail
  `MissingSettleAccounts` from a client omission
  (`settle_builder_always_attaches_ledger`).
- **2.15 Keeper opened the next `Collect` late (P5.2).** `engine::decide` now
  pipelines the roll with the settle tail — drained shards `reset_shard`
  concurrently with the remaining settles, and `shards_ready == num_slab_shards`
  rolls with a single `start_auction` (no reset pass).
- **4.9 One `MakerQuote` PDA per maker (Phase 2).** `quote_index` is now a
  fourth PDA seed (`MAX_QUOTES_PER_MAKER = 4`), so a maker can run up to four
  concurrent ladders.
- **4.10 `benign()` used fragile string-matching (P5.3).**
  `crates/sdk/src/retry.rs` now parses the numeric custom error code and matches
  an explicit `BENIGN_CODES` allowlist; a coded error not on the list is a REAL
  error (the old rule swallowed every "custom program error"). The substring
  matcher survives only for code-less transport races.

### 6. Adversarial-review fixes (post-plan)

A five-dimension independent review of the whole plan diff. One HIGH from the
money-path pass, one HIGH + three MEDIUM + one LOW from the on-chain passes;
all fixed (or accepted-and-documented above as 5.1/5.2). The off-chain pass
found no correctness bugs.

- **[HIGH] Insurance-withdraw recipient theft.** `apply_insurance_withdraw` is
  permissionless and the proposal stored no destination, so anyone could
  front-run the authority's apply and redirect the staged pool withdrawal to
  their own same-mint account (user backing stayed intact, so no gate fired).
  Fixed: the recipient must now be owned by `Vault.authority`, so funds can only
  land where the delay-gated authority controls them.
- **[HIGH] Maker-quote settle completeness (Market v13).** `start_auction`
  gated only on the taker slab; a folded-but-unsettled maker quote was orphaned
  at roll, leaving the takers' matched side uncovered — a conservation break a
  hostile maker could force. Fixed with `settled_maker_quote_count`, gated
  symmetric to the taker settle gate (`review_fixes.rs`).
- **[MEDIUM] `PAUSE_ROLL` was a no-op.** `start_auction` now checks
  `require_not_paused(PAUSE_ROLL)`, so the documented wind-down control works.
- **[MEDIUM] `crank_fee` was unbounded.** An insurance outflow settable
  instantly via `update_market_params`, so a malicious authority could sweep the
  pool in one finalize, bypassing the staged-withdraw delay. Bounded now
  (`MAX_CRANK_FEE`) in the shared validator.
- **[MEDIUM] Partial liquidation reverted on large accrued realized PnL.** The
  code flushed ALL realized to the free ledger, leaving the isolated remainder
  underwater though the account was healthy → un-liquidatable until a full
  close. Fixed to flush only the closed slice and keep the pre-existing realized
  backing the remainder (`partial_liquidation_survives_large_accrued_realized`).
- **[LOW] `MAX_SLAB_SHARDS` 256 → 64.** `force_reset`/`close_market` dedup a
  `u64` shard mask, so a shard id ≥ 64 made a market un-closeable (rent
  stranded); the cap now matches the mask width.

### A note on a false alarm

An earlier review flagged `liquidate/processor.rs`'s owner payout
(`returned_to_owner = equity − penalty`) as double-debiting the owner's
collateral. It isn't: that value is already the correct absolute residual
after seizing all collateral, proven by `fuzz_liquidation_outcome_conserves`
(`returned + penalty == equity`). Not every reported issue is a real one —
this one was checked and correctly left alone.
