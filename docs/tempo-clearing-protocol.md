# Tempo — The Clearing Protocol

*How to run a uniform-price dual batch auction across many transactions on Solana L1, when the whole book does not fit in one transaction.*

This is the hard core of the project. Everything else (storage, margin, funding, liquidation) is comparatively standard engineering. The central claims here were checked against small simulations before being built; results are quoted inline, and where something is unproven or risky it says so explicitly.

---

## 1. The problem restated

A uniform-price auction must, in principle, look at *every* resting order to find the one price that maximizes matched volume, then allocate fills. On Solana L1 you cannot load or write hundreds of accounts in one transaction (1.4M CU cap, 64 MiB loaded-data cap, 12M CU per-account-per-block write cap). So "clear the whole book in one instruction" does not scale.

The question this document answers: **can clearing be decomposed into many cheap transactions whose per-transaction cost and persistent state do NOT grow with the number of orders — without letting whoever triggers those transactions manipulate the result?**

The answer I arrived at is **yes**, and it rests on one mathematical fact plus one engineering consequence.

---

## 2. The key insight: clearing is an *accumulation*, not a whole-book scan 🟢 (simulation-tested)

A uniform-price clearing price is recoverable from **cumulative sums** alone. Define, over price `p`:
- Demand `D(p)` = total quantity of buy orders priced at or above `p` (falls as `p` rises).
- Supply `S(p)` = total quantity of sell orders priced at or below `p` (rises as `p` rises).
- Matched volume at `p` = `min(D(p), S(p))`.

Because `D` only falls and `S` only rises, `min(D, S)` is single-peaked. The clearing price `P*` is the crossing that maximizes matched volume.

The consequence: **you never need all orders in memory at once.** You need the *cumulative quantity at each price*. So represent the book as a **histogram over price ticks** — perps already have a fixed tick size, so the number of ticks is a bounded constant:

```
demand_at_tick[0..T]   // sum of buy quantity at each tick
supply_at_tick[0..T]   // sum of sell quantity at each tick
```

Every order, when processed, just *adds its quantity into one bucket*. After all orders are folded in, a single pass over the `T` buckets builds the cumulative curves and finds `P*`.

**The histogram size is `2 × T`, fixed — completely independent of how many orders are in the book.** That is the property that makes multi-transaction clearing possible: state and per-transaction work decouple from order count.

*Simulation result:* across 2,000 random books, the histogram method produced the identical clearing price as a brute-force reference every time (0 mismatches). Across 2,000 more runs where orders were fed in arbitrary chunk sizes simulating separate transactions, still 0 mismatches.

---

## 3. The three-phase clearing protocol

This turns one impossible instruction into three kinds of cheap, bounded transactions.

### Phase 1 — ACCUMULATE (many cheap transactions, permissionless)
The book lives as resting orders. To clear, the orders must be folded into the histogram. A permissionless crank calls `process_chunk` repeatedly; each call folds a bounded slice of orders (say K orders) into the `demand_at_tick` / `supply_at_tick` arrays, and marks those orders as "accumulated" so they can't be folded twice.

- Each `process_chunk` tx touches at most K orders and the fixed histogram → bounded CU.
- Any number of crankers can run these in parallel-ish (subject to the histogram write-lock; see §6).
- Cost per tx is constant in book size; you just need ⌈N/K⌉ of them.

**Shipped (Stage A — sharded book, `docs/plan.md`, `docs/design-decisions.md` DDR-1).** The order book is no longer one slab account with a hard ~90-order ceiling — it is `num_slab_shards` independent `OrderSlab` shard accounts, each with its own ≤90-order cap. Orders are still folded a bounded slice at a time, but now "a bounded slice" is scoped to one shard per `process_chunk` call, and shards fold *in parallel* (they are different accounts; only the shared histogram write serializes across them, and that write is now bounded to ≤`num_slab_shards` calls/round, not one per order — see `docs/bench/cu_report.md`). Submission is sharded the same way: `submit_order`/`cancel_order` touch only their own shard, so intake across shards is fully parallel (`Market` stays read-only on both paths).

**Shipped (Stage B — resting orders, DDR-2/DDR-3).** An order that fills only partially (or not at all) is no longer discarded at the end of the round — Phase 3 SETTLE re-arms it `Resting` with its reduced `remaining`, and it is folded again automatically in the *next* round's Phase 1, at no extra cost to the trader (no resubmission). Because the oracle-anchored tick window can recenter between rounds, a carried order whose fixed price falls outside the new window is classified `Marketable` (the market moved through it — fold it at the boundary tick so it fills) or `Passive` (the market moved away — leave it resting, skip it this round); see §6 item 4 and DDR-3.

**Shipped (Stage C1 — always-open submission, DDR-4).** `submit_order` no longer requires the `Collect` phase. An order submitted while a round is mid-flight is tagged for the *next* round (`arm_auction_id = current + 1`) and Phase 1 simply skips it until that round arrives — so users are never blocked from placing an order, without needing a second histogram or any extra bookkeeping at roll time.

### Phase 2 — DISCOVER (one cheap transaction, permissionless)
Once every active order is accumulated, one `finalize_clear` call does the single pass over the `T` buckets: builds cumulative `D` and `S`, finds `P*` and matched volume `V`, and computes the **per-tick allocation constants** for each side (how much volume each price tick receives, best-price-first, rationing the marginal tick). It publishes these as a small, fixed-size `ClearingResult`. Cost is O(T) — bounded by the tick count, not orders.

### Phase 3 — SETTLE (each user pays for their own fill, permissionless to trigger)
Here is the move that kills the "write N positions in one tx" problem: **the auction does not push fills to users. Each user pulls their own.** Given the published `ClearingResult`, every order can self-compute its own fill from a handful of constants, with zero cross-order coordination:

- Orders priced strictly better than the marginal fill tick: fill fully.
- Orders at the marginal tick on the rationed side: fill pro-rata by size, using published `(volume_allocated_to_tick, total_qty_at_tick)`.
- Integer floor division rounds *against* the filler; leftover dust stays with the protocol.

Each `settle_fill` tx updates exactly one position (the caller's), so the per-position write cost is paid by that user, in their own transaction, off the hot path.

*Simulation result:* per-order self-computed fills never exceeded matched volume across 5,000 random books (after one bug fix — see §7), with residual bid/ask imbalance of at most a few integer units of pure rounding dust.

---

## 4. Why this is safe against a hostile or absent crank — the part that matters most

The whole point of permissionless triggering is that you must assume the crank is adversarial. Two properties make that safe.

### 4.1 Commutativity → the crank cannot bias the price 🟢 (simulation-tested)
Folding an order into the histogram is integer addition into a bucket. Addition is commutative and associative, so **the final histogram is identical no matter which crank processes which orders, in which chunks, in which order.** A hostile crank has zero freedom over `P*` through ordering or partitioning.

*Simulation result:* 500 random permutations/partitions of the same book all produced the identical histogram (500/500).

This is the central security advantage over a naive "one cranker matches everything" design: there is no privileged sequencing position to exploit, because sequence does not affect the outcome.

### 4.2 The only residual freedom is *censorship*, so completeness must be enforced 🟡
The simulations also showed the sharp edge: **dropping even one order changes `P*` about 19% of the time** (96/500). So the one thing a hostile crank *could* do is refuse to accumulate certain orders. Defenses:

- Orders rest in the book account(s) before clearing begins; `finalize_clear` must refuse to run unless a verifiable counter shows **every active order has been accumulated exactly once** (e.g. an accumulated-count that must equal the active-order count, plus per-order "accumulated in batch B" flags preventing double-fold and skip).
- Because *anyone* can call `process_chunk`, a censored order's owner (or any honest party) can always accumulate it themselves. Censorship requires excluding an order that someone else can simply include. The crank cannot finalize around a still-unaccumulated order.

🟡 I'm confident in the commutativity result and the shape of the completeness defense, but the exact bookkeeping that *guarantees* "every active order, exactly once, no double-count, no skip" across many concurrent permissionless `process_chunk` calls is fiddly and is where an auditor must focus. This is the real security surface — not the price math.

**Shipped, and the bookkeeping choice matters (Stage A — Design Z, DDR-1).** Once the book is sharded (§3), "a verifiable counter" cannot mean a single `Market`-level aggregate updated by every `submit_order`/`cancel_order`/`process_chunk` call — that was tried first and rejected: keeping the counter correct forced `submit_order` to write-lock `Market` on every shard's first order, which serialized intake across *all* shards and defeated the point of sharding, and a single missed decrement (found in `cancel_order`) permanently wedged `finalize_clear` behind a stale nonzero counter. The shipped design instead re-derives completeness live: `finalize_clear` takes **every shard account** as a trailing account and calls the authoritative per-shard scan (`all_active_orders_accumulated`) on each one — the exact same kind of scan the single-slab design always did, just repeated per shard instead of once. There is no `Market`-level completeness counter to drift. This keeps the auditable property from this section intact — "every active order, exactly once" is still a live, recomputable fact, not a cached one — while restoring the parallel-intake benefit sharding was built for. See `docs/design-decisions.md` DDR-1 for the two rejected alternatives and why.

### 4.3 Liveness if the crank vanishes mid-clear 🟡
Because every phase is permissionless and the histogram is persistent on-chain state, a stalled clear can be resumed by anyone — the partial histogram and the per-order accumulated flags are already committed. No single crank is on a critical path. The failure mode is *delay* (the batch takes longer to clear), not *loss*. The protocol must define what happens to the period clock if a clear over-runs (see §6 open question).

### 4.4 Evidence in the shipped implementation 🟢
The three guarantees above are not just arguments — each maps to an on-chain mechanism and a test. The threat model is unchanged: **assume every trigger party (crank) is adversarial.** This now covers the sharded, resting-order, always-open design (Stages A/B/C1 — `docs/plan.md`, `docs/design-decisions.md`), not just the original single-slab version.

**Order-independence (no sequencing bias).** Folding is checked integer addition into one bucket (`state/histogram.rs::fold`), so the histogram — and the price `find_cross` computes from it — is identical regardless of who folds what, in what chunking or order. Sharding the book does not change this: every shard folds into the *same* histogram, so the commutativity argument applies across shards exactly as it did within one slab.
- `state/histogram.rs::test_fold_commutativity` — the same `(region, tick, qty)` multiset folded forward vs. reversed yields a byte-identical bucket region.
- `tests/integration-tests/tests/determinism.rs` — one big `process_chunk` vs. many small chunks in reversed slot order **by a different signer** produces a byte-identical `ClearingResult`.
- `tests/integration-tests/tests/sharding.rs::orders_route_to_distinct_shards_and_fold` — orders submitted to different shards land in their own slab accounts and still fold into one shared histogram (`accumulated_count` sums across shards).

**Anti-censorship (completeness).** `finalize_clear` refuses to publish unless every active order has been folded exactly once. This is now proven per-shard rather than via a single `Market`-level counter (Design Z / DDR-1 — see §4.2): `finalize_clear` takes every `OrderSlab` shard as a trailing account and calls `all_active_orders_accumulated` on each one; `process_chunk` flips each order `Resting → Accumulated` so it can't be double-folded, and `OrderSlabHeader.count` (not a separate `Market.active_order_count` mirror — PERF-1 removed that) is the single source of truth for how many orders are still live in a shard. A resting order that's legitimately unfoldable this round (its price is outside the recentered window and passive, or it was submitted mid-round and is armed for the next one) is *exempt*, not silently skipped — see §6 item 4 and DDR-3/DDR-4. Because accumulation is permissionless, a censored order's owner (or anyone) can fold it themselves.
- `tests/integration-tests/tests/censorship.rs::skipped_order_blocks_finalize_until_a_different_signer_includes_it` — a non-initial signer accumulates a deliberately-skipped order, after which finalize succeeds; finalize fails `AuctionNotComplete` while any `Resting` order remains.
- `tests/integration-tests/tests/active_order_count.rs::counts_track_resting_orders_through_submits_and_cancels` — the slab's own live-order count tracks the resting set exactly across submit/cancel (no separate Market mirror to drift).
- `tests/integration-tests/tests/sharding.rs::nonempty_unfolded_shard_still_blocks_finalize` / `::empty_shards_do_not_block_finalize` / `::finalize_rejects_short_or_duplicate_shard_set` — a single unfolded order anywhere across K shards blocks finalize; an empty shard never does; and a hostile caller cannot dodge the scan by passing a short or duplicated shard list.
- `tests/integration-tests/tests/phase_guards.rs::finalize_fails_when_chunk_skipped` — the general phase-guard case, unchanged by sharding.

**Liveness (delay, not loss).** All intermediate state is committed on-chain, so a stalled clear is resumable by anyone; the freeze lifecycle (`start_auction`, system-design §7) keeps a round open until fully settled. Sharding adds a second dimension to this: a stalled *shard* (a `process_chunk` or `reset_shard` nobody has run yet) doesn't need the market's original cranker either.
- `tests/integration-tests/tests/liveness.rs` — a crank stops mid-accumulation; a different signer resumes and completes the clear, matching an uninterrupted run.
- `tests/integration-tests/tests/sharding.rs::submit_then_cancel_does_not_wedge_clearing` / `::multi_shard_round_rolls_after_all_shards_reset` / `::multi_shard_force_reset_recovers_and_next_round_rolls` — a submit+cancel never leaves a shard in a state that blocks the round; a multi-shard round rolls once every shard is reset; the authority escape hatch (`force_reset`) recovers a stuck multi-shard round and the next roll still succeeds.
- `tests/integration-tests/tests/wedge.rs::start_auction_rolls_empty_discovered_round` / `::start_auction_refuses_discovered_with_orders` — an order-less round still rolls (doesn't need a `Settling` phase nobody triggered); a round with unsettled orders correctly refuses to roll.

**Resting orders don't reopen the completeness gate (Stage B, DDR-2/DDR-3).** A carried-over `Resting` order is unfolded at the start of its new round, so it must be re-folded before `finalize_clear` will run — the censorship guarantee is unchanged by carrying orders across rounds.
- `tests/integration-tests/tests/resting_orders.rs::carried_resting_order_blocks_finalize_next_round` / `::roll_gate_rejects_unsettled_then_carries_resting` / `::partial_fill_rests_then_completes_conserving` — a carried order blocks finalize until re-folded; the roll gate rejects a shard with any still-`Accumulated` (unsettled) order; a partial fill's quantity conserves exactly across the two rounds it takes to complete.
- `tests/integration-tests/tests/resting_orders.rs::passive_resting_order_parks_then_folds_when_window_returns` / `::marketable_resting_order_folds_after_recenter` — the DDR-3 window-move classification, end to end: a passive order parks without blocking finalize and folds once the window returns over it; a marketable order (the market moved through it) folds and fills after the window recenters past it.
- `tests/integration-tests/tests/resting_orders.rs::expired_resting_order_is_consumed_not_rearmed` / `::permissionless_reap_boundary_is_strict_less_than` / `::submit_of_already_expired_order_is_rejected` — an expired order leaves the book instead of re-arming; anyone may reap a *past* (not still-live) expired order; an already-expired order is rejected at submit.

**Always-open submission doesn't let a mid-round order jump the queue (Stage C1, DDR-4).**
- `tests/integration-tests/tests/phase_guards.rs::submit_after_accumulating_arms_next_round` — an order submitted after `Collect` has closed is accepted, does not fold into or block the in-flight round, and is eligible starting the next one. `::cancel_is_always_open_symmetric_with_submit` confirms cancellation has the same always-open property.

The real audit surface is this completeness/anti-censorship bookkeeping — now specifically that the per-shard scans in `finalize_clear`/`reset_shard` are correct on every path, that `Accumulated`/`Consumed`/`Resting` transitions are exactly-once and exactly-correct across a round boundary, that the window-move exemption (DDR-3) and the always-open exemption (DDR-4) can't be abused to permanently hide a foldable order, and that `finalize_clear` cannot run early — not the commutative price math, which is unchanged by any of Stages A/B/C1.

---

## 5. What this buys you, concretely

- **Per-transaction cost and persistent clearing state are O(ticks), independent of order count.** A market can accept far more orders per batch than a single-transaction clear ever could.
- **Position writes are sharded across users**, each paying their own settle cost, sidestepping the "write N accounts at once" wall and spreading the 12M-CU-per-account write budget across many different position accounts instead of one.
- **The hostile-cranker problem largely dissolves**: commutativity removes sequencing MEV; permissionless inclusion removes censorship leverage; determinism means a wrong published result is rejectable by recomputation.

---

## 6. Honest open problems and failure modes (not solved here)

These are real and I will not pretend otherwise.

1. **Histogram write-lock contention — 🟡 bounded and measured, not eliminated.** Every `process_chunk` still writes the *same* histogram account, but the order book itself is no longer one account either: Stage A (`docs/plan.md`, DDR-1) shards the *order book* into `num_slab_shards` independent `OrderSlab` accounts that fold *in parallel* into the one shared histogram, so the number of accumulation transactions per round is now bounded by the shard count, not the order count — a market with 10 orders in each of 16 shards still only needs (up to) 16 `process_chunk` calls touching the histogram, each folding many orders at once. **What this does and doesn't fix:** submission and settlement are now genuinely parallel (they touch only their own shard, never the histogram or `Market`), and the histogram's own contention window shrinks from O(orders) transactions to O(shards) transactions. The histogram account itself is *not* sharded — a sharded-histogram-summed-at-finalize design (the idea floated here originally) was not needed once the order book itself was sharded, and remains unbuilt. `docs/bench/cu_report.md` (vs. the pre-shard baseline `cu_report_pre_shard.md`) is the real measurement this section used to call for: at the dev target of 16 shards × 90-order cap, `finalize_clear`'s per-shard completeness scan (the other O(shards) cost, see §4.2/§4.4) adds ~3,603 CU/shard on top of the O(ticks) discovery pass, landing at ~160,542 CU total — 11.5% of the 1,400,000 CU/tx cap, comfortably inside budget at that target.

2. **The period clock vs. clear duration — 🟡 the submit-side dead time is fixed; processing overlap is still open.** Stage C1 (always-open submission, DDR-4) removes the "book frozen, can't submit" problem for *users*: `submit_order` is accepted in any phase, an order submitted mid-round is simply deferred to the next round. What Stage C1 does **not** do is let two rounds *process* (accumulate/discover) at once — there is still exactly one histogram and one `ClearingResult` per market, so round N+1 cannot accumulate while round N is still settling. That true overlap (Stage C2 — double-buffer the histogram/result by round parity) is designed on paper (`docs/plan.md` §4.2) but deliberately not built, gated behind a benchmark showing Stage A + C1 throughput is actually insufficient (see `docs/known-issues.md` §2.14). The freeze model otherwise still applies: a slow clear lengthens that round, it does not corrupt anything, and anyone can keep cranking it to completion.

3. **Settlement laziness vs. margin safety — 🟢 addressed by the resting-order margin design (Stage B, DDR-2/DDR-3).** The original worry — a user could delay claiming a *losing* fill, or a position's margin could be computed against stale numbers — is resolved by the combination of: (a) margin is reserved **at submission time** against a fixed worst-case price snapshot (`Order.worst_price`, missing-features §1.1), so the money needed for the worst outcome of a fill is locked before the auction ever clears, not discovered after; (b) `settle_fill`'s re-lock of a resting order's leftover margin is designed to **never revert** (`UserCollateral::lock_up_to`) — a trade that already cleared can never be "un-filled" to fix a collateral shortfall, so if a window move leaves an order under-margined, the fill still settles and the resulting position is left for the ordinary liquidation backstop to handle, rather than the settle transaction reverting and wedging the round. This does not remove liquidation risk (a resting order the market gapped through can end up thinly margined), but it removes the *correctness* hazard — settlement is never blocked or corrupted by lazy claiming.

4. **Tick range and histogram size — 🟢 resolved (shipped, not just designed).** `T` (the tick count) is still a fixed constant per market, but the *window* it covers is no longer pinned at genesis: `Market.window_floor_price` anchors tick 0, and `start_auction` re-centers it on the current oracle price at every round roll (`window_floor = oracle − (num_ticks/2)·tick_size`, snapped to the tick grid), frozen for the round in between so mid-round tick↔price mapping never moves. This is exactly the "dynamic/centered tick window" this section used to flag as probably needed. The remaining open question is what happens to a *resting* order whose price the window recenters past — that's Stage B's `classify_resting_fold` (marketable-fill/passive-park), covered under item 3 above and DDR-3, not a histogram-sizing problem anymore.

5. **Maker/taker in the dual auction (🟡).** This document modeled a single uniform-price cross for clarity. The actual DFBA runs *two* such auctions (bid auction and ask auction) with maker/taker segregation. The histogram method applies to each independently, and the dual structure is now **implemented and tested in the program** — orders are routed into four histogram regions by `(side, is_maker)`, `find_cross` runs once per pool, and each order settles against its own auction (`program/src/clearing.rs`, `process_chunk`, `finalize_clear`, `settle_fill`; `happy_path` LiteSVM test). What remains open is *analytical*: I have not re-run the clearing **simulations** for the full dual-auction structure with maker/taker constraints. I expect it carries over cleanly; I have not proven it via simulation.

6. **Integer-rounding dust accounting (🟢→🟡).** Rounding leaves small residual imbalances. These must be explicitly swept to the insurance fund or protocol, never left to silently break conservation. Mechanically simple but must be deliberate.

---

## 7. A note on method (and a caught bug)

My first attempt at the per-order fill allocation was wrong — it assumed all strictly-better orders fill fully without checking they collectively fit under the matched volume, which produced 729 volume-cap violations across 3,000 trials. The simulation caught it; I corrected the logic to ration each side from the best price down to exactly `V`, after which violations went to 0/5,000. I'm flagging this because it's a concrete example of why these mechanisms must be simulated and fuzzed, not reasoned about on paper — and why the eventual on-chain version needs invariant fuzzing (Trident) and an audit. The arithmetic *looks* obvious and was wrong on the first pass.

---

## 8. Bottom line

The central worry from the previous design — "whole-book clearing can't fit in an L1 transaction" — has a credible, simulation-backed answer: **represent the book as a fixed-size price histogram, accumulate into it across many cheap commutative transactions, discover the price in one bounded pass, and let users pull their own fills.** The price math is sound and tested. The commutativity gives strong hostile-cranker resistance for free.

What remained unsolved at that point was not the clearing arithmetic but its *systems integration*: histogram write-lock contention, the period clock under multi-slot clears, and lazy-settlement vs. margin safety. Those three (§6, then all 🔴) have since been substantially closed by the scaling work in `docs/plan.md`: **sharding the order book** (Stage A, DDR-1) bounds the histogram/completeness write contention to the shard count and is measured in `docs/bench/cu_report.md`; **resting orders with a fixed margin snapshot and a never-revert re-lock** (Stage B, DDR-2/DDR-3) closes the lazy-settlement-vs-margin-safety gap; **always-open submission** (Stage C1, DDR-4) removes the period-clock dead time for users, though true round-*processing* overlap (Stage C2) remains a deliberately deferred stretch goal, not yet needed by measurement. The honest line between "this is shown to work" and "this is production-ready" has moved: the clearing arithmetic and its systems integration are now both implemented and tested (LiteSVM + devnet); genuinely open items are narrower and tracked in `docs/known-issues.md` (§2.14 Stage C2, §2.13 devnet validation depth) rather than being open research questions about the core design.
