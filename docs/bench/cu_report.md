# Tempo Clearing Benchmark (CU profile)

Measured under LiteSVM 0.13 (in-process SVM). CU accounting is a faithful proxy for the on-chain meter; treat the *relative profile*, the *scaling*, and the *derived ceilings* as the signal, not the absolute digits. This is the **Stage A sharded** design: the OrderSlab is split into `num_slab_shards` shards, each holding up to `SLAB_CAP` orders, that submit and fold in parallel (submit is read-only on Market since PERF-1); the single AuctionHistogram and Market remain shared. Per-instruction CU is unchanged from the pre-shard baseline (`cu_report_pre_shard.md`) — sharding adds no per-tx overhead; it multiplies intake throughput by the shard count.

Solana limits used: **1400000 CU/tx**, **12000000 CU/account/block** (write-lock).

## submit_order — CU vs slab occupancy

Writes Market + OrderSlab. The cost grows with occupancy because `find_free_slot` and the per-trader-cap count scan the slab (O(n)).

| orders already resting | CU |
|---|---|
| 1 | 7494 |
| 30 | 9139 |
| 60 | 9289 |
| 89 | 13934 |

## process_chunk — fold cost

Writes the shard + Market + AuctionHistogram. Folding is O(orders) but the per-order cost is small next to the fixed base (event CPI + account I/O), so we report the clean endpoints: one order vs a full shard (SLAB_CAP orders) in a single chunk. Shards fold in parallel into the one histogram (commutative addition).

| orders folded | CU |
|---|---|
| 1 | 11920 |
| 90 | 27775 |

Incremental: ~**11742 CU base** + ~**178 CU/order**. A single chunk tx could fold ~**7799** orders under the 1400000 CU/tx limit — far more than a slab can hold — so **folding compute is not the constraint**; the slab's single-account size cap is.

## finalize_clear — CU vs num_ticks

One transaction; a single O(ticks) pass over the 4 histogram regions (both crosses).

| num_ticks | CU |
|---|---|
| 64 | 37407 |
| 128 | 58687 |
| 256 | 95247 |

At the max supported 256 ticks finalize uses ~95247 CU — 6.8% of the 1400000 CU/tx limit, so the discovery pass fits comfortably in one tx across the whole tick range.

## finalize_clear — CU vs shard count (Design-Z completeness scan)

At 256 ticks, passing K shards (each folded). On top of the O(ticks) discovery pass, finalize scans every shard it is passed (`all_active_orders_accumulated`, an O(capacity) loop per shard) to prove completeness in one tx (DDR-1). This is the cost that grows with K·SLAB_CAP.

| shards passed | CU |
|---|---|
| 1 | 106487 |
| 8 | 130722 |
| 16 | 160542 |

At the dev target of 16 shards × 90 cap, finalize uses ~160542 CU — 11.5% of the 1400000 CU/tx limit. The per-shard scan adds ~3603 CU/shard on top of the ~106487 CU tick pass, so finalize stays a single tx well under the limit at the dev target; K·SLAB_CAP is the cost to watch as shard count grows (DDR-1 re-review trigger: chunked finalize past ~40 shards).

## settle_fill — CU

One order per tx (writes Market + OrderSlab, + Position when filled). Includes the marginal-tick cumulative scan. Measured: **20869 CU**.

## The former hard ceiling — now sharded away

Solana caps a CPI-created/grown account at **10_240 bytes** per instruction (`MAX_PERMITTED_DATA_INCREASE`). At `ORDER_LEN = 112` (Stage C1) one OrderSlab account tops out near **90 orders**, which is why the pre-shard design was capped at 128 and could not reach "thousands of orders". **Stage A removes this by sharding**: the slab is split into `num_slab_shards` independent shard accounts (`init_shard`), each sized at `SLAB_CAP` (90 orders, kept within one CPI `CreateAccount` through every stage). N shards ⇒ N·SLAB_CAP orders/round with no per-tx overhead, and — because submit is read-only on Market and each shard is its own account — submissions and settlements to different shards run in parallel. The single histogram is still O(ticks) and untouched. Completeness stays a hard gate: `finalize_clear` refuses until every shard it is passed scans as fully folded (Design Z), an O(K) check backed by a per-shard confirming scan.

## Throughput of one full shard (SLAB_CAP orders)

Settle write-locks Market (OI), so a shard's settle CU competes for Market's 12M-CU/block budget; submit is read-only on Market and hits only its own shard, so shards submit in parallel. Modelling one full shard end-to-end (aggregate = this × num_slab_shards for the parallel parts):

| phase | txs | CU each | CU total |
|---|---|---|---|
| submit (own shard, parallel) | 90 | ~13934 (occ 89) | ~1254060 |
| accumulate | 1 | ~27775 | ~27775 |
| finalize | 1 | ~95247 | ~95247 |
| settle | 90 | ~20869 | ~1878210 |
| **total Market write-lock** | | | **~3255292 CU** |

A full shard puts ~3255292 CU on the Market write-lock — about 27.1% of one block's 12M budget — so one shard's settle-side load clears in ~1 block(s). Submission is now parallel across shards (Market read-only), so intake scales with `num_slab_shards`; the remaining shared serialization is the Market OI write on settle (a candidate for OI-sharding if the benchmark shows it is the wall).


## Reading the result

- The clearing math (`finalize_clear`) is **not** the bottleneck — it is O(ticks), one tx, and fits the per-tx limit comfortably across the whole supported tick range.
- The ceiling is **write-lock contention on the shared accounts** (Market / OrderSlab / Histogram). Submission, accumulation and settlement each serialize per market within a block.
- `submit_order` and `settle_fill` cost grows with slab occupancy (the O(n) scans) — the slab-scan/free-list optimization and de-hot-pathing Market would lift the submit/settle ceiling; sharding the slab + histogram would lift the accumulation ceiling. Whether that work is warranted is exactly what these numbers are meant to decide.

