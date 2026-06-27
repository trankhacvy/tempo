# Tempo Clearing Benchmark (CU profile)

Measured under LiteSVM 0.13 (in-process SVM). CU accounting is a faithful proxy for the on-chain meter; treat the *relative profile*, the *scaling*, and the *derived ceilings* as the signal, not the absolute digits. This is the CURRENT **unsharded** design (Market / OrderSlab / AuctionHistogram are each written on the hot path).

Solana limits used: **1400000 CU/tx**, **12000000 CU/account/block** (write-lock).

## submit_order — CU vs slab occupancy

Writes Market + OrderSlab. The cost grows with occupancy because `find_free_slot` and the per-trader-cap count scan the slab (O(n)).

| orders already resting | CU |
|---|---|
| 1 | 8269 |
| 32 | 14734 |
| 64 | 9214 |
| 120 | 10054 |

## process_chunk — fold cost

Writes Market + OrderSlab + AuctionHistogram. Folding is O(orders) but the per-order cost is small next to the fixed base (event CPI + account I/O), so we report the clean endpoints: one order vs a full 128-order slab in a single chunk.

| orders folded | CU |
|---|---|
| 1 | 10390 |
| 128 | 33735 |

Incremental: ~**10207 CU base** + ~**183 CU/order**. A single chunk tx could fold ~**7594** orders under the 1400000 CU/tx limit — far more than a slab can hold — so **folding compute is not the constraint**; the slab's single-account size cap is.

## finalize_clear — CU vs num_ticks

One transaction; a single O(ticks) pass over the 4 histogram regions (both crosses).

| num_ticks | CU |
|---|---|
| 64 | 38810 |
| 128 | 58590 |
| 256 | 102650 |

At the max supported 256 ticks finalize uses ~102650 CU — 7.3% of the 1400000 CU/tx limit, so the discovery pass fits comfortably in one tx across the whole tick range.

## settle_fill — CU

One order per tx (writes Market + OrderSlab, + Position when filled). Includes the marginal-tick cumulative scan. Measured: **20773 CU**.

## The hard ceiling: single-account size (not compute)

Solana caps a CPI-created/grown account at **10_240 bytes** per instruction (`MAX_PERMITTED_DATA_INCREASE`). The OrderSlab (~72 bytes/order) and the AuctionHistogram (~32 bytes/tick) are created this way, so a single market is capped at roughly **140 orders/auction** and **~310 ticks** — *regardless of compute budget*. The program enforces `orders_per_auction_cap ≤ 128` and `num_ticks ≤ 256` accordingly. Reaching the "thousands of orders" goal therefore requires either pre-sizing the accounts over multiple realloc transactions, or **sharding the slab/histogram** across several accounts — which would *also* relieve the write-lock contention below. This is the single most important measured constraint.

## Throughput of one full auction (≤128 orders)

Every hot-path tx write-locks the Market account, so the whole round's CU competes for Market's 12M-CU/block budget. Modelling a full 128-order auction end-to-end:

| phase | txs | CU each | CU total |
|---|---|---|---|
| submit | 128 | ~10054 (occ 120) | ~1286912 |
| accumulate | 1 | ~33735 | ~33735 |
| finalize | 1 | ~102650 | ~102650 |
| settle | 128 | ~20773 | ~2658944 |
| **total Market write-lock** | | | **~4082241 CU** |

A full 128-order auction puts ~4082241 CU on the Market write-lock — about 34.0% of one block's 12M budget — so a single market clears a **full auction in ~1 block(s)**. Submission and settlement (128 txs each, serialized on the shared Market/OrderSlab locks) dominate; clearing itself is negligible.


## Reading the result

- The clearing math (`finalize_clear`) is **not** the bottleneck — it is O(ticks), one tx, and fits the per-tx limit comfortably across the whole supported tick range.
- The ceiling is **write-lock contention on the shared accounts** (Market / OrderSlab / Histogram). Submission, accumulation and settlement each serialize per market within a block.
- `submit_order` and `settle_fill` cost grows with slab occupancy (the O(n) scans) — the slab-scan/free-list optimization and de-hot-pathing Market would lift the submit/settle ceiling; sharding the slab + histogram would lift the accumulation ceiling. Whether that work is warranted is exactly what these numbers are meant to decide.

