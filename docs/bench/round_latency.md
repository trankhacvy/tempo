# Round latency & the Stage-C2 decision (P5.1)

Two measurements, one decision. **Measured 2026-07-08** on the live devnet
market `DQi8KnRwvLCMdGBwP1SLoWXSTMubpWzkxmxQqbK2zdLd` (Phase-4 binary,
`8gpz…wnJD`), driven by the P5.2 keeper (pipelined resets + early roll) with
the reference mm-bot, two sim traders, and the liquidator — i.e. the normal
orchestrator stack, public devnet RPC, no co-location.

## 1. Devnet wall-clock per phase — 48 complete rounds (auctions 28–78)

Derived from the orchestrator log's mm-bot ticks (~0.85 s sampling), so ±1 s
per cell. Post-processor: one state = first observation of `(round, phase)`;
duration = time to the next state.

| phase | mean s | median s | p90 s | min s | max s |
|---|---|---|---|---|---|
| collect | 19.7 | 22.4 | 23.4 | 11.3 | 32.8 |
| accumulate | 27.5 | 26.3 | 36.4 | 14.4 | 39.8 |
| discovered (settle begins) | 20.3 | 21.9 | 29.6 | 10.9 | 38.8 |
| settling (tail + roll) | 37.6 | 34.1 | 64.2 | 2.6 | 75.3 |
| **full round** | **105.0** | **104.5** | 133.9 | 74.2 | 146.6 |

The **serial tail** C2 would overlap (phases 2+3: settle-all + reset-all +
roll): **median 56.6 s, mean 57.9 s — 55 % of the round**. The collect window
it would overlap into: mean 19.7 s.

The P5.2 pipelining is visible in the log (`Settle { …, resets: [0, 2] }` —
drained shards resetting while other shards' fills settle), and the roll is a
single `start_auction` when `shards_ready == num_slab_shards`.

## 2. What the tail is made of — the CU model (LiteSVM, deterministic)

`benchmark.rs::benchmark_round_tail_model`, 16 shards × 90 orders/shard
(1,440 orders — ~200× the live round's order count):

| phase | txs | CU/tx | Σ CU |
|---|---|---|---|
| intake (submit_order) | 1440 | 7,951 | 11,449,440 |
| accumulate (process_chunk ×shards) | 16 | 33,775 | 540,400 |
| discover (finalize_clear) | 1 | 169,542 | 169,542 |
| settle (settle_fill ×orders) | 1440 | 20,868 | 30,049,920 |
| reset (reset_shard ×shards) | 16 | 12,301 | 196,816 |
| roll (start_auction) | 1 | 11,225 | 11,225 |

Serial tail (settle+reset+roll): **1,457 txs, Σ 30.26 M CU ≈ 2.5
Market-write-locked blocks ≈ ~1 second of chain capacity** — at a load two
hundred times heavier than the measured live rounds.

## 3. Decision — C2 (double-buffered round overlap): **NO-GO** (deferred, recorded)

The plan's literal trigger ("tail < collect window ⇒ C2 dead") did **not**
fire — the wall-clock tail (56.6 s) exceeds the collect window (19.7 s) by
~3×. But the premise behind that trigger — that the tail is *capacity-bound
work* whose only escape is overlapping it with the next round — is falsified
by the CU model: even at 1,440 orders/round the whole tail is ~1 s of chain
time. The measured 57-second tail is **per-transaction confirmation latency
at trivial load** (~4–8 orders/round: one settle round-trip each at devnet
confirm times, a serial maker-quote settle, and one keeper poll per phase
transition), not throughput.

That changes which lever is cheapest:

- **C2 would buy** a round cadence of at best `max(head, tail)` ≈ 60–70 s
  (-35 %), at the price of the riskiest change in the scaling plan — two live
  rounds sharing one durable book (parity-seeded histogram + clearing PDAs,
  cross-round status disambiguation on every order slot).
- **The keeper can buy the same or more with zero on-chain risk**: pack
  multiple `settle_fill`s per transaction (flood-style batching — 8 settles =
  1 confirmation round-trip), raise settle concurrency, and fire phase
  transitions without awaiting the prior action's confirm (P5.2 already did
  this for resets, measurably).

**Decision: do not build C2 now.** `known-issues.md` §2.14 stays closed-as-
deferred with this record as the reason.

**Re-open trigger (mechanical, not vibes):** implement C2 only when a
*loaded* benchmark (≥1,000 orders/round live, or any production market)
shows the settle tail **CU-bound** — i.e. tail wall-clock tracking
`Σ settle CU / 12 M CU-per-block` (chain capacity) rather than
`orders × confirm-latency / concurrency` (keeper latency). Until keeper-side
batching is implemented and shown insufficient, C2 is premature.

## Reproducing

```bash
# CU model (LiteSVM):
cargo test -p tempo-integration-tests --test benchmark \
  benchmark_round_tail_model -- --ignored --nocapture

# Devnet wall-clock: run the orchestrator ≥50 rounds, then post-process its
# log (the script lives in the session scratchpad; it derives per-phase spans
# from the mm-bot tick lines' auction_id/phase fields).
```
