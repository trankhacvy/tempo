# Design Decision Records

Short, dated records of non-obvious architecture choices, kept so they can be re-reviewed
later. Newest first.

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
