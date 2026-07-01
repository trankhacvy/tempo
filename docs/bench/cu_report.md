# Tempo Clearing Benchmark (CU profile)

Measured under LiteSVM 0.13 (in-process SVM). CU accounting is a faithful proxy for the on-chain meter; treat the *relative profile*, the *scaling*, and the *derived ceilings* as the signal, not the absolute digits. This is the **Stage A sharded** design: the OrderSlab is split into `num_slab_shards` shards, each holding up to `SLAB_CAP` orders, that submit and fold in parallel (submit is read-only on Market since PERF-1); the single AuctionHistogram and Market remain shared. Per-instruction CU is unchanged from the pre-shard baseline (`cu_report_pre_shard.md`) — sharding adds no per-tx overhead; it multiplies intake throughput by the shard count.

Solana limits used: **1400000 CU/tx**, **12000000 CU/account/block** (write-lock).

## submit_order — CU vs slab occupancy

Writes Market + OrderSlab. The cost grows with occupancy because `find_free_slot` and the per-trader-cap count scan the slab (O(n)).

| orders already resting | CU |
|---|---|
| 1 | 7460 |
| 30 | 10605 |
| 60 | 7755 |
| 89 | 13900 |

## process_chunk — fold cost

Writes the shard + Market + AuctionHistogram. Folding is O(orders) but the per-order cost is small next to the fixed base (event CPI + account I/O), so we report the clean endpoints: one order vs a full shard (SLAB_CAP orders) in a single chunk. Shards fold in parallel into the one histogram (commutative addition).

| orders folded | CU |
|---|---|
| 1 | 11331 |
| 90 | 27263 |

Incremental: ~**11152 CU base** + ~**179 CU/order**. A single chunk tx could fold ~**7758** orders under the 1400000 CU/tx limit — far more than a slab can hold — so **folding compute is not the constraint**; the slab's single-account size cap is.

## finalize_clear — CU vs num_ticks

One transaction; a single O(ticks) pass over the 4 histogram regions (both crosses).

| num_ticks | CU |
|---|---|
| 64 | 29457 |
| 128 | 55237 |
| 256 | 93297 |

At the max supported 256 ticks finalize uses ~93297 CU — 6.7% of the 1400000 CU/tx limit, so the discovery pass fits comfortably in one tx across the whole tick range.

## settle_fill — CU

One order per tx (writes Market + OrderSlab, + Position when filled). Includes the marginal-tick cumulative scan. Measured: **22339 CU**.

## The former hard ceiling — now sharded away

Solana caps a CPI-created/grown account at **10_240 bytes** per instruction (`MAX_PERMITTED_DATA_INCREASE`). At `ORDER_LEN = 88` one OrderSlab account tops out near **115 orders**, which is why the pre-shard design was capped at 128 and could not reach "thousands of orders". **Stage A removes this by sharding**: the slab is split into `num_slab_shards` independent shard accounts (`init_shard`), each sized at `SLAB_CAP` (90 orders, kept within one CPI `CreateAccount` through every stage). N shards ⇒ N·SLAB_CAP orders/round with no per-tx overhead, and — because submit is read-only on Market and each shard is its own account — submissions and settlements to different shards run in parallel. The single histogram is still O(ticks) and untouched. Completeness stays a hard gate: `finalize_clear` refuses until every shard reports folded (`shards_pending == 0`), an O(1) check backed by a per-shard confirming scan.

## Throughput of one full shard (SLAB_CAP orders)

Settle write-locks Market (OI), so a shard's settle CU competes for Market's 12M-CU/block budget; submit is read-only on Market and hits only its own shard, so shards submit in parallel. Modelling one full shard end-to-end (aggregate = this × num_slab_shards for the parallel parts):

| phase | txs | CU each | CU total |
|---|---|---|---|
| submit (own shard, parallel) | 90 | ~13900 (occ 89) | ~1251000 |
| accumulate | 1 | ~27263 | ~27263 |
| finalize | 1 | ~93297 | ~93297 |
| settle | 90 | ~22339 | ~2010510 |
| **total Market write-lock** | | | **~3382070 CU** |

A full shard puts ~3382070 CU on the Market write-lock — about 28.2% of one block's 12M budget — so one shard's settle-side load clears in ~1 block(s). Submission is now parallel across shards (Market read-only), so intake scales with `num_slab_shards`; the remaining shared serialization is the Market OI write on settle (a candidate for OI-sharding if the benchmark shows it is the wall).


## Reading the result

- The clearing math (`finalize_clear`) is **not** the bottleneck — it is O(ticks), one tx, and fits the per-tx limit comfortably across the whole supported tick range.
- The ceiling is **write-lock contention on the shared accounts** (Market / OrderSlab / Histogram). Submission, accumulation and settlement each serialize per market within a block.
- `submit_order` and `settle_fill` cost grows with slab occupancy (the O(n) scans) — the slab-scan/free-list optimization and de-hot-pathing Market would lift the submit/settle ceiling; sharding the slab + histogram would lift the accumulation ceiling. Whether that work is warranted is exactly what these numbers are meant to decide.

