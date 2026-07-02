# Design Decision Records

Short, dated records of non-obvious architecture choices, kept so they can be re-reviewed
later. Newest first.

---

## DDR-3 — Resting orders vs. the moving tick window (marketable-fill / passive-park)

**Status:** decided 2026-07-02 · to be implemented on `feat/sharded-book` (Stage B fix batch) ·
fixes the two blockers from the high-effort code review of commit `5c6c63c`.

### Context / the problem

Stage B (DDR-2) lets an unfilled/partial order **rest across rounds carrying a fixed `price`**.
But the histogram's tick window is **oracle-anchored and recenters every roll** (`start_auction`
→ `recenter_window`, so `window_floor = oracle − (num_ticks/2)·tick_size`). The two facts
collide:

1. **Permanent market wedge (blocker).** Once the oracle moves so a resting order's fixed price
   leaves `[window_floor, window_floor + num_ticks·tick_size)`, next round's `process_chunk`
   calls `price_to_tick_raw(order.price, …)?`, which **hard-errors** (`InvalidPrice`/`InvalidTick`).
   The order can never fold → `all_active_orders_accumulated` never returns true → `finalize_clear`
   is blocked **forever**, market-wide. Any participant can trigger it on purpose with one dust
   resting order + a normal oracle move → a permissionless denial-of-service on the whole market.
2. **`settle_fill` re-lock revert.** The DDR-2 margin split releases the filled slice at the
   *stale* submit-time `worst_price`, but re-locks the leftover position at the *actual* clearing
   price; after an upward recenter the re-lock can need more than was released and `lock()` reverts
   → the order sticks `Accumulated` → the roll gate wedges.

### The economic insight

An off-window resting order is not an error — it is a limit order the market has moved relative to,
and a limit order has a well-defined meaning by side:

- **Marketable** (the market moved *through* it): a **SELL below the window floor** or a **BUY
  above the window top**. The trader's limit is already satisfied by every in-window price → it
  **should fill** at the clearing price (strictly better than its limit).
- **Passive** (the market moved *away* from it): a **SELL above the top** or a **BUY below the
  floor**. It **should keep resting** until the window slides back over it, or it expires.

### Options considered

- **Auto-expire any off-window order at roll.** Simpler completeness logic, but *wrong*: it throws
  away marketable orders that were owed a fill and silently kills passive orders that would return
  one round later; also needs the owner's collateral account at permissionless-crank time to
  refund margin (the cranker doesn't hold it). Rejected.
- **Widen / freeze the window.** Doesn't fix it — any larger move still wedges, and it bloats
  O(ticks) every round. Rejected.
- **Minimal "skip both, never crash."** Prevents the wedge (fold-skip off-window orders; don't let
  passive ones block finalize) but leaves *marketable* orders stranded, unfilled, until the market
  happens to return — a real fairness/correctness bug for urgent exits. Rejected as a stopping
  point (it is the safe subset of the chosen fix).
- **Marketable-fill + passive-park (CHOSEN).** Classify each off-window resting order by side:
  fold marketable ones at the boundary tick so they fill; skip passive ones and exempt them from
  the completeness gate; expiry is the eventual GC for a passive order the market never revisits.

### Decision: **marketable-fill + passive-park**

### How it stays safe (DDR-1 censorship guarantee holds)

The classification `in-window | marketable | passive` is a **pure, deterministic function of
`(order.price, order.side, window_floor, tick_size, num_ticks)`** — all on-chain, recomputable by
anyone. `finalize_clear`'s completeness gate becomes: *every resting order that is in-window OR
marketable must be folded; only passive-out-of-window orders may remain `Resting`.* A malicious
crank cannot reclassify an order (the window is fixed by the oracle at roll), so completeness stays
authoritative and commutative — **no new trust, no Market aggregate counter** (Design Z intact).

### The margin coupling (fixes blocker 2)

Margin only ever changes where the owner's collateral account is present: **passive parked orders
do not fill → no margin change** (and their reservation, taken against the submit-window worst
case, still covers them). **Marketable orders fill in `settle_fill`**, where the account *is*
present, so the re-lock is handled there. To guarantee `settle_fill` **never reverts** (which would
wedge the roll): the leftover reservation is held at the exact `initial_margin(remaining,
worst_price)`, and when the resulting position's fresh margin at the clearing price exceeds the
released amount, the difference is locked from the owner's **free** collateral; if free collateral
is insufficient, the fill still settles (the trade cleared — conservation forbids un-filling it)
and the resulting position is left **immediately liquidatable** for the existing risk backstop,
rather than reverting the crank. Accepted consequence: a resting order the market gapped through
can open a position slightly under initial margin, cushioned by its reserved margin and resolved by
liquidation — liveness is never sacrificed for the initial-margin gate.

### Consequences / re-review triggers

- `process_chunk` and the completeness gate now need the window params (small, already on `Market`).
- Marketable folds land at a boundary tick — a deliberate conservative representation (the order is
  willing to trade at any in-window price); it fills at the uniform clearing price like any other.
- Re-review if a future stage lets orders rest with **per-order price bands** or non-oracle windows,
  or if the "fill-then-liquidate" under-margin path proves exploitable under an adversarial oracle
  (it should not be — the mover doesn't control the oracle and forfeits reserved margin).

---

## DDR-2 — Stage B resting orders: the roll gate + the partial-fill margin split

**Status:** decided 2026-07-01 · implemented on `feat/sharded-book` (Stage B) · `OrderSlabHeader`
VERSION 4→5, `Order` `ORDER_LEN` 88→104 · resolves the DDR-1 Stage-B re-review trigger.

### Context / the problem

Stage B lets an unfilled/partial order **carry across rounds** (place-once UX). `settle_fill`
now re-arms a not-fully-filled, non-expired order back to `Resting` (reduced `remaining`,
`cum_before = 0`) instead of `Consumed`. Two things this breaks if handled naïvely:

1. **The roll gate.** `reset_shard` gated the roll on `count == 0` (shard drained). With resting
   orders `count` never reaches 0 — survivors stay in the book — so draining-to-empty would
   **wedge the roll forever**.
2. **Margin on a partial fill.** The pre-Stage-B path released the order's *entire* worst-case
   reservation at settle (the leftover was discarded). Now the leftover keeps resting and still
   needs margin, so the release must be *split* between the filled slice and the carried leftover.

### Decisions

- **Roll gate = "no order still `Accumulated`", scanned authoritatively.** `reset_shard` now
  refuses unless `all_accumulated_orders_settled(shard)` (no slot is still `Accumulated`). Settle
  turns each folded order into `Consumed` (leaves) or `Resting` (re-armed), so "no `Accumulated`"
  is the exact "fully settled" test. It keeps `Resting` survivors in place (frees only
  `Consumed`/`Empty` slots) and recomputes `count`/`resting_count` from the compacted shard.
  `reset_shard` + `shards_ready` + the all-shards `force_reset` are **kept** (not dropped); only
  the gate + the keep-resting compaction changed. This mirrors DDR-1's finalize gate
  (`all_active_orders_accumulated`): an authoritative per-shard scan, **no new `Market` counter**.
  The keeper already keys its roll decision on `accumulated_orders().is_empty()`, so it matches
  the on-chain gate with no keeper change. `next_order_id` is **not** reset at roll (kept
  monotonic) so a new order can never reuse a surviving order's id.

- **Partial-fill margin split = release the filled slice's own worst-case margin.** On a re-arm,
  release `min(reserved, initial_margin(fill, worst_price))` and keep the remainder locked on the
  order. Because that release ≥ the filled position's `initial_margin(fill, clearing_price)` lock
  (worst_price ≥ clearing price), settle **can never revert on the position lock** — the "settle
  only nets a release" invariant is preserved even though the leftover now keeps a reservation.
  The leftover's carried reservation is thus at most ~1 base unit below its standalone worst case
  (ceil rounding, conservative in the safe direction — never a shortfall on a live position). A
  fixed `worst_price` **snapshot on the order** keeps a resting order margin-stable as the
  oracle-anchored window moves (a resting sell can't silently become under-margined).

### DDR-1 re-review trigger — resolved

DDR-1 flagged: *"Stage B changes the fold/roll model — confirm the authoritative per-shard
finalize scan still holds and carried-over Resting orders are re-counted correctly."* Confirmed:
a carried `Resting` order is unfolded at the start of the next round, so `finalize_clear`'s
per-shard `all_active_orders_accumulated` scan **blocks finalize until it is re-folded** — the
censorship guarantee is unchanged. Pinned by
`resting_orders::carried_resting_order_blocks_finalize_next_round`.

### Re-review triggers

- Stage C1 (always-open submit) adds `arm_auction_id`: the roll gate must additionally NOT count
  orders armed for a *future* round as unsettled — revisit `all_accumulated_orders_settled`.
- If a benchmark shows the per-shard settle/roll scan (`O(cap)` each) is a cost as `cap` grows.
- The per-shard `MAX_ORDERS_PER_TRADER` cap is now a *standing* per-shard cap; a global standing
  cap (across shards) is deferred — revisit if per-shard routing proves insufficient.

---

## DDR-1 — Stage A shard completeness: drop the counter (Design Z)

**Status:** decided 2026-07-01 · implemented on `feat/sharded-book` in commit `a499114`
(Market VERSION 10→11) · **flagged for re-review** (triggers below).

### Context / the problem

Sharding splits the order book into `num_slab_shards` `OrderSlab` accounts so submits/settles
run in parallel and we break the 128-order single-account cap. But clearing must still prove
**every order in every shard has been folded** before `finalize_clear` runs (the censorship /
completeness guarantee) — and scanning all shards' *orders* at finalize was assumed too slow.

The first implementation added an **aggregate counter** on the `Market` account
(`shards_pending` = number of shards still holding unfolded orders). This caused two problems:

1. **Killed parallel intake (the point of sharding).** To keep the counter correct,
   `submit_order` had to *write* `Market` (first order into a shard: `shards_pending += 1`).
   Solana write-locks a writable account for the whole tx regardless of mutation, so **every
   submit across every shard serialized on `Market` again** — reproducing the exact contention
   sharding was meant to remove. Sharding kept the *size-cap* win but lost the *parallel-intake*
   win.
2. **Counter drift = wedge bugs.** A separate mirror counter drifts from reality. The code review
   found `cancel_order` forgot to decrement it → `shards_pending` stuck > 0 → `finalize_clear`
   reverts `AuctionNotComplete` forever → a single permissionless submit+cancel **wedged the
   market**. (This is the same lesson as PERF-1, which removed Tempo's earlier mirror counters
   for exactly this reason.)

### Options considered

- **X — keep the counter as-is.** submit write-locks `Market` (serializes); keeps the drift-prone
  counter. Simplest keeper. Rejected: loses parallel intake and keeps the bug class.
- **Y — counter, but keeper cranks empties.** `shards_pending = num_slab_shards`; submit stays
  `Market`-read-only; the keeper must `process_chunk` **every** shard each round (incl. empty
  ones) so the counter reaches 0. Restores parallelism and scales to huge K, but keeps a counter
  and adds a per-shard crank obligation.
- **Z — drop the counter (CHOSEN).** No `Market.shards_pending`. `finalize_clear` takes **all K
  shard accounts** as trailing accounts and authoritatively checks each is fully folded
  (`all_active_orders_accumulated` per shard) — the same scan the single-slab design did, now
  per shard. submit/cancel touch **only their own shard** (`Market` read-only again).

### Decision: **Z**

### Rationale

1. **Restores parallel intake** — `submit_order` / `cancel_order` are `Market`-read-only again, so
   submits to different shards run in parallel (the headline Stage-A benefit).
2. **Eliminates the whole counter-drift bug class** — no aggregate to keep in sync, so bugs like
   the cancel-wedge cannot recur. (Applies the PERF-1 lesson: authoritative state, not mirrors.)
3. **Authoritative & cheap enough** — finalize is one tx, off the hot path. Scanning K shards ×
   cap orders at the dev target (16 × 90 = 1,440 reads ≈ ~50–70k CU) sits comfortably under the
   1.4M CU/tx limit on top of the ~93k-CU cross.

### Consequences / trade-offs

- `finalize_clear` now carries **all K shard accounts** in its account list. Fine for the dev
  target and up to **~40–50 shards/market** (Solana's per-tx account limit; address-lookup tables
  can raise it). Beyond that, finalize must chunk or fall back to an aggregate.
- finalize CU grows ~O(K · cap). Still one tx at the target, but it is now the tick-independent
  cost to watch as K grows.
- `OrderSlabHeader` can shed the `resting_count` field and the fold-idempotency guard that only
  existed to service the counter (optional; shrinks the header → slightly larger max cap).

### Re-review triggers

- You want **> ~40 shards per market** (finalize account-list ceiling) → revisit (chunked finalize
  or a parallel-safe aggregate).
- finalize CU gets tight as `num_slab_shards · cap` grows.
- **Stage B (resting orders)** changes the fold/roll model (orders re-fold each round) — confirm
  the authoritative per-shard finalize scan still holds and that carried-over Resting orders are
  re-counted correctly.
- A future benchmark shows `Market`-write-on-submit (Design X) was actually acceptable and the
  extra finalize accounts aren't worth it (unlikely, but measure).
