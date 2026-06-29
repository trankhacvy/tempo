# Tempo `program/` — A Deep, Simple Explanation

This document explains the on-chain Solana program in the `program/` folder. It is
written in **simple English** so it is easy to follow even if English is not your
first language. The goal: after reading this, you understand **every feature** in
the program and **how the program handles each one**.

We only talk about `program/` (the smart contract). We do not talk about the
off-chain Rust code, the TypeScript client, or the docs folder.

---

## Table of contents

1. [The big picture: what Tempo is and the problem it solves](#1-the-big-picture)
2. [The core idea: a batch auction](#2-the-core-idea-a-batch-auction)
3. [The histogram trick (the "mailboxes")](#3-the-histogram-trick)
4. [The three-phase clearing protocol (the heart)](#4-the-three-phase-clearing-protocol)
5. [The dual auction (maker side + taker side)](#5-the-dual-auction)
6. [Marginal-tick rationing: how fills are shared fairly](#6-marginal-tick-rationing)
7. [The auction lifecycle (the phase machine)](#7-the-auction-lifecycle)
8. [How the code is organized](#8-how-the-code-is-organized)
9. [The account model (all the data structures)](#9-the-account-model)
10. [The full instruction set](#10-the-full-instruction-set)
11. [The money path: collateral, vault, deposit, withdraw](#11-the-money-path)
12. [Positions, funding, and PnL](#12-positions-funding-and-pnl)
13. [Mark price and the oracle](#13-mark-price-and-the-oracle)
14. [Liquidation](#14-liquidation)
15. [Risk hardening features](#15-risk-hardening-features)
16. [Cross-margin](#16-cross-margin)
17. [Maker quotes (parametric liquidity)](#17-maker-quotes)
18. [Account migration](#18-account-migration)
19. [Security properties (why it is safe)](#19-security-properties)
20. [Low-level details: zero-copy layout and events](#20-low-level-details)
21. [Known gaps (not bugs)](#21-known-gaps)

---

## 1. The big picture

**Tempo is a perpetuals DEX** (a decentralized exchange for "perpetual futures")
that runs directly on Solana. A perpetual future ("perp") is a contract that lets
you bet on the price of something (for example SOL) going up or down, using
leverage, without an expiry date.

**The problem Tempo fixes:** Most exchanges match trades **continuously** — the
moment an order arrives, it is matched. This sounds good, but it creates a race:
the **fastest bot wins**. Fast bots jump ahead of normal users, and they extract
value (this is called **MEV**, "maximal extractable value"). Speed becomes more
important than having a good price.

**Tempo's answer: a Dual Flow Batch Auction (DFBA).** Instead of matching one
order at a time, Tempo:

1. **Collects** all orders during a short time window.
2. At the end of the window, it **clears them all together at ONE single price**.

Everyone in the same batch gets the **same price**. Being 1 millisecond faster
gives you no advantage, because your order joins the same batch as everyone
else's. This removes the speed race and most MEV.

> Think of it like a school auction. Instead of selling each item to whoever
> shouts first, the teacher collects everyone's secret bids, then finds one fair
> price that clears the market. Speed of shouting does not matter.

The **core deliverable** is the **clearing engine** (the auction math). On top of
that, the program adds a real **money path** (collateral, deposits, withdrawals),
**risk management** (funding, liquidation, insurance), and **advanced features**
(cross-margin, maker quotes).

---

## 2. The core idea: a batch auction

A **uniform-price batch auction** works like this:

- Buyers say "I will buy up to X at price P or lower."
- Sellers say "I will sell up to Y at price P or higher."
- The exchange finds the **one price** where the amount people want to buy equals
  the amount people want to sell. That price is the **clearing price**.
- Everyone trades at that single clearing price.

The hard part on a blockchain: a normal exchange keeps the whole **order book**
(the list of all orders) in memory and sorts it. On Solana, memory and compute
("compute units", CU) per transaction are **very limited**. If you have thousands
of orders, you cannot load and sort all of them in one transaction.

Tempo solves this with a clever data structure: the **price histogram**.

---

## 3. The histogram trick

### The key mathematical insight

You do **not** need the full order book to find the clearing price. You only need
the **cumulative demand and supply at each price level**. A clearing price can be
recovered from **sums alone**.

So instead of storing every order, Tempo stores a **histogram** (a bar chart) over
**price ticks**.

- A **tick** is one small price step. The market has a fixed number of ticks
  (`num_ticks`, at most 256), and each tick is `tick_size` apart.
- For each tick, the histogram keeps a few `u64` counters: "how much quantity
  wants to buy here" and "how much quantity wants to sell here."

The team calls these counters the **"mailboxes."** Each order drops its quantity
into the correct mailbox (price bucket), and then disappears from the math. We
never need all orders in memory at once.

### Why this is powerful

The cost of clearing is **O(ticks)**, not O(orders). Whether there are 10 orders
or 10,000 orders, the histogram has the same fixed size (it depends only on the
number of price ticks). This makes clearing **cheap and predictable** on Solana.

The histogram lives in the `AuctionHistogram` account. Its size is
`header + 4 regions × num_ticks × 8 bytes` (we explain the 4 regions in
[section 5](#5-the-dual-auction)).

### Why "folding" is the secret to safety

Adding an order's quantity into a mailbox is just **integer addition**:

```
mailbox[tick] = mailbox[tick] + order_quantity
```

Addition is **commutative**: `a + b = b + a`. The order in which you add things
does **not** change the final sum. This is the single most important security
property of Tempo, and we will return to it many times. It means: **no matter who
processes the orders, or in what order, the final histogram is identical.** A
hostile actor cannot rig the price by choosing a clever sequence.

The folding function is `fold(...)` in `state/histogram.rs`, and it uses
**checked addition** so it can never silently overflow.

---

## 4. The three-phase clearing protocol

This is **the heart of Tempo**. Clearing is split into three kinds of cheap,
**permissionless** transactions. "Permissionless" means **anyone** can send them —
you do not need to be a special, trusted operator. The person who sends these
transactions is called a **cranker** (they "crank the handle" of the machine).

The three phases are: **ACCUMULATE → DISCOVER → SETTLE**.

### Phase 1 — ACCUMULATE (`process_chunk`)

**Goal:** fold resting orders into the histogram.

- A cranker calls `process_chunk` with a start index and a max count. The program
  processes a **bounded slice** of the order slab (for example, orders 0 to 50).
  Many such transactions together cover all orders. This is why it is "chunked" —
  no single transaction has to do everything.
- For each order that is still `Resting`, the program:
  1. Converts the order's price to a tick.
  2. **Folds** (adds) the order's quantity into the correct histogram mailbox.
  3. Marks the order as `Accumulated` (so it can never be folded twice).
  4. Records a `cum_before` snapshot — the bucket's value *just before* this order
     was added. This snapshot is used later for fair rationing (see
     [section 6](#6-marginal-tick-rationing)).
- It bumps two counters: `accumulated_order_count` on the market and on the
  histogram.

**Phase transition:** the very first `process_chunk` moves the market from
`Collect` to `Accumulating` — but only **after the collection window has closed**
(it checks `Clock.slot >= phase_deadline_slot`). This keeps the order book open
for the full window, so every order in the window joins the same batch.

**Why this is safe:** because folding is commutative (addition), two crankers
folding different chunks in any order produce the exact same histogram. The
`Accumulated` flag stops double-folding (counting an order twice). The completeness
check in the next phase stops skipping (ignoring an order).

(File: `instructions/process_chunk/processor.rs`.)

### Phase 2 — DISCOVER (`finalize_clear`)

**Goal:** find the single clearing price and write the result. This is **one
transaction**.

- First, a **completeness check**. The program refuses to run unless
  **every active order has been folded exactly once**. It checks this in two ways:
  1. A fast counter hint: `accumulated_order_count == active_order_count` (and the
     same for maker quotes).
  2. A real, trustworthy scan of the order slab: `all_active_orders_accumulated`
     confirms **no slot is still `Resting`**. This second check is the real
     guarantee — it does not trust the counters, it looks at the actual data.
  - If anything is still unfolded, it returns `AuctionNotComplete`.
- Then it reads the histogram's four regions into arrays and runs the clearing
  math: **`find_cross`** (the crown jewel, in `clearing.rs`). It runs it **twice**
  — once for the bid auction, once for the ask auction
  ([section 5](#5-the-dual-auction)).
- It writes a `ClearingResult` account holding the clearing prices, the matched
  volumes, and the rationing constants for both auctions.
- It moves the phase to `Discovered` and records the last fill prices.
- **Crank fee (optional):** if the cranker supplies their collateral account and
  the vault, the program pays them a flat `crank_fee` from the insurance pool, as
  a reward for doing the work. This is conserving (money moves inside the vault, it
  is not created).

**An important DoS protection:** the program **does not trust** the bump byte the
caller sends for the `ClearingResult` PDA. It re-derives the canonical address
itself with `find_program_address`. If a caller tried to create the result at a
wrong address, a later `settle_fill` would reject it and the market would be stuck
forever. By computing the address itself, the program prevents this permanent
denial-of-service.

(File: `instructions/finalize_clear/processor.rs`.)

### Phase 3 — SETTLE (`settle_fill`)

**Goal:** give each trader their fill. This is **one transaction per user**.

Fills are **pulled, not pushed**. Each `settle_fill` settles exactly one order, so
the cost of writing to that user's position is paid in **that user's own
transaction**. This spreads the work across many transactions instead of one giant
one.

For one order, the program:

1. Finds the order in the slab (using `order_id` and a `slot_hint`).
2. Checks the order is `Accumulated` (folded but not yet settled).
3. Chooses which auction it belongs to (a sell → bid auction, a buy → ask
   auction).
4. Computes the fill amount with **`fill_against_cross`** — the single shared fill
   classifier (so taker fills and maker fills always use the exact same boundary
   and never drift apart).
5. Marks the order `Consumed` and reduces the slab `count`.
6. **Releases** the worst-case margin that was reserved when the order was
   submitted.
7. If `fill > 0`, it **requires the position account** and applies the trade:
   updates size, average entry price, realized PnL, funding, and social loss.
8. If a collateral account and vault are supplied, it flushes realized PnL, charges
   or rebates the protocol fee, re-locks margin to the new size, and conserves the
   money through the insurance pool.

**One key safety rule:** a non-zero fill is **never silently thrown away**. The
position account is **mandatory** whenever `fill > 0`. Otherwise a malicious
cranker could "consume" your matched trade with empty accounts and destroy it. Only
a **zero-fill** order (it matched nothing) may be consumed cheaply without a
position.

(File: `instructions/settle_fill/processor.rs`.)

---

## 5. The dual auction

Tempo does not run one auction — it runs **two auctions at the same time**, which is
why it is called a **Dual** Flow Batch Auction.

There are two kinds of participants:

- **Takers** — normal traders who submit orders (`submit_order`). They want to
  trade now.
- **Makers** — liquidity providers who post standing quotes (`MakerQuote`). They
  provide the liquidity.

The two auctions are:

1. **Bid auction** = maker-buys (demand) vs taker-sells (supply).
2. **Ask auction** = taker-buys (demand) vs maker-sells (supply).

To keep these separate, the histogram has **four regions** (`NUM_REGIONS = 4`):

| Region        | Filled by             | Meaning                        |
|---------------|-----------------------|--------------------------------|
| `BidDemand`   | maker buy quotes      | demand side of the bid auction |
| `BidSupply`   | taker sell orders     | supply side of the bid auction |
| `AskDemand`   | taker buy orders      | demand side of the ask auction |
| `AskSupply`   | maker sell quotes     | supply side of the ask auction |

- Taker orders (from `submit_order`) are **taker-only** and fold only into
  `BidSupply` (a sell) or `AskDemand` (a buy).
- Maker quotes fold only into `BidDemand` (their buys) and `AskSupply` (their
  sells).

In `finalize_clear`, the program runs `find_cross` once for the bid auction
(`BidDemand` vs `BidSupply`) and once for the ask auction (`AskDemand` vs
`AskSupply`). It publishes **both** prices in the `ClearingResult`. Each order
settles against its own auction.

---

## 6. Marginal-tick rationing

This is a subtle but very important piece of math. It answers: **when there is not
enough volume to fill everyone at the clearing price, who gets filled, and how
much?**

### The setup

`find_cross` finds the clearing tick where cumulative demand and supply cross. At
that exact "marginal tick," one side usually has **more** quantity than the other.
The smaller side is fully filled; the bigger side must be **rationed** (shared).

The rules `fill_against_cross` applies:

- Orders **strictly better** than the marginal tick (a buy above it, a sell below
  it) are **filled in full**. They are competitive, so they always trade.
- The **scarce side** (the smaller side) at the marginal tick is **filled in
  full**.
- The **rationed side** (the bigger side) at the marginal tick is filled
  **pro-rata** (in proportion), using `compute_marginal_fill`.

### The conservation trick (telescoping floors)

The danger with pro-rata sharing is **rounding**. If you round each person's share
independently, the parts might not add up to the whole — you could create or
destroy quantity. That would break the books.

Tempo avoids this with a **telescoping cumulative-floor** formula. Each order
remembers `cum_before` — how much quantity was in front of it in the same bucket
(captured during ACCUMULATE). Its fill is:

```
fill = floor((cum_before + qty) × V / Q) − floor(cum_before × V / Q)
```

where `V` is the volume allocated to the marginal tick and `Q` is the total
quantity at it.

Because each person's "end" is the next person's "start," the floors **cancel out
across everyone** (they "telescope"). The sum of all fills is **exactly `V`** — not
one unit more or less. No quantity is created or lost. Any tiny rounding loss
(at most one unit of "dust") rounds **against the user**, never against the
protocol.

This also means the result is **independent of settle order**. It does not matter
who settles first; the totals always conserve. (Functions: `find_cross`,
`compute_marginal_fill`, `fill_against_cross` in `clearing.rs`. They have many unit
tests plus differential fuzz tests with 20,000+ iterations.)

---

## 7. The auction lifecycle

One market reuses the same accounts round after round. A market walks through a
**phase machine**:

```
Collect  →  Accumulating  →  Discovered  →  Settling  →  (next round) Collect
   0            1               2              3
```

- **Collect (0):** the book is open. Traders submit and cancel orders. Makers
  update their quotes. The window stays open for `COLLECT_WINDOW_SLOTS = 2` slots.
- **Accumulating (1):** crankers fold orders into the histogram (`process_chunk`,
  `process_maker_quote`).
- **Discovered (2):** `finalize_clear` has found the prices and written the result.
- **Settling (3):** each trader pulls their fill (`settle_fill`,
  `settle_maker_quote`).

### Rolling to the next round (`start_auction`)

`start_auction` is **permissionless** and rolls the market into the next round. It
only succeeds when the previous round is **fully settled** (phase is `Settling` or
`Discovered`, **and the order slab is empty** — every order is `Consumed`). Then it:

- Bumps `current_auction_id`.
- **Zeroes** the histogram buckets and the slab slots (so `Consumed` slots can be
  reused — they are never freed otherwise).
- Resets the counters and reopens `Collect`.
- **Re-centers the tick window on the current oracle price** (best-effort): it
  reads a fresh, confidence-checked oracle price and calls `recenter_window`. If
  the oracle is stale or uncertain, it keeps the old window and the roll still
  succeeds. A bad feed **delays** recentering, it never **halts** the market.

### The "freeze model" (no pipelining)

A new round **cannot open until the previous one is completely settled**. There is
no overlapping of rounds. This is a deliberate design choice (system-design §7).
The failure mode is **delay, not loss**: if no one cranks, the round just waits.
Anyone can step in and keep cranking, because all the crank instructions are
permissionless.

### Emergency reset (`force_reset`)

If a round gets stuck in a bad way, the **market authority** (admin) can call
`force_reset`. This is the **only** non-permissionless escape hatch. It bumps the
auction id and resets the round to `Collect` using the same shared
`reset_round_to_collect` helper that `start_auction` uses.

---

## 8. How the code is organized

The program is built with **Pinocchio** — a `no_std`, zero-copy, zero-dependency
framework for Solana programs. "Zero-copy" means the program reads account data
directly in place, without copying it into new structures (this saves precious
compute). It uses **Codama** to generate the IDL (the interface description that
clients use).

### Code flow

```
lib.rs           declares the program ID, modules, #![no_std]
   ↓
entrypoint.rs    reads the 1-byte discriminator, routes to a process_* handler
   ↓
instructions/*/  one folder per instruction: accounts.rs · data.rs · processor.rs
   ↓
clearing.rs      pure clearing math (find_cross, fill_against_cross, ...)
state/*.rs       zero-copy account structs
```

### The strict layout rules (conventions)

The codebase follows a strict, consistent style. When you add something, you
**mirror the closest existing instruction**:

- **No logic in `mod.rs`** — only module declarations.
- **All validation lives in `TryFrom`.** `accounts.rs` validates accounts;
  `data.rs` validates and parses the instruction data; `processor.rs` contains
  **only the business logic**. So validation and logic are cleanly separated.
- **No floating point anywhere.** Only `u64` / `u128` (and `i128`) with checked or
  saturating math. Always **round against the user**.
- **No magic numbers** — every constant is named.
- **Permissionless cranks are treated as adversaries.** Correctness must come from
  the math (commutativity + completeness), never from trusting the caller.
- **Single source of truth** — the program ID is `crate::ID`, referenced
  everywhere; never copied.

---

## 9. The account model

All state lives in **PDAs** (Program Derived Addresses — accounts the program
controls). Every state struct is **zero-copy** `#[repr(C)]` with a 2-byte prefix:
**1 byte discriminator** (which type it is) + **1 byte version** (which layout
version). `assert_no_padding!` guarantees the struct has no hidden padding bytes.

Here are all the accounts:

### `Market` (disc 1, the main account)

Seeds: `[b"market", market_seed]`. This is the central config and state for one
market. It holds (among many fields):

- Auction state: `current_auction_id`, `phase`, `phase_deadline_slot`.
- Histogram config: `tick_size`, `num_ticks`, `window_floor_price`.
- Order bookkeeping: `accumulated_order_count`, `active_order_count`,
  `orders_per_auction_cap`.
- Last prices: `last_bid_fill_price`, `last_ask_fill_price`.
- Risk config: `maintenance_margin_bps`, `initial_margin_bps`,
  `liquidation_penalty_bps`, `max_position_notional`.
- Fees: `maker_fee_bps`, `taker_fee_bps` (signed — negative means a rebate),
  `integrator_share_bps`, `crank_fee`.
- Funding: `funding_index` (i128), `last_funding_ts`.
- Oracle: `oracle`, `oracle_feed_id`, `collateral_mint`.
- Risk hardening: `oi_long`, `oi_short` (open interest, u128),
  `social_loss_index_long`, `social_loss_index_short` (i128), the braked effective
  price fields, `max_price_move_bps_per_slot`, `soft_stale_slots`.
- Maker quote counters: `next_quote_id`, `active_maker_quote_count`,
  `folded_maker_quote_count`.

Key helper methods: `price_to_tick` / `tick_to_price` (map price ↔ tick using the
window floor), `recenter_window` (center the window on the oracle each round),
`advance_effective_price` (the price brake), `apply_oi_delta` (keep open interest
in step), `socialize_bad_debt` (ADL to the winning side), `validate_price`.

### `AuctionHistogram` (disc 2, the "mailboxes")

Seeds: `[b"histogram", market]`. A header plus a `4 × num_ticks` region of `u64`
buckets. `fold_buy` / `fold_sell` do the checked, commutative addition. Its size
depends only on the tick count, **not** the order count.

### `OrderSlab` (disc 4)

Seeds: `[b"orderslab", market]`. An array of `Order` slots, bounded by
`orders_per_auction_cap`. Each `Order` (88 bytes) holds price, quantity, remaining,
order_id, trader, side, status (`Empty=0` / `Resting=1` / `Accumulated=2` /
`Consumed=3`), the `cum_before` snapshot, and `reserved_margin`. The header has a
`next_free_hint` cursor for fast O(1) slot allocation. Helpers find free slots,
look up orders by id, check completeness, and compute prefix sums.

### `ClearingResult` (disc 3)

Seeds: `[b"clearing", market]`. A small fixed result holding, for **both** the bid
and ask auctions: the clearing price, matched volume, the volume allocated to the
marginal tick, total quantity at the marginal tick, the marginal tick index, and
which side was rationed. Each user reads these constants to self-compute their fill.

### `Position` (disc 5, version 3)

Seeds: `[b"position", market, owner]`. One trader's position in one market:

- `size` (signed i64: positive = long, negative = short),
- `entry_price` (the average entry price, a VWAP),
- `collateral` (locked margin for this position),
- `realized_pnl` (i128), `last_funding_index` (i128),
- `last_social_index` (i128, for ADL — added in version 2),
- `margin_mode` (0 = isolated, 1 = cross — added in version 3).

Methods: `apply_fill` (update VWAP and realize PnL on reduce/flip),
`settle_funding`, `settle_social_loss` (charge the current side only — it never
credits), `snapshot_social_index`.

### `UserCollateral` (disc 7)

Seeds: `[b"collateral", owner]`. A trader's money ledger:
`balance`, `locked`, and `free() = balance − locked`. Methods: `credit`, `debit`,
`lock`, `release`, `apply_pnl` (returns any bad debt if a loss is bigger than the
balance).

### `Vault` (disc 6, version 2)

Seeds: `[b"vault", collateral_mint]`. The shared collateral pool:
`collateral_mint`, `vault_token_account`, `insurance_balance`, `authority_bump`,
`bump`. The vault authority PDA (`[b"vault_authority", ...]`) signs token transfers
out of the vault.

### `MarginAccount` (disc 9)

Seeds: `[b"margin", owner]`. A cross-margin group: up to
`MAX_CROSS_POSITIONS = 8` member position keys that share one `UserCollateral`
ledger. (It is **not** in the IDL because its fixed `[u8; 256]` array does not map
to a Codama node; clients read its layout directly.)

### `MakerQuote` (disc 8, version 3)

Seeds: `[b"maker_quote", market, maker]`. A maker's standing parametric quote (a
price ladder). See [section 17](#17-maker-quotes).

---

## 10. The full instruction set

The first byte of every instruction is a **discriminator** that picks the handler
(`entrypoint.rs`). Here is the complete list:

| # | Name | What it does |
|---|------|--------------|
| 0 | `InitializeMarket` | Create a market + its histogram + its order slab |
| 1 | `SubmitOrder` | A taker submits an order (Collect phase) |
| 2 | `CancelOrder` | A trader cancels a resting order (Collect phase) |
| 3 | `ProcessChunk` | ACCUMULATE: fold a chunk of orders into the histogram |
| 4 | `FinalizeClear` | DISCOVER: find clearing prices, write the result |
| 5 | `SettleFill` | SETTLE: pull one order's fill |
| 6 | `StartAuction` | Roll into the next round |
| 7 | `InitPosition` | Create a trader's position account |
| 8 | `ReadOracle` | Read the oracle, compute mark, emit an event (read-only) |
| 9 | `InitVault` | Create the global collateral vault |
| 10 | `InitCollateral` | Create a trader's collateral ledger |
| 11 | `Deposit` | Move tokens in, credit the ledger |
| 12 | `Withdraw` | Debit the ledger, move tokens out |
| 13 | `UpdateFunding` | Accrue funding from oracle vs mark |
| 14 | `Liquidate` | Liquidate an unhealthy position |
| 15 | `ForceReset` | Admin escape hatch for a stuck round |
| 16 | `InitMakerQuote` | Create a maker's quote |
| 17 | `UpdateMakerQuoteMid` | Move a quote's center tick (O(1) re-quote) |
| 18 | `UpdateMakerQuoteLevels` | Replace a quote's full ladder |
| 19 | `ClearMakerQuote` | Deactivate a quote |
| 20 | `ProcessMakerQuote` | ACCUMULATE a maker quote into the histogram |
| 21 | `SettleMakerQuote` | SETTLE a maker quote's fills |
| 22 | `InitMarginAccount` | Create a cross-margin group |
| 23 | `AddPositionToMargin` | Add a flat position to a group |
| 24 | `WithdrawCross` | Withdraw against combined cross-margin equity |
| 25 | `LiquidateCross` | Liquidate a cross-margin account |
| 26 | `MigrateMarket` | Upgrade an old Market account (v4 → v5) |
| 27 | `MigratePosition` | Upgrade an old Position account (v1/v2 → v3) |
| — | `RemovePositionFromMargin` | Remove a flat position from a group |
| — | `CloseMakerQuote` | Close an inactive quote, refund rent |
| 228 | `EmitEvent` | Internal self-CPI used to emit events |

### `submit_order` in more detail

When a taker submits an order during `Collect`:

- It validates the phase and the price (the price must fall inside the histogram
  window).
- **Anti-spam:** one trader may hold at most `MAX_ORDERS_PER_TRADER = 8` resting
  orders in one auction.
- **Pre-trade margin reservation (very important):** Because a batch auction only
  discovers the price **after** matching, the program reserves, at submit time, an
  **upper bound** on the margin the fill could ever need. It locks
  `worst_qty × worst_price × initial_bps`. A buy can clear at most at its limit
  price; a sell can clear at most at the window top. By locking the worst case now,
  `settle_fill` only ever **releases** margin — it can never fail for lack of
  collateral (which would wedge the whole round). A `reduce_only` order reserves
  only the part that would open new exposure, so closing a position is never
  blocked.
- **Position size limit:** if `max_position_notional` is set, the order's
  worst-case **new** exposure is capped. A pure reduce/close adds zero new exposure
  and is never blocked.
- It writes the order into a free slot and bumps the counters.

### `cancel_order`

Permissionless to trigger but the trader must sign. Only during `Collect`. It
erases the order, decrements the counts, and **releases the reserved margin** using
the same shared `release_order_reservation` helper that `settle_fill` uses (so the
two release sites can never drift apart).

---

## 11. The money path

### The vault and the ledger

- **`Vault`** is the single shared pool. It owns an SPL token account holding all
  collateral, plus an `insurance_balance` used to absorb losses and pay rebates.
- Each trader has a **`UserCollateral`** ledger: `balance`, `locked`, and
  `free = balance − locked`.

### Deposit (`deposit`)

The trader signs. The program transfers tokens from the trader's token account
into the vault's token account (an SPL `Transfer` CPI), then **credits**
`balance += amount`. The vault token account is checked against the address stored
in the vault, and the ledger owner is checked.

### Withdraw (`withdraw`)

The trader signs. The program **debits** the ledger (`debit` fails if the amount is
more than `free`, so locked margin is protected), then transfers tokens out of the
vault to the trader. The transfer is signed by the **vault authority PDA** (seeds
`[b"vault_authority", authority_bump]`).

### The conservation invariant

The core safety rule of the money path: **`vault tokens ≥ Σ all balances +
insurance`** at all times. Whenever a trader's balance changes (PnL), the opposite
change happens in the insurance pool, so money is never created. A gain is funded
**from** insurance and **fails closed** (`InsuranceInsolvent`) if insurance is too
small — the program **never mints money**. A loss accrues **to** insurance. This
logic lives in the shared `settle_money.rs` (`conserve_and_socialize`), used by
`settle_fill`, `settle_maker_quote`, `liquidate`, and `liquidate_cross` so all four
paths behave identically.

---

## 12. Positions, funding, and PnL

### A position

`Position` stores a signed `size` (long or short), an average `entry_price`, locked
`collateral`, and accumulated `realized_pnl`. `apply_fill`:

- **Increasing** the position: updates the VWAP entry price.
- **Reducing or flipping**: realizes the PnL on the closed part.

### Funding

Funding keeps the perp price tied to the real (oracle) price. It is a periodic
payment between longs and shorts:

- If the mark price is **above** the oracle, longs pay shorts (and vice versa).
- The program uses a **monotonic funding index** (`funding_index`, i128, scaled by
  `FUNDING_SCALE = 1e9`). Each position remembers the index value at its last
  settlement (`last_funding_index`); the difference is what it owes or receives.

`update_funding` is **permissionless**. It:

- Reads the oracle (must match the market's feed, be fresh within
  `MAX_AGE_SECS = 120`s, and pass the confidence check `DEFAULT_MAX_CONF_BPS = 500`).
- Computes the mark price anchored to the oracle within `MARK_BAND_BPS = 500` bps.
- Computes the period rate: `period_fraction_bps = (elapsed × 10000 /
  FUNDING_INTERVAL_SECS).min(10000)`, where `FUNDING_INTERVAL_SECS = 3600` (1
  hour), capped at `MAX_FUNDING_RATE = FUNDING_SCALE / 100` (≈1% per period).
- Advances `funding_index` and stamps `last_funding_ts`.

Each position settles funding lazily inside `settle_fill` / `liquidate` via
`settle_funding`, which moves the owed amount into realized PnL.

### PnL math (no floats)

- `unrealized_pnl = size × (mark − entry)` (signed).
- The fee on a fill is `signed_protocol_fee` (negative = rebate). Takers pay
  `taker_fee_bps`; makers pay `maker_fee_bps`.
- Big multiplications use `wide_math.rs` (256-bit `mul_div_floor` / `mul_div_ceil`)
  so `qty × price × bps` can never overflow even at extreme sizes.

---

## 13. Mark price and the oracle

### The oracle (`oracle.rs`)

Tempo reads **Pyth** price feeds (the `PriceUpdateV2` format). The reader is
`no_std` and parses the bytes directly. It checks:

- The account is owned by the Pyth receiver (`PYTH_RECEIVER_ID`).
- The feed id matches the market's bound feed.
- The price is positive and not stale (older than `MAX_AGE_SECS = 120`s is
  rejected) and not from the future.
- Confidence (uncertainty) is within `DEFAULT_MAX_CONF_BPS = 500` bps — a very
  uncertain price is refused.

Prices are normalized to a fixed `1e8` scale (`price_1e8`).

### Mark price (`mark.rs`)

`compute_mark_price` decides the "fair" price used for risk:

- If both auctions crossed → the **midpoint** of the two clearing prices.
- If only one crossed → that side.
- If neither crossed → the **oracle** price.
- The result is always **clamped to a band** around the oracle (±`MARK_BAND_BPS`),
  so a manipulated fill price cannot move the mark too far.

### `read_oracle`

A read-only, permissionless instruction that reads the live oracle, computes the
mark, and emits an event. It is used to prove end-to-end oracle integration on
devnet without changing any state.

---

## 14. Liquidation

A position is **liquidatable** when its **equity** falls below the **maintenance
margin** requirement. `liquidate` is **permissionless** — anyone can liquidate an
unhealthy position and earn the penalty as a reward.

The math (`margin.rs`, `liquidation_outcome`):

- `maintenance_margin = |size| × mark × maintenance_margin_bps / 10000`.
- `equity = collateral + realized_pnl + unrealized_pnl` (priced at the mark).
- Liquidatable if `equity < maintenance_margin`.
- `penalty = |size| × mark × liquidation_penalty_bps / 10000` → paid to the
  liquidator.
- `returned_to_owner = max(0, equity − bad_debt)` → what is left for the owner.
- `bad_debt` = the loss beyond the collateral.

The flow in `liquidate`:

1. Read the oracle (fresh → advance the braked effective price; soft-stale → use
   the frozen price).
2. Settle the position's funding and social loss, then **zero it out** (size,
   collateral, entry, realized).
3. Release the owner's locked collateral, apply the loss, return any leftover.
4. Credit the liquidator's ledger with the penalty.
5. Adjust the vault insurance: insurance absorbs the bad debt up to its balance;
   any **residual** beyond insurance is **socialized** to the winning side by open
   interest (ADL — see next section).
6. Update the market's open interest.

---

## 15. Risk hardening features

These are the "M3-v1.5" features that make Tempo robust under stress.

### Open interest (OI) tracking

The market tracks `oi_long` and `oi_short` (total long and short size). Every fill
and liquidation calls `apply_oi_delta` to keep these exact. OI is used to share
losses fairly during ADL.

### ADL / socialized loss

When a liquidation creates **bad debt** larger than the insurance pool, the loss
cannot just vanish (that would break conservation). Instead it is **socialized to
the winning side** in proportion to their open interest. This is **Auto-Deleverage
(ADL)**.

The mechanism uses a per-side **social loss index** (`social_loss_index_long`,
`social_loss_index_short`, both i128). `socialize_bad_debt` raises the index of the
winning side. Each position later pays its share via `settle_social_loss`, which
only ever **charges** the current side, **never credits** (so it cannot be gamed).
A freshly opened position re-snapshots the index so it never pays for losses that
happened before it existed.

### Hard solvency gate

Any gain that would be paid out is checked against the insurance pool first. If the
pool cannot cover it, the transaction **fails closed** (`InsuranceInsolvent`). The
program never pays out money it does not have.

### Per-slot price brake

`max_price_move_bps_per_slot` plus the braked **effective price** logic
(`advance_effective_price`, `clamp_price_step`) limit how far the risk price can
move in a single slot. This stops a single manipulated update from causing a
cascade of liquidations.

### Oracle soft-stale fallback

`solvency_mark` (oracle.rs) defines three states:

- **Fresh** — a normal recent oracle price; used directly.
- **Frozen** — the price is slightly stale (within `soft_stale_slots`); the last
  good price is "frozen" and used, so the market keeps running.
- **Hard-stale** — too old; risk operations halt rather than act on bad data.

### Overflow-safe notional math

`wide_math.rs` provides 256-bit `mul_div_floor` / `mul_div_ceil`. This means
`quantity × price × bps` can never overflow, even for huge positions.

### Formal verification

`kani_proofs.rs` runs the Kani model checker on the raw arithmetic (`find_cross`,
`unrealized_pnl`, `wide_mul`) to **prove** there are no panics / overflows /
underflows. The heavier correctness properties (which the model checker cannot
fully explore) are covered by **differential fuzz tests** (50k iterations, no
external dependencies).

---

## 16. Cross-margin

By default each position is margined **in isolation** — its own collateral backs
only itself. **Cross-margin** lets one trader group several positions so that a
**profit on one offsets a loss on another**. The account is judged by **one
combined equity** vs **one combined maintenance** requirement.

### How it works

- `init_margin_account` creates a `MarginAccount` group for the owner.
- `add_position_to_margin` adds a **flat** position (size 0, collateral 0) to the
  group and sets its `margin_mode = 1`. It checks there are no in-flight orders, so
  a resting order cannot settle as isolated after the mode flips.
- `remove_position_from_margin` removes a flat member (and compacts the array so
  the slot is reusable).

### The completeness rule (the key safety idea)

Any operation that extracts value (`withdraw_cross`, `liquidate_cross`) must see
**every member position** of the group. If a user could hide a losing position,
they could withdraw money they do not really have. So the instruction **requires
all members to be supplied** and **fails closed** if any is missing.

To tell the program which members are live (have a position) vs flat, the
instruction uses a **`live_mask` u8 bitmap** — one bit per member. A **live** member
needs a `(position, market, oracle)` triple; a **flat** member needs only the
`position` account. The supplied account count must exactly equal
`live_count × 3 + flat_count`. (Because the mask is a u8, a group is capped at 8
members.)

### Combined equity math (`cross_margin.rs`)

Each member contributes a `LegContribution`:

- `equity = realized + recognized_unrealized − pending`
- `maintenance = |size| × mark × bps / 10000`

The single knob `credit_unrealized_gains` distinguishes the two callers:

- **Liquidation** uses `true`: it marks to the true price, so both gains and losses
  count toward whether the account is underwater.
- **Withdrawal** uses `false` (the **backed-profit rule**): only **losses** dock
  equity; unrealized **paper gains are not credited** toward what you may withdraw.
  You cannot withdraw profit that is not yet backed by real settled money.

`liquidate_cross` closes **one** member position per call (the first live one),
realizes its PnL, charges the penalty, and socializes any shortfall — repeated
calls wind the account down in bounded steps.

---

## 17. Maker quotes

Makers provide liquidity not with single orders but with a **parametric quote** — a
**price ladder** described by a few parameters. This is `MakerQuote`.

### The ladder

A quote is anchored to a center tick `mid_tick`. Each level `k` has an `offset` and
a `size`:

- Bid level `k` rests at `mid_tick − offset_k` (a buy below the center).
- Ask level `k` rests at `mid_tick + offset_k` (a sell above the center).

There are up to `MAX_LEVELS = 8` levels per side. The brilliant part: to re-quote
(move all your prices), the maker only changes `mid_tick` — that is **O(1)**
(`update_maker_quote_mid`). The levels themselves rarely change
(`update_maker_quote_levels` replaces them fully when needed).

A quote has a `delegate` (who may edit the ladder but never move money), a
`sequence` nonce (replay protection — each edit must use a higher number), and an
`expiry_slots` clock (the quote is skipped if it gets too old).

### Folding maker quotes (`process_maker_quote`)

This is the maker side of ACCUMULATE. For each active, not-yet-folded, not-expired
quote, the cranker folds:

- Each bid level into `BidDemand[mid_tick − offset]`.
- Each ask level into `AskSupply[mid_tick + offset]`.

It captures a `cum_before` **snapshot** per level (the bucket value before this
fold) for fair rationing later. Levels that fall off the price grid are skipped and
keep the `SNAPSHOT_UNFOLDED` sentinel (= `u64::MAX`), so they fill **zero** in
settlement — a never-folded level can never mint a position.

**Fold-once idempotency:** each quote stores `folded_auction_id`. If it already
equals the current round, folding is a no-op. After folding, it sets the id and
bumps `folded_maker_quote_count` (used by the completeness check in
`finalize_clear`).

### Settling maker quotes (`settle_maker_quote`)

This is the maker side of SETTLE. For each level it calls the **same**
`fill_against_cross` classifier the taker path uses (so maker and taker fills can
never drift and stop netting to the matched volume). It uses each level's saved
snapshot for marginal-tick rationing. It sums the bid fills and ask fills, applies
them to the maker's position (funding, social loss, VWAP, realized PnL), charges the
maker fee, re-locks margin, and conserves the money through insurance.

**Settle-once idempotency** uses `settled_auction_id`. It also **requires** the
quote to have been folded this round (`folded_auction_id == current_auction_id`),
otherwise it fails — a quote cannot settle a round it never participated in.

### Lifecycle helpers

- `init_maker_quote` — create the quote, register it active.
- `clear_maker_quote` — deactivate (status = 0), zero the ladder, decrement the
  active count. The account stays (rent trapped) until closed.
- `close_maker_quote` — close an inactive quote and refund the rent to the maker.

---

## 18. Account migration

Because the on-chain accounts have evolved over versions, the program can **upgrade
old accounts in place** without losing data. Migration grows the account
(`realloc`), zero-initializes the new tail, fills in any fields that need values,
and bumps the version byte.

- **`migrate_market`** (disc 26): upgrades a **version 4** Market to **version 5**
  (the risk block: OI, social-loss indices, effective price, price brake,
  soft-stale config; plus the later window-floor and pre-trade-risk fields). It is
  authority-gated. After it runs, `oi_long`/`oi_short` start at 0 and are rebuilt as
  positions migrate.
- **`migrate_position`** (disc 27): upgrades a **version 1 or 2** Position to
  **version 3**. It is owner-gated. A v1 upgrade also **re-adds the position's size
  to the market's open interest** (because `migrate_market` reset OI to 0). It
  requires the order slab to be empty (quiescent) so no in-flight settle can race
  the OI rebuild.

Both migrations target the **exact prior version** — they check the version byte
first and refuse anything else. Always verify a deployed account's version before
migrating.

---

## 19. Security properties

Tempo's safety does **not** depend on trusting the people who send crank
transactions. It comes from math and strict checks:

1. **Commutativity.** Folding orders into the histogram is integer addition, so the
   final histogram is identical no matter who cranks, in what order. A hostile
   cranker cannot rig the price by sequencing.

2. **Completeness.** `finalize_clear` refuses to run until **every** order is
   folded — and it confirms this by scanning the slab itself, not just trusting a
   counter. The only residual crank attack is **censorship** (refusing to fold an
   order), and that just causes **delay**, because anyone else can fold it.

3. **Exact conservation.** The telescoping floor in `compute_marginal_fill`
   guarantees the sum of fills equals the matched volume exactly. Open interest is
   conserved. Rounding always goes **against the user**, never against the protocol.

4. **Fail-closed money.** The vault never pays out more than it has
   (`InsuranceInsolvent`). Gains are funded from insurance; losses and bad debt go
   to insurance or are socialized via ADL. Money is never created.

5. **No silent loss of a trade.** A non-zero fill always requires the position
   account; a matched trade can never be quietly discarded by a malicious settler.

6. **DoS resistance.** `finalize_clear` derives the canonical `ClearingResult`
   address itself, so a bad bump cannot wedge the market permanently.

7. **Permissionless but bounded.** All cranks are open to anyone, so liveness does
   not depend on one operator; but they are bounded (chunked) so they fit in
   Solana's compute limits.

---

## 20. Low-level details

### The `le_field!` macro and align-1 layout

This is a subtle but important detail. Account data is **pointer-cast at byte
offset 2** (after the 1-byte discriminator + 1-byte version). Offset 2 is **not
8-byte aligned**. If a struct had a native `u64` field, reading it at an unaligned
address is **undefined behavior** (it actually panicked on the host before this was
fixed).

The fix: every multi-byte integer in a zero-copy state struct is stored as a
**little-endian byte array** (`[u8; N]`), which keeps the struct alignment at 1. The
`le_field!` macro generates getter/setter accessors that read/write these byte
arrays correctly. So you will see fields like `tick_size_le: [u8; 8]` with a
`tick_size()` accessor. **Rule: when adding a numeric field to a state struct, use
`le_field!`, never a bare `u64`.**

### Events

Every state-changing instruction emits an event so off-chain indexers can follow
along. Events are emitted via a **self-CPI** through the `EmitEvent` instruction
(discriminator 228). This is indexer-friendly and avoids log truncation. There is
an `event_authority` PDA, and each instruction carries trailing `event_authority` +
`tempo_program` accounts. The event structs live in `events/` (`MarketInitialized`,
`OrderSubmitted`, `OrderCancelled`, `ChunkProcessed`, `ClearingFinalized`,
`FillSettled`, and more).

**Important rule:** a CPI requires no outstanding account borrows — the code always
reads fields into local variables and drops the `try_borrow` guards **before**
calling `emit_event`.

### Errors

`errors.rs` defines `TempoProgramError` (using `thiserror` + Codama errors). Each
error converts into a `ProgramError::Custom`. Examples you will meet:
`AuctionWrongPhase`, `AuctionNotComplete`, `AuctionIdMismatch`,
`InsufficientCollateral`, `InsuranceInsolvent`, `MissingSettleAccounts`,
`PositionLimitExceeded`, `TraderOrderCapReached`, `InvalidOrderStatus`.

---

## 21. Known gaps

These are **deliberate** — they are documented decisions, not bugs:

- **Tick window** is a fixed-size window centered on the oracle each round.
  Production might want a more dynamic window (clearing-protocol §6.4).
- **PnL backing** is "v1.1 conserving" — PnL flows through the insurance pool and is
  conserved, but true **OI-netted** mark-to-market between longs and shorts (where
  longs' and shorts' PnL directly offset each other continuously) is a later
  upgrade.
- **The dual auction is fully implemented and tested in code** (both `find_cross`
  passes, the four regions, both settle paths), but the **clearing *simulations***
  on the dual maker/taker structure, and validation on **live devnet** (only
  LiteSVM tests so far), are still pending.
- The genuinely **open research questions** — histogram write-lock contention,
  period clock vs. multi-slot clearing, max orders per auction — are the **point of
  the M1 benchmark**. They are measurements to produce, not code to "fix."

---

## One-paragraph summary

**Tempo** is a perps DEX on Solana that replaces the speed-race of continuous
matching with a **batch auction**: orders are collected over a short window and all
cleared at **one uniform price**. The magic that makes this cheap on-chain is the
**price histogram** ("mailboxes"): the book is reduced to cumulative sums per tick,
and folding orders into it is **commutative integer addition**, so the result is
the same no matter who cranks. Clearing is split into three permissionless,
bounded phases — **ACCUMULATE** (fold), **DISCOVER** (find the price, with a strict
completeness check), and **SETTLE** (each user pulls their fill, conserved exactly
by a telescoping-floor rationing). It runs as a **dual auction** (maker side + taker
side) over four histogram regions. On top of this pure clearing engine sits a full
**money and risk system**: a collateral vault and ledger, pre-trade margin
reservation, oracle-anchored funding and mark price, liquidation, an insurance pool
with **ADL/socialized loss**, a hard solvency gate, a price brake, soft-stale oracle
fallback, overflow-safe 256-bit math, position limits, **cross-margin**, and
**parametric maker quotes** — all built so correctness comes from **math and strict
checks**, never from trusting whoever sends the transaction.
