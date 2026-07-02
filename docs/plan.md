# Implementation Plan — Scaling Tempo: Sharded Book · Resting Orders · Pipelining

**Status:** proposal (no code written yet). Every change below is grounded in the current
source (paths + real identifiers referenced). Numbers/enum values reflect the code as of
`Market` VERSION 9, `OrderSlabHeader` VERSION 3, `Order` `ORDER_LEN = 88`, instruction
discriminators `0..=29` + `EmitEvent = 228`.

---

## 0. Goal & guiding principle

Three product goals:

1. **Scale past the 128-order cap** → thousands of orders per auction (sharding).
2. **Resting orders** → place once; unfilled/partial quantity carries to the next round.
3. **Pipelining / no dead time** → users can always submit; rounds overlap.

**Guiding principle (the key design decision):** today `Market`, `OrderSlab`,
`AuctionHistogram`, `ClearingResult` are **one account each per market, wiped every round**
(`reset_round_to_collect` in `program/src/instructions/round.rs`). We separate two concerns
that are currently tangled:

- **The order book = durable, sharded state** (many `OrderSlab` shards). Orders live here
  across rounds. *Not* wiped every round.
- **The clearing artifacts = ephemeral per-round snapshots** (`AuctionHistogram`,
  `ClearingResult`). Cheap, O(ticks), rebuilt each round.

Everything else follows from this split.

**Why this is safe to shard at all** (the argument to keep front-of-mind): folding is
commutative integer addition (`state/histogram.rs::fold`, `test_fold_commutativity`), and
clearing reads only cumulative sums (`clearing.rs::find_cross`). So the book is a CRDT — it
splits into N shards that fold in parallel with zero cross-shard coordination and are summed
at discovery. No new matching logic is required.

---

## 1. Staged rollout (ship independently, lowest risk first)

| Stage | What | Unlocks | Risk |
|---|---|---|---|
| **A** | Shard the `OrderSlab` (keep a single histogram) | 10k+ orders, parallel submit/settle, breaks the 128 cap | Medium |
| **B** | Resting orders (carry unfilled/partial forward) | place-once UX; standing limit book | Medium |
| **C** | Always-open submission + (optional) per-round histograms | no dead time; round overlap | High (do last) |

Each stage compiles, tests, and deploys on its own. B assumes A. C assumes A+B.

Throughout, keep the two adversarial invariants intact (see `CLAUDE.md`): **commutativity**
(fold order can't change the result) and **completeness** (no order can be silently skipped).
Sharding must not weaken either.

---

## 2. STAGE A — Shard the OrderSlab

### 2.1 New PDA scheme

Today: `OrderSlabHeader::PREFIX = b"order_slab"`, seeds `[b"order_slab", market]`
(`state/order.rs`). New: add a shard index to the seeds.

```rust
// state/order.rs — OrderSlabHeader::seeds()  (shard_id stored in the header)
fn seeds(&self) -> Vec<&[u8]> {
    vec![Self::PREFIX, self.market.as_ref(), self.shard_id_le.as_ref()] // NEW 3rd seed
}
```

- `shard_id` is a `u16` stored little-endian (`shard_id_le: [u8; 2]`), consistent with the
  align-1 rule. Seed is the 2 raw bytes.
- Shard 0's address is **not** the same as today's single slab, so this is not
  append-compatible — a fresh market must be provisioned (dev-phase: fine). Bump
  `OrderSlabHeader::VERSION` 3 → 4.

### 2.2 State changes

**`OrderSlabHeader` (`state/order.rs`)** — add two fields, bump VERSION, update `DATA_LEN` +
`assert_no_padding!` + `to_bytes_inner` + `new()`:

```rust
pub struct OrderSlabHeader {
    pub auction_id_le: [u8; 8],
    pub next_order_id_le: [u8; 8],
    pub capacity_le: [u8; 4],
    pub count_le: [u8; 4],
    pub market: Address,
    pub bump: u8,
    pub next_free_hint_le: [u8; 4],
    // --- v4 (sharding) ---
    pub shard_id_le: [u8; 2],        // which shard (also a PDA seed)
    pub resting_count_le: [u8; 4],   // orders in this shard still Resting (not yet folded)
}
// le_field!(shard_id,       set_shard_id,       shard_id_le,       u16);
// le_field!(resting_count,  set_resting_count,  resting_count_le,  u32);
```

`resting_count` is the **per-shard, authoritative-by-locality** fold counter. It is updated in
the *same borrow* as the order's `status`, so it cannot drift from that shard's contents
(this is the property PERF-1 lacked — its removed counters lived on a *different* account).

**`Market` (`state/market.rs`)** — add shard config + the global completeness aggregate. Bump
VERSION 9 → 10, update `DATA_LEN`, `assert_no_padding!`, `to_bytes_inner`, `new()`:

```rust
// --- v10 (sharding) ---
pub num_slab_shards_le: [u8; 2],   // how many shards this market has (set at init)
pub shards_pending_le:  [u8; 2],   // shards not yet fully folded THIS round (completeness aggregate)
// le_field!(num_slab_shards, set_num_slab_shards, num_slab_shards_le, u16);
// le_field!(shards_pending,  set_shards_pending,  shards_pending_le,  u16);
```

Add a `migrate_market` v9→v10 path (mirror the existing `MigrateMarket` in
`instructions/migrate_market/`) that appends 4 zero bytes and sets `num_slab_shards` to the
provisioned count. (Dev-phase alternative: just re-provision markets.)

### 2.3 Completeness with shards (the important part)

Do **not** scan all shards in `finalize_clear` (that would be O(total orders) in one tx).
Instead:

- `submit_order` inserting into shard `s` does `shard.resting_count += 1`.
- `process_chunk` folding an order in shard `s` does `shard.resting_count -= 1`.
- When a `process_chunk` call drives a shard's `resting_count` to `0`, it **scans just that
  shard** (≤128 orders, cheap) with the existing `all_active_orders_accumulated(shard_data,
  capacity)` (`state/order.rs`) to *confirm*, then decrements `Market.shards_pending` **once**.
- `finalize_clear` gate becomes O(1): `if market.shards_pending() != 0 { return AuctionNotComplete }`.

This keeps the censorship guarantee (an authoritative scan still runs — just amortized
per-shard at fold time, not all-at-once), while finalize stays cheap.

### 2.4 Instruction changes

**`submit_order` (data + processor + accounts + definition)**

- `data.rs`: add `shard_id: u16` (LEN 18 → 20). Update the exact-length gate.
- `definition.rs` `SubmitOrder`: add `shard_id: u16` to the variant, and make the `order_slab`
  account's `default_value` shard-derived (client passes the resolved shard PDA — Codama can't
  derive it from an arg-provided index cleanly, so document that the client resolves
  `[b"order_slab", market, shard_id.to_le_bytes()]`).
- `accounts.rs`: unchanged shape (still one `order_slab`), but the processor validates its PDA
  against `shard_id`.
- `processor.rs` (`instructions/submit_order/processor.rs`): validate
  `ix.data.shard_id < market.num_slab_shards()`; validate the slab PDA with the shard seed;
  after inserting, `header.set_resting_count(header.resting_count() + 1)`.
  Client shard-selection strategy (off-chain): least-full, or `hash(trader) % num_shards`.

**`process_chunk` (processor + accounts)**

- Accounts unchanged in *shape* (`cranker, market, order_slab, histogram, event_authority,
  tempo_program`) — but now `order_slab` is a **specific shard**. One `process_chunk` call =
  one shard. K shards ⇒ K calls (parallel; each write-locks a *different* shard).
- `processor.rs` (`instructions/process_chunk/processor.rs`): after the fold loop, maintain the
  counter:

```rust
// after folding `folded` orders in this shard:
let slab = OrderSlabHeader::from_bytes_mut(&mut slab_data)?;
let rc = slab.resting_count().saturating_sub(folded);
slab.set_resting_count(rc);
let shard_done = rc == 0 && all_active_orders_accumulated(&slab_data, capacity)?;
// ... then, if shard_done and this shard was not already counted done this round,
// decrement Market.shards_pending once (guard with a per-shard "folded_this_round" flag
// keyed to auction_id to make it idempotent under repeated cranks).
```

Keep the **single histogram**: all shards fold into the one `AuctionHistogram`
(`[b"histogram", market]`, unchanged). Folding stays commutative, so parallel shard folds are
safe; the histogram is not on the hot *per-order* path (it is touched by K accumulate txs, not
N submit/settle txs), so its write-lock is not the bottleneck. This keeps `finalize_clear`
unchanged (one histogram, one `find_cross` pair).

**`finalize_clear` (processor)**

- Replace the single-slab completeness scan (`instructions/finalize_clear/processor.rs`, the
  `all_active_orders_accumulated` block) with the O(1) gate:

```rust
if market.shards_pending() != 0 {
    return Err(TempoProgramError::AuctionNotComplete.into());
}
```

- Everything else (read histogram, `find_cross` bid+ask, write `ClearingResult`, phase →
  Discovered) is unchanged.
- **Also move the phase flip Discovered→Settling here** (set it in finalize, or leave in
  settle). Moving it out of `settle_fill` lets settle stop write-locking `Market` for the phase
  flip (helps §2.6).

**`settle_fill` (processor + accounts)**

- Accounts: `order_slab` is now the **shard** holding the order (client knows it from the
  `OrderSubmitted` event, which should now include `shard_id`).
- Logic is otherwise unchanged (read `ClearingResult`, `fill_against_cross`, apply to Position).
  Settles for orders in different shards now hit different slab accounts ⇒ parallel.

### 2.5 Reset / roll with shards (Stage A only; superseded by Stage B)

`start_auction` currently zeroes the one slab + histogram (`round.rs::reset_round_to_collect`).
With K shards it cannot zero all K in one tx (Solana account limit). Add a permissionless
per-shard reset:

- **New instructions `InitShard = 30` / `ResetShard = 31`** (as implemented): `init_shard` creates
  one shard PDA per tx; `reset_shard(market, order_slab_shard)` — requires the
  shard drained (`count == 0`), zeroes its slots, sets `auction_id = next`, and increments a
  Market `shards_ready` counter.
- `start_auction` precondition becomes `shards_ready == num_slab_shards` (plus histogram zeroed
  as today). It resets `shards_ready = 0` and `shards_pending = num_slab_shards` for the new
  round.

> Note: once **Stage B (resting orders)** lands, we stop zeroing shards at roll, so `ResetShard`
> becomes a rarely-used maintenance/GC path rather than a hot path.

### 2.6 De-hot-pathing `Market` on settle (parallel settle)

With sharded slabs, `submit_order` is already fully parallel (it's read-only on `Market` since
PERF-1). But `settle_fill` still write-locks `Market` for (a) the phase flip and (b)
`apply_oi_delta`. To make settle parallel across shards:

- (a) Phase flip: move to `finalize_clear` (§2.4). Then settle only *reads* the phase.
- (b) Open interest: **optional OI-sharding.** Add `oi_long`/`oi_short` deltas onto each slab
  shard header, summed lazily when a consumer needs the total (liquidation/ADL). Simpler
  interim: keep OI on `Market` and accept that settle serializes on `Market` (≈570 settles/block
  ≈ ~1,400/s — enough for a first version). Ship OI-sharding only if the benchmark shows it's the
  wall.

### 2.7 IDL / clients / tests (Stage A)

- Regenerate clients (`just generate-clients`) after the `definition.rs` edits; commit `idl/` +
  `clients/`.
- Update the SDK (`crates/sdk`) ix-builders + PDA helpers for the shard seed and the `shard_id`
  arg; add a `slab_shard_pda(market, shard_id)` helper.
- Tests: extend `state/order.rs` unit tests (shard header roundtrip, `resting_count`); add a
  host test that folds across K shards and asserts the single histogram equals the unsharded
  fold (commutativity across shards); LiteSVM `happy_path` variant with 3 shards.

### 2.8 Cost (per market, one-time, refundable rent)

- 1 shard ≈ `OrderSlabHeader::LEN + 100·ORDER_LEN` ≈ `64 + 8800 ≈ 8.9 KB` → **fits one
  `CreateAccount`** (< the 10,240 `MAX_PERMITTED_DATA_INCREASE`), no multi-realloc.
- Rent-exempt ≈ **~0.062 SOL/shard**. 10 shards ≈ **0.63 SOL**; 80 shards ≈ **~5 SOL**.
- **Honest sizing:** 10 shards × 100 = 1,000 orders/round; 80 shards = 8,000. Pick the target
  explicitly. The histogram is unchanged (single account), so tick cost is unaffected.

---

## 3. STAGE B — Resting orders (carry unfilled / partial forward)

Turns Tempo from a place-every-round call auction into a **standing limit book cleared by
batch auction**. Builds on Stage A.

### 3.1 State changes

**`Order` (`state/order.rs`)** — add two fields, bump `ORDER_LEN` 88 → 104, bump slab VERSION,
update `to_bytes`/`from_bytes`/`assert_no_padding!`/`empty`/`new_resting`:

```rust
pub struct Order {
    // ... existing fields ...
    pub cum_before: u64,
    pub reserved_margin: u64,
    // --- resting (v5) ---
    pub worst_price: u64,        // fixed worst-case exec price snapshotted at submit (see §3.3)
    pub expires_at_auction: u64, // 0 = good-till-cancelled; else auto-expire at this auction id
}
```

`worst_price` makes a resting order's margin stable across rounds (§3.3). `expires_at_auction`
bounds how long an order can rest (anti-spam; a client sets e.g. `current + 20`).

### 3.2 `settle_fill` — the core change

Today (`instructions/settle_fill/processor.rs`) the order is always marked `Consumed` and
`count` is decremented. New ending:

```rust
let fully_filled = fill == order.remaining;
let expired = order.expires_at_auction != 0 && order.expires_at_auction <= auction_id;

if fully_filled || expired {
    updated.remaining = order.remaining - fill;
    updated.status = OrderStatus::Consumed as u8;
    write_order(...);
    header.set_count(header.count().saturating_sub(1));   // leaves the book
    // release ALL leftover reserved_margin (existing path)
} else {
    // PARTIAL or ZERO fill, still live → re-arm for next round
    updated.remaining = order.remaining - fill;
    updated.status = OrderStatus::Resting as u8;          // NOT Consumed
    updated.cum_before = 0;                                // per-round; cleared for next fold
    write_order(...);
    // do NOT decrement count; the order stays in the book
    // release only the FILLED portion's margin (see §3.3); keep the rest locked
    // re-arm the shard completeness counter: this order will need folding next round
    slab.set_resting_count(slab.resting_count() + 1);
}
```

The filled portion still becomes a Position exactly as today. Only the "what happens to the
leftover" branch changes.

### 3.3 Margin for resting orders

- Reserve against a **fixed `worst_price` stored on the order** (snapshot at submit — the code
  already computes `worst_price` in `submit_order/processor.rs`; just persist it). This makes a
  resting buy *and* sell margin-stable across rounds even as the oracle-anchored window moves.
- On a partial fill, release the filled slice:
  `release = reserved_margin − initial_margin(new_remaining, worst_price, initial_bps)`; keep
  the rest locked; store the reduced `reserved_margin`.
- Reuse `margin::initial_margin` and `settle_money::release_order_reservation` (both already
  exist). No new money-path primitive.

### 3.4 `start_auction` / `reset_round_to_collect` — keep resting orders

Change `round.rs::reset_round_to_collect` so it **does not blanket-zero the slab slots**.
Instead, per shard:

```rust
for i in 0..capacity {
    let o = read_order(&slab_data, capacity, i)?;
    match OrderStatus::from_u8(o.status)? {
        OrderStatus::Resting => { /* keep as-is: reduced remaining, status Resting, cum_before 0 */ }
        _ => { write_order(&mut slab_data, capacity, i, &Order::empty())?; } // Consumed/Empty → free
    }
}
// recompute this shard's resting_count = number of surviving Resting orders
// set shard.auction_id = next_id; keep next_free_hint sane (or rebuild)
```

Then `Market.shards_pending` for the new round = number of shards with `resting_count > 0`.
The histogram is still fully zeroed (it's the per-round snapshot). Next round's `process_chunk`
re-folds the surviving Resting orders normally (Resting → Accumulated).

### 3.5 Cancellation of a resting order

`cancel_order` currently requires phase `Collect` and status `Resting`
(`instructions/cancel_order/processor.rs`). A resting order between rounds is `Resting` in
`Collect`, so cancellation already works. Verify the reservation release uses the (possibly
reduced) `reserved_margin` on the order — it does (`order.reserved_margin`). No change needed,
but add a test for cancel-after-partial.

### 3.6 Honest edge cases

- **Reduce-only + resting:** a reduce-only order that outlives the position it was reducing
  should be re-evaluated each round; keep the existing `reduce_only` reservation logic per round
  (it already reserves only the opening portion).
- **Book growth:** resting orders accumulate, so the per-shard 128 cap now bounds *standing*
  orders, not per-round flow. This makes Stage A's shard count more important (size the shards
  for the expected resting depth). Add a per-trader standing-order cap (extend
  `MAX_ORDERS_PER_TRADER`, currently 8, or make it a Market config).
- **Re-fold cost each round:** every resting order is re-folded per round (cheap: benchmark
  shows ~183 CU/order, ~7,594 orders/tx), but it is ongoing work — the keeper must crank it.

### 3.7 Tests (Stage B)

- `state/order.rs`: roundtrip with the two new fields; `ORDER_LEN` assertions.
- Host test: partial fill → order stays `Resting` with reduced `remaining` and margin; next
  round re-folds and fills the rest; total filled across rounds == original qty (conservation).
- LiteSVM: two-round scenario where a partial rests then completes without re-submitting.

---

## 4. STAGE C — Always-open submission & pipelining (no dead time)

Two options, from light to heavy. **Recommend C1 first**; treat C2 as a stretch.

### 4.1 C1 (recommended) — always-open submission

Problem today: `submit_order` requires phase `Collect`
(`submit_order/processor.rs::require_phase(AuctionPhase::Collect)`), so between Collect windows
users cannot place orders (dead time).

Fix: **accept `submit_order` in any phase.** If the market is past `Collect`, the new order is
tagged for the **next** round and is *not* eligible for the current round's fold.

- Add `arm_auction_id: u64` to `Order` (the round this order first becomes foldable). In
  `Collect` it's the current id; otherwise `current + 1`.
- `process_chunk` folds an order only if `order.arm_auction_id == market.current_auction_id()`.
  Orders armed for the next round are skipped this round and counted only against next round's
  `shards_pending`.
- `Market.shards_pending` bookkeeping must separate "this round's resting" from "armed for
  next" — simplest is a second per-shard counter `next_round_count`, promoted to `resting_count`
  at roll.

This removes dead time with **no new histogram accounts** — the book is continuously open;
each round simply folds the orders armed for it. Much less risk than C2.

### 4.2 C2 (stretch) — per-round histograms for true processing overlap

For maximum throughput, let round N+1's *accumulate* overlap round N's *settle*. Requires the
per-round clearing artifacts to not collide:

- **Double-buffer** the histogram + result by round parity: seeds
  `[b"histogram", market, &[(auction_id % 2) as u8]]` and
  `[b"clearing", market, &[(auction_id % 2) as u8]]`. Pre-create both buffers at init (2
  accounts, no dynamic creation). Round N uses buffer `N%2`; N+1 uses `(N+1)%2`; they never
  alias until N+2 (by which time N is fully settled).
- `Order.cum_before` becomes per-buffer (`cum_before: [u64; 2]` indexed by parity) so a resting
  order can carry independent fold prefixes for two in-flight rounds.
- `finalize_clear` / `settle_fill` / `process_chunk` take the parity-selected histogram/result.
- **Ordering rule to keep safe:** round N+1 `finalize_clear` must not run until round N `settle`
  has read its results for shared orders — enforce by requiring N fully settled
  (`shards drained`) before N+1 finalize, i.e. overlap *collection + accumulation* but serialize
  *finalize→settle* per order. Document precisely; add invariants + fuzz.

C2 is genuinely more complex (two live rounds touching the same durable book). Only build it if
the benchmark proves C1 + Stage A parallel settle is insufficient.

### 4.3 Pipeline cadence bookkeeping

Either option keeps the collection window logic (`COLLECT_WINDOW_SLOTS`, `phase_deadline_slot`)
but the roll (`start_auction`) opens the next `Collect` immediately after `Discovered`/`Settling`
begins rather than after full settlement. With C1 this is mostly a scheduling change in the
keeper (`crates/keeper`) plus the always-open submit; with C2 it's the double-buffer.

---

## 5. Cross-cutting: files to touch (checklist)

Program (`program/src/`):
- `state/order.rs` — `OrderSlabHeader` (shard fields, VERSION 4→…); `Order` (worst_price,
  expires_at_auction, arm_auction_id, ORDER_LEN); seeds; helpers (`resting_count` maintenance);
  tests.
- `state/market.rs` — `num_slab_shards`, `shards_pending`, (`shards_ready`), VERSION 10; DATA_LEN;
  assert_no_padding; migrate path.
- `instructions/submit_order/{data,accounts,processor}.rs` — `shard_id`, shard PDA validate,
  counter++, persist `worst_price`/`expires_at_auction`, always-open (C1).
- `instructions/process_chunk/processor.rs` — per-shard fold + counter/shards_pending.
- `instructions/finalize_clear/processor.rs` — O(1) `shards_pending` gate; phase flip.
- `instructions/settle_fill/processor.rs` — resting branch (§3.2).
- `instructions/start_auction/processor.rs` + `round.rs` — keep-resting reset; shards_ready.
- `instructions/reset_shard/*` — **new** (disc 30); `entrypoint.rs`, `traits/instruction.rs`,
  `instructions/definition.rs`, `impl_instructions.rs`, `instructions/mod.rs`.
- `instructions/migrate_market/*` — v9→v10 append.
- `events/*` — add `shard_id` to `OrderSubmitted`/`FillSettled` (indexer/keeper need it).

Off-chain:
- `idl/` + `clients/` — regenerate (`just generate-clients`).
- `crates/sdk` — shard PDA helpers, new ix args, `reset_shard` builder.
- `crates/keeper` — crank all shards (submit/accumulate/settle fan-out), cadence for pipelining.
- `crates/sim`, `tests/integration-tests` — multi-shard scenarios.

New instruction discriminators (as implemented): `InitShard = 30`, `ResetShard = 31` (next free after `CloseMakerQuote
= 29`; `EmitEvent` stays `228`).

---

## 6. Testing & verification strategy

- **Unit (host, `cargo test --features idl`):** shard header/Order roundtrips; per-shard
  completeness counter; cross-shard fold == unsharded fold (commutativity across shards);
  resting partial → carry → complete conserves quantity; margin release on partial.
- **Fuzz:** extend the existing `clearing.rs` fuzzes to fold a random book split across random
  shards and assert identical `find_cross` + OI conservation across rounds with resting orders.
- **Kani:** the arithmetic (`find_cross`, `wide_mul`, `unrealized_pnl`) is unchanged, so existing
  proofs hold; add a small proof that `resting_count`/`shards_pending` never underflow.
- **LiteSVM (`tests/integration-tests`):** K-shard happy path; two-round resting; always-open
  submit during Accumulate/Settle lands in the next round.
- **Benchmark (`crates/bench`, `docs/bench/cu_report.md`):** re-run to show parallel-shard
  submit/settle throughput and confirm finalize stays O(ticks) with a single histogram. Publish
  before/after.

---

## 7. Honest risks & open questions

1. **Completeness via per-shard counters.** We trade the all-at-once authoritative scan for
   per-shard scans amortized at fold time + an O(1) `shards_pending` gate. This is safe *iff*
   `resting_count` is always maintained in the same borrow as order status (it is, by design) —
   but it must be reviewed as carefully as the current gate, because it *is* the censorship
   surface. Keep an optional full-scan debug assert behind a feature flag.
2. **Settle still serializes on `Market` OI** until OI-sharding lands (§2.6). Measure before
   committing to the extra complexity.
3. **Resting sells & a moving window.** Solved by snapshotting `worst_price` on the order (§3.3);
   verify a resting order never becomes under-margined after a window recenter (test it).
4. **Book growth vs shard size.** Resting orders accumulate; size shard count for standing depth,
   add a per-trader standing cap, and rely on `expires_at_auction` to bound spam.
5. **C2 pipelining is genuinely hard** (two live rounds over one durable book). Gate it behind
   real benchmark need; C1 removes dead time at a fraction of the risk.
6. **Fixed shard count is a hard cap.** Chosen at init; raising it later means adding shards
   (cheap, but a new provisioning step). Pick a generous count up front.

---

## 8. Recommended sequence

1. **Stage A** (sharding) — the headline scaling win; ship + benchmark first.
2. **Stage B** (resting orders) — the UX win; ship second.
3. **Stage C1** (always-open submit) — removes dead time; ship third.
4. **Stage C2 / OI-sharding** — only if the benchmark says the remaining serialization is the
   wall.

Each stage is independently valuable and independently testable, and none requires a rewrite of
the clearing math — the crown jewel (`clearing.rs`) is untouched throughout.

---

## 9. Detailed TODO list (work breakdown)

Execution order is top-to-bottom. Each `[ ]` is a discrete, reviewable unit of work. Do **not**
start coding until this list is agreed.

### Stage 0 — Prep & scaffolding

- [x] 0.1 Create a feature branch `feat/sharded-book` off `main`.
- [x] 0.2 Snapshot the current benchmark: archived `docs/bench/cu_report.md` as
      `docs/bench/cu_report_pre_shard.md` (the CU profile is the sharding-relevant baseline);
      verified `cargo run -p tempo-bench` still builds and reproduces the O(ticks) shape.
- [x] 0.3 **Decided:** `num_slab_shards = 16` (dev default; it is a per-market init param, so a
      production market can be created with 80+ shards for ~7,200 orders). **per-shard cap = 90**,
      sized for the FINAL `Order` size (`ORDER_LEN` grows 88 → 104 in B → ~112 in C1): at 112 B,
      `2 + header(69) + cap·112 ≤ 10,240` ⟹ `cap ≤ 90`, keeping every shard within a **single
      `CreateAccount`** (no multi-realloc) through all stages. 16 × 90 = **1,440 orders/round**
      initially; raise the shard count to scale.
- [x] 0.4 **Decided: re-provision, do NOT migrate.** The slab seed gains `shard_id`, so the old
      single slab and "shard 0" live at different addresses — in-place slab migration is impossible
      regardless. **Skip the ⟨migrate⟩ tasks (A10)**; the `Market` v9→v10 append-migrate is optional
      and deferred. Devnet markets are re-provisioned fresh.
- [x] 0.5 **Benchmark source note:** the CU numbers in `docs/bench/cu_report.md` are produced by the
      LiteSVM harness `tests/integration-tests/tests/benchmark.rs`, NOT `crates/bench` (host timings).
      Task A13.4 must regenerate via that integration test.

### Stage A — Shard the OrderSlab

**A1. State: `OrderSlabHeader` (`program/src/state/order.rs`)**
- [x] A1.1 Add fields `shard_id_le: [u8;2]` and `resting_count_le: [u8;4]`.
- [x] A1.2 Add `le_field!` accessors: `shard_id`/`set_shard_id` (u16), `resting_count`/`set_resting_count` (u32).
- [x] A1.3 Bump `OrderSlabHeader::VERSION` 3 → 4; update the version doc comment.
- [x] A1.4 Update `DATA_LEN`, `assert_no_padding!`, `to_bytes_inner`, and `new(...)` (accept `shard_id`).
- [x] A1.5 Change `seeds()` / `seeds_with_bump()` to append `shard_id_le` as the 3rd seed.
- [x] A1.6 Update slab unit tests (header roundtrip incl. new fields; seed length == 3; `resting_count` set/get).

**A2. State: `Market` (`program/src/state/market.rs`)**
- [x] A2.1 Add fields `num_slab_shards_le: [u8;2]`, `shards_pending_le: [u8;2]` (and `shards_ready_le: [u8;2]` for §2.5 reset).
- [x] A2.2 Add `le_field!` accessors for each.
- [x] A2.3 Bump `Market::VERSION` 9 → 10; update the version history comment.
- [x] A2.4 Update `DATA_LEN`, `assert_no_padding!`, `to_bytes_inner`, and `new(...)` (accept `num_slab_shards`; init `shards_pending`/`shards_ready`).
- [x] A2.5 Update `Market` unit tests (roundtrip, defaults).

**A3. Instruction: `submit_order`**
- [x] A3.1 `data.rs`: add `shard_id: u16`; LEN 18 → 20; update the exact-length gate + tests.
- [x] A3.2 `processor.rs`: validate `shard_id < market.num_slab_shards()`; validate the shard PDA with the shard seed; on insert do `resting_count += 1`.
- [x] A3.3 `definition.rs` `SubmitOrder`: add the `shard_id: u16` field; document client-resolved shard PDA for `order_slab`.
- [x] A3.4 `accounts.rs`: no shape change (still one `order_slab`); confirm doc comment mentions the shard.

**A4. Instruction: `process_chunk` (`program/src/instructions/process_chunk/processor.rs`)**
- [x] A4.1 After the fold loop, decrement the shard's `resting_count` by `folded`.
- [x] A4.2 When `resting_count == 0`, run `all_active_orders_accumulated(shard_data, capacity)` to confirm, then decrement `Market.shards_pending` **once** (idempotent per round via an auction-id-keyed "shard folded" guard).
- [x] A4.3 Keep the single histogram (all shards fold into `[b"histogram", market]`); no signature change.
- [x] A4.4 Add a test: fold across K shards → single histogram equals the unsharded fold (cross-shard commutativity).

**A5. Instruction: `finalize_clear` (`program/src/instructions/finalize_clear/processor.rs`)**
- [x] A5.1 Replace the single-slab `all_active_orders_accumulated` gate with `if market.shards_pending() != 0 { AuctionNotComplete }`.
- [x] A5.2 Move the phase flip Discovered→Settling into finalize (set `phase = Settling` here) so settle stops write-locking Market for the flip.
- [x] A5.3 Confirm the maker-quote completeness gate (`folded_maker_quote_count == active_maker_quote_count`) is untouched.

**A6. Instruction: `settle_fill` (`program/src/instructions/settle_fill/processor.rs`)**
- [x] A6.1 `order_slab` account is now the shard holding the order; validate its shard PDA.
- [x] A6.2 Remove the Discovered→Settling flip (now done in finalize); read phase only.
- [ ] A6.3 (Optional, §2.6) OI-sharding: DEFERRED — OI stays on Market (settle still write-locks Market for the OI update). The benchmark shows submit is already parallel across shards; pursue OI-sharding only if settle serialization becomes the wall.

**A7. New instructions: `InitShard = 30`, `ResetShard = 31`** (implemented — `init_shard` creates each shard PDA one-per-tx; `reset_shard` drains/rolls one shard)
- [x] A7.1 Create `program/src/instructions/reset_shard/{mod,accounts,data,processor}.rs`.
- [x] A7.2 Logic: require the shard drained (`count == 0`), zero its slots, set `auction_id = market.current_auction_id() + 1`, `resting_count = 0`, `shards_ready += 1` on Market.
- [x] A7.3 Register: `entrypoint.rs`, `traits/instruction.rs` (disc 30 + `TryFrom`), `instructions/definition.rs`, `impl_instructions.rs`, `instructions/mod.rs`.

**A8. Instruction: `start_auction` / `round.rs`**
- [x] A8.1 Precondition becomes `shards_ready == num_slab_shards` (instead of the single-slab `count == 0`).
- [x] A8.2 On roll: zero the histogram (single account, unchanged), reset `shards_ready = 0`, `shards_pending = num_slab_shards`, bump auction id, reopen Collect.
- [x] A8.3 Update `reset_round_to_collect` to no longer touch the slab (the shard reset is now `ResetShard`).

**A9. Instruction: `initialize_market`**
- [x] A9.1 Accept `num_slab_shards` in `data.rs` + `definition.rs` (recompute `InitializeMarket` byte length; update `CLAUDE.md`'s "129 bytes" note).
- [x] A9.2 Create **all** shard PDAs at init (loop `create_pda_account` per shard, each ≤10,240 B), or provide a separate `InitShard` ix if the per-tx account/CU budget is tight — decide and note.
- [x] A9.3 Set `Market.num_slab_shards`, `shards_pending = num_slab_shards`.

**A10. ⟨migrate⟩ (SKIPPED — re-provision per §0.4)**
- [ ] A10.1 SKIPPED: no `migrate_market` v9→v10 path — markets are re-provisioned. The two v4→v5 / v1→v3 migration integration tests are `#[ignore]`d (they target a pre-shard layout the VERSION-10 bump superseded).
- [x] A10.2 Old single slabs can't be migrated to shards in place → re-provision (documented; slab seed gained `shard_id`).

**A11. Events**
- [x] A11.1 Add `shard_id` to `OrderSubmittedEvent` and `FillSettledEvent` (`program/src/events/`), so clients/keeper know which shard to settle.

**A12. Off-chain (Stage A)**
- [x] A12.1 Regenerate IDL + clients: `just generate-clients`; commit `idl/` + `clients/`.
- [x] A12.2 `crates/sdk`: add `slab_shard_pda(market, shard_id)`; thread `shard_id` through the submit/settle/process-chunk builders; add a `reset_shard` builder.
- [x] A12.3 `crates/keeper`: fan out `process_chunk`/`settle_fill`/`reset_shard` across all shards; shard-selection helper for submit (least-full / hash).
- [x] A12.4 `crates/sim` + `tests/integration-tests`: multi-shard `happy_path`.

**A13. Tests & benchmark (Stage A)**
- [x] A13.1 Host unit tests for A1–A6 additions.
- [x] A13.2 Cross-shard fold-commutativity + `shards_pending` completeness tests.
- [x] A13.3 Kani: underflow-freedom proof for `resting_count`/`shards_pending`.
- [x] A13.4 Re-run the LiteSVM CU benchmark (`tests/integration-tests/tests/benchmark.rs`, NOT `crates/bench` — see 0.5); write `docs/bench/cu_report.md` showing parallel submit/settle + unchanged finalize; compare to `cu_report_pre_shard.md`.
- [x] A13.5 `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --features idl`, `cargo-build-sbf`.

### Stage B — Resting orders

**B1. State: `Order` (`program/src/state/order.rs`)**
- [x] B1.1 Add fields `worst_price: u64` and `expires_at_auction: u64`.
- [x] B1.2 Bump `ORDER_LEN` 88 → 104; update `to_bytes`/`from_bytes`/`assert_no_padding!`/`empty()`/`new_resting()`.
- [x] B1.3 Bump `OrderSlabHeader::VERSION` (4 → 5) — slot region size changed.
- [x] B1.4 Update Order roundtrip tests for the new fields + length.

**B2. Instruction: `submit_order`**
- [x] B2.1 Persist the (now unconditionally computed) `worst_price` onto the order.
- [x] B2.2 Accept `expires_at_auction` (0 = GTC, absolute auction id) in `data.rs` + `definition.rs` (LEN 20 → 28).
- [x] B2.3 Per-trader standing cap: `MAX_ORDERS_PER_TRADER` is enforced per shard; reference clients route each trader to a deterministic shard (`pda::shard_for_trader` = `hash(trader) % num_slab_shards`) so it acts as a global cap (DDR-3 / Finding 4).

**B3. Instruction: `settle_fill` (the core change, §3.2)**
- [x] B3.1 Compute `fully_filled = (fill == order.remaining)` and `expired = expires_at_auction != 0 && expires_at_auction <= auction_id`.
- [x] B3.2 Full/expired branch → status `Consumed`, `count -= 1`, release all leftover margin.
- [x] B3.3 Partial/zero-and-live branch → `remaining -= fill`, status `Resting`, `cum_before = 0`, `resting_count += 1`, **do not** decrement `count`.
- [x] B3.4 Margin on partial: hold the leftover at the EXACT `initial_margin(remaining, worst_price, initial_bps)` (clamped ≤ reserved) and release the rest (DDR-3 / Finding 6, supersedes the DDR-2 `min(reserved, im(fill))` split which left the leftover ~1 unit short). Any shortfall covering the filled slice's position lock is absorbed by the no-revert re-lock (B7).

**B4. Roll: `reset_shard` (§3.4 — under Design Z the roll lives in `reset_shard`, not `round.rs`)**
- [x] B4.1 Replace the blanket slot-zero with a per-slot pass: keep `Resting`, free `Consumed`/`Empty`.
- [x] B4.2 Roll gate changed from `count == 0` to `all_accumulated_orders_settled` (no `Accumulated` remains); recompute `count`/`resting_count` = surviving Resting orders. (Design Z: NO `Market.shards_pending` aggregate — DDR-2.)
- [x] B4.3 Reset `next_free_hint = 0`; keep `next_order_id` monotonic (never reset — avoids id reuse vs survivors).
- [x] B4.4 Histogram still fully zeroed by `reset_round_to_collect` (the per-round snapshot; unchanged).

**B5. Instruction: `cancel_order`**
- [x] B5.1 Cancel-after-partial releases the (reduced) `reserved_margin` — logic unchanged (a re-armed Resting order is `Resting` in `Collect`, so it cancels normally).

**B6. Tests (Stage B — `tests/integration-tests/tests/resting_orders.rs`)**
- [x] B6.1 Partial fill → order stays Resting with correct `remaining` (`partial_fill_rests_then_completes_conserving`).
- [x] B6.2 Two-round: partial rests then completes; Σ fills across rounds == original qty (conservation).
- [x] B6.3 Expiry: an order past `expires_at_auction` is Consumed, not re-armed (`expired_resting_order_is_consumed_not_rearmed`).
- [x] B6.4 DDR-1 trigger: a carried Resting order blocks finalize until re-folded (`carried_resting_order_blocks_finalize_next_round`); roll gate rejects unsettled Accumulated (`roll_gate_rejects_unsettled_then_carries_resting`).
- [x] B6.5 Full gate green: `fmt` / workspace `clippy -D warnings` / host `test` / `build-sbf` / `integration-test` (42 suites). Note: the `worst_price` snapshot keeps a resting sell margin-stable across a window recenter by construction (§3.3) — a dedicated money-path recenter test (B6.4-margin) is deferred to the money-path suite.

**B7. DDR-3 code-review fixes (resting orders vs. the moving tick window)** — the high-effort review of the Stage-B commit found two blockers + four lesser defects; all fixed and verified (see `docs/design-decisions.md` DDR-3).
- [x] B7.1 (Finding 1, blocker) Marketable-fill / passive-park: `classify_resting_fold` (pure, in `state/market.rs`, unit-tested) classifies a resting order whose fixed price left the recentered window. `process_chunk` folds in-window/marketable orders (marketable at the boundary tick) and SKIPS passive ones; `all_active_orders_accumulated` exempts only passive orders (window params threaded from `finalize_clear`). No more permissionless wedge; the censorship guarantee holds on a recomputable verdict (no new counter). Keeper mirror updated (`snapshot::all_resting_folded`).
- [x] B7.2 (Finding 2, blocker) `settle_fill` never reverts: the re-lock uses `UserCollateral::lock_up_to` (locks what's free, never errors); the position's collateral is set to what was actually locked, so a resting order the window gapped through settles (leaving a liquidatable position on a shortfall) instead of wedging the roll.
- [x] B7.3 (Finding 3) `reduce_only` persisted on `Order` (reused a padding byte → `ORDER_LEN` still 104, cap still 90); `settle_fill` clamps a carried reduce-only order's fill to the reduce headroom, so it can never open new exposure.
- [x] B7.4 (Finding 4) Deterministic per-trader shard routing — see B2.3.
- [x] B7.5 (Finding 5) Sim `TEMPO_SIM_CAP` clamp ceiling 115 → 90 (matches the on-chain cap).
- [x] B7.6 (Finding 6) Leftover reservation held exact — see B3.4.
- [x] B7.7 Tests: `classify_resting_fold` + `all_active_orders_accumulated` (passive-exempt) + `lock_up_to` unit tests; integration `passive_resting_order_parks_then_folds_when_window_returns`, `marketable_resting_order_folds_after_recenter`, `reduce_only_order_cannot_open_exposure`. Full gate green (program 200 host tests; 100 integration across 42 suites).

### Stage C1 — Always-open submission (no dead time)

- [ ] C1.1 `Order`: add `arm_auction_id: u64` (bump `ORDER_LEN` + slab VERSION again).
- [ ] C1.2 `submit_order`: remove `require_phase(Collect)`; set `arm_auction_id = current` if in Collect else `current + 1`.
- [ ] C1.3 `process_chunk`: fold an order only if `arm_auction_id == current_auction_id`.
- [ ] C1.4 Completeness: add a per-shard `next_round_count`; promote it to `resting_count` at roll; keep `shards_pending` correct for the current round only.
- [ ] C1.5 `start_auction`: on roll, promote `next_round_count` and re-arm carried orders.
- [ ] C1.6 `crates/keeper`: schedule the next Collect to open immediately (cadence change).
- [ ] C1.7 Tests: submit during Accumulate/Discover/Settle lands in the next round; current round unaffected.

### Stage C2 — Per-round histograms (stretch; only if benchmark demands)

- [ ] C2.1 Double-buffer histogram + result by parity: seeds `[b"histogram", market, &[auction_id % 2]]`, `[b"clearing", market, &[auction_id % 2]]`; pre-create both at init.
- [ ] C2.2 `Order.cum_before` → `[u64; 2]` indexed by parity.
- [ ] C2.3 `process_chunk`/`finalize_clear`/`settle_fill` take the parity-selected accounts.
- [ ] C2.4 Enforce the ordering invariant: round N fully settled before round N+1 finalize (per shared order); document + fuzz.
- [ ] C2.5 `crates/keeper`: drive two overlapping rounds; add safety backpressure.
- [ ] C2.6 Extensive fuzz + LiteSVM for overlapping rounds; benchmark the throughput gain vs. C1.

### Cross-cutting close-out

- [ ] X.1 Update `CLAUDE.md` (module map, instruction list, discriminators, the "129 bytes" note, VERSION numbers).
- [ ] X.2 Update `docs/system-design.md` / `docs/tempo-clearing-protocol.md` for the sharded + resting model; refresh `docs/design-viz.html` if needed.
- [ ] X.3 Update `docs/verification.md` (invariant→test matrix) for the new completeness + resting invariants.
- [ ] X.4 Update `docs/known-issues.md` / `docs/missing-features.md` (close the throughput + resting items).
- [ ] X.5 Final full-suite gate: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --features idl`, `cargo-build-sbf`, `just integration-test`, `cargo kani`.
- [ ] X.6 Devnet: re-provision (or migrate) a market with shards; run the `crates/sim` fleet end-to-end; publish the new benchmark.
