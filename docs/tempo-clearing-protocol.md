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

### 4.3 Liveness if the crank vanishes mid-clear 🟡
Because every phase is permissionless and the histogram is persistent on-chain state, a stalled clear can be resumed by anyone — the partial histogram and the per-order accumulated flags are already committed. No single crank is on a critical path. The failure mode is *delay* (the batch takes longer to clear), not *loss*. The protocol must define what happens to the period clock if a clear over-runs (see §6 open question).

### 4.4 Evidence in the shipped implementation 🟢
The three guarantees above are not just arguments — each maps to an on-chain mechanism and a test. The threat model is unchanged: **assume every trigger party (crank) is adversarial.**

**Order-independence (no sequencing bias).** Folding is checked integer addition into one bucket (`state/histogram.rs::fold`), so the histogram — and the price `find_cross` computes from it — is identical regardless of who folds what, in what chunking or order.
- `state/histogram.rs::test_fold_commutativity` — the same `(region, tick, qty)` multiset folded forward vs. reversed yields a byte-identical bucket region.
- `tests/integration-tests/tests/determinism.rs` — one big `process_chunk` vs. many small chunks in reversed slot order **by a different signer** produces a byte-identical `ClearingResult`.

**Anti-censorship (completeness).** `finalize_clear` refuses to publish unless `Market.accumulated_order_count == Market.active_order_count` (every active order folded exactly once); `submit_order`/`cancel_order` maintain the active count, and `process_chunk` flips each order `Resting → Accumulated` so it can't be double-folded. Because accumulation is permissionless, a censored order's owner (or anyone) can fold it themselves.
- `tests/integration-tests/tests/censorship.rs` — a non-initial signer accumulates a deliberately-skipped order, after which finalize succeeds; finalize fails `AuctionNotComplete` while any `Resting` order remains.
- `tests/integration-tests/tests/active_order_count.rs` — the completeness denominator tracks the resting set exactly across submit/cancel.

**Liveness (delay, not loss).** All intermediate state is committed on-chain, so a stalled clear is resumable by anyone; the freeze lifecycle (`start_auction`, system-design §7) keeps a round open until fully settled.
- `tests/integration-tests/tests/liveness.rs` — a crank stops mid-accumulation; a different signer resumes and completes the clear, matching an uninterrupted run.

The real audit surface is this completeness/anti-censorship bookkeeping (that `active_order_count` is correct on every path, that `Accumulated`/`Consumed` transitions are exactly-once, and that `finalize_clear` cannot run early) — not the commutative price math.

---

## 5. What this buys you, concretely

- **Per-transaction cost and persistent clearing state are O(ticks), independent of order count.** A market can accept far more orders per batch than a single-transaction clear ever could.
- **Position writes are sharded across users**, each paying their own settle cost, sidestepping the "write N accounts at once" wall and spreading the 12M-CU-per-account write budget across many different position accounts instead of one.
- **The hostile-cranker problem largely dissolves**: commutativity removes sequencing MEV; permissionless inclusion removes censorship leverage; determinism means a wrong published result is rejectable by recomputation.

---

## 6. Honest open problems and failure modes (not solved here)

These are real and I will not pretend otherwise.

1. **Histogram write-lock contention (🔴 the biggest one).** Every `process_chunk` writes the *same* histogram account, so under the 12M-CU-per-account-per-block limit, accumulation transactions for one market serialize and share that budget — the single-account write-lock trade-off, now on the clearing path. Mitigation ideas: shard the histogram into several sub-accounts (e.g. by tick range) that are summed at finalize, restoring some parallelism; or bound batch size so total accumulation fits the budget. **That a sharded histogram sums correctly and cheaply at finalize is unverified, and the real contention is unmeasured — this is what the throughput benchmark settles.**

2. **The period clock vs. clear duration (🔴).** If accumulation+discovery+settlement spans multiple slots, what happens to the next period? Options: overlapping pipelines (period N+1 accumulates while N settles), or a hard "book frozen until clear completes" lock that can stretch a period. Overlap is more complex and may have subtle correctness issues I have not worked through. Freezing is simpler but means a busy book lengthens periods, hurting the latency story. Unresolved.

3. **Settlement laziness vs. margin safety (🔴).** If users settle their own fills lazily, a user could delay claiming a *losing* fill. Positions must be updated atomically enough that margin/liquidation always sees the true post-auction state — possibly fills must be applied to a position's collateral accounting at discover time as a *pending obligation* even if the position-detail write is lazy. I have not designed this carefully; it interacts with the perp margin engine and is dangerous to get wrong.

4. **Tick range and histogram size (🟡).** `T` must cover the plausible price range at the chosen tick size. Too wide wastes space and the O(T) discover pass; too narrow can't represent extreme prices. Dynamic/centered tick windows around the oracle price are probably needed. Manageable, but a real design detail.

5. **Maker/taker in the dual auction (🟡).** This document modeled a single uniform-price cross for clarity. The actual DFBA runs *two* such auctions (bid auction and ask auction) with maker/taker segregation. The histogram method applies to each independently, and the dual structure is now **implemented and tested in the program** — orders are routed into four histogram regions by `(side, is_maker)`, `find_cross` runs once per pool, and each order settles against its own auction (`program/src/clearing.rs`, `process_chunk`, `finalize_clear`, `settle_fill`; `happy_path` LiteSVM test). What remains open is *analytical*: I have not re-run the clearing **simulations** for the full dual-auction structure with maker/taker constraints. I expect it carries over cleanly; I have not proven it via simulation.

6. **Integer-rounding dust accounting (🟢→🟡).** Rounding leaves small residual imbalances. These must be explicitly swept to the insurance fund or protocol, never left to silently break conservation. Mechanically simple but must be deliberate.

---

## 7. A note on method (and a caught bug)

My first attempt at the per-order fill allocation was wrong — it assumed all strictly-better orders fill fully without checking they collectively fit under the matched volume, which produced 729 volume-cap violations across 3,000 trials. The simulation caught it; I corrected the logic to ration each side from the best price down to exactly `V`, after which violations went to 0/5,000. I'm flagging this because it's a concrete example of why these mechanisms must be simulated and fuzzed, not reasoned about on paper — and why the eventual on-chain version needs invariant fuzzing (Trident) and an audit. The arithmetic *looks* obvious and was wrong on the first pass.

---

## 8. Bottom line

The central worry from the previous design — "whole-book clearing can't fit in an L1 transaction" — has a credible, simulation-backed answer: **represent the book as a fixed-size price histogram, accumulate into it across many cheap commutative transactions, discover the price in one bounded pass, and let users pull their own fills.** The price math is sound and tested. The commutativity gives strong hostile-cranker resistance for free.

What remains genuinely unsolved is not the clearing arithmetic but its *systems integration*: histogram write-lock contention, the period clock under multi-slot clears, and lazy-settlement vs. margin safety. Those three (all 🔴 in §6) are the real research that the throughput benchmark and a careful margin design must resolve. That's the honest line between "this is shown to work" and "this is production-ready," drawn clearly rather than blurred.
