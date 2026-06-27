# Tempo — Overview

Tempo is an open-source **batch-auction perpetual-futures DEX** on Solana L1. Instead
of matching trades the instant they arrive, it collects orders over a short window and
clears them together at a single uniform price. This removes the speed/latency advantage
that continuous order books reward, and with it the MEV that comes from racing to
reorder other people's trades.

This document explains *why* batch auctions and *how* Tempo runs one fully on-chain. The
mechanism is described in detail in `tempo-clearing-protocol.md`; the architecture in
`system-design.md`; the money/risk layer in `risk-model.md`.

---

## 1. The problem with continuous matching

Almost every exchange matches trades one by one, in time order. This sounds fair but
rewards whoever has the fastest connection and the fastest bot — not whoever offers the
best price. On a blockchain it is worse: bots race to grab stale prices and to reorder
other people's transactions for profit (MEV), and regular traders pay through worse
fills.

The fix is well established (Budish, Crampton & Shim, 2015 — *"The High-Frequency
Trading Arms Race"*): instead of matching continuously, **collect all orders in a short
window, then match them together at one fair clearing price.** Inside the window,
arriving early gives no advantage; the race for speed disappears and competition shifts
back to price. Traditional markets already use this for their most important prices — a
stock exchange's open and close are set by a batch auction, not continuous trading.

The on-chain batch-auction mechanism Tempo implements is the **Dual Flow Batch Auction
(DFBA)** described by Jump Crypto (2025): each round runs *two* auctions — a bid auction
(maker-buys vs taker-sells) and an ask auction (taker-buys vs maker-sells) — each
clearing at its own single price. Tempo's contribution is a fully on-chain, L1-native,
fully-settling implementation of that mechanism **for perpetuals**, with the two hard
problems below solved.

---

## 2. The two hard problems Tempo solves

### A — Who runs each auction, and why trust them?

On Solana there is no built-in timer; code runs only when someone sends a transaction.
So *someone* must trigger the matching at the end of each window. Whoever does that is in
a powerful position: if they are offline the auction never runs, and if they are
dishonest they could delay it or sneak their own order in first.

**Tempo makes triggering safe for everyone, no matter who does it:**

- **Anyone can trigger it, and the result is fixed.** The clearing price is a pure,
  deterministic function of the resting orders. Whoever runs it computes the same answer,
  and the program rejects any wrong answer. If one trigger party is offline or hostile,
  anyone else can run that auction instead.
- **The price cannot be biased by ordering.** Orders are folded into a price histogram by
  integer addition, which is commutative — so the result is identical no matter who folds
  which orders in what order. There is no privileged sequencing position to exploit.
- **Orders cannot be censored.** The program refuses to finalize until *every* resting
  order has been folded in exactly once, and because folding is permissionless, a
  censored order's owner (or anyone) can include it themselves.

This turns a trusted, fragile role into an open, trustless one. (Full argument and the
tests behind each claim: `tempo-clearing-protocol.md §4`.)

### B — How does clearing fit inside Solana's limits?

A uniform-price auction must, in principle, look at *every* order to find the one price
that maximizes matched volume — and the whole book does not fit in a single L1
transaction. Tempo's answer: a clearing price is recoverable from **cumulative sums
alone**, so the book is represented as a fixed-size **histogram over price ticks**. Its
size depends on the tick count, never on the number of orders. Clearing then decomposes
into many cheap transactions whose cost is independent of order count:

1. **Accumulate** — fold a bounded slice of orders into the histogram (repeat until all
   are folded). Commutative, so any trigger party can run any slice.
2. **Discover** — one pass over the buckets finds the clearing price and the fill rules.
3. **Settle** — each trader *pulls* their own fill in their own transaction, so position
   writes are spread across accounts instead of one hot account.

(Full method and simulation results: `tempo-clearing-protocol.md`.)

---

## 3. How a round works, end to end

Each market repeats the same cycle:

1. **Collect** — traders submit orders into the order book during a short window.
2. **Accumulate** — orders are folded into the price histogram (permissionless).
3. **Discover** — the single clearing price(s) and fill allocation are computed and
   published (permissionless).
4. **Settle** — each trader claims their fill; positions update, the clearing price
   becomes the new mark, and funding accrues.

Everything runs on-chain — there is no off-chain matching engine. The price comes only
from real buyers and sellers competing, never from a pool or a formula. On top of the
clearing engine sits a full perpetuals money layer: SPL collateral custody, oracle-priced
funding and liquidation, an insurance fund, socialized-loss handling, and cross-margin —
all detailed in `risk-model.md`.

---

## 4. Honest limits

- **Liquidity is the real-world risk.** A perps venue needs traders on both sides;
  bootstrapping that is a separate challenge from the mechanism itself.
- **Throughput has open questions.** How many orders one auction can clear per block —
  given Solana's per-account write-lock budget — is a measured number, not a guarantee
  (`system-design.md §7`).
- **Some perp mechanics need more validation.** Batch-perp funding stability and the
  dual-auction simulation are areas still being hardened before mainnet
  (`tempo-clearing-protocol.md §6`, `risk-model.md`).

The clearing arithmetic is implemented, tested, and hostile-trigger-resistant. The
remaining work is systems integration and economic hardening, drawn explicitly in the
design docs rather than glossed over.

---

## Credits

- **Jump Crypto** — the *Dual Flow Batch Auction (DFBA)* mechanism Tempo implements.
- **Budish, Crampton & Shim (2015)** — the economic case that continuous order books are a
  flawed design and frequent batch auctions fix them.
