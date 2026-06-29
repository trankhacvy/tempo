# Tempo — Enhancement & Hardening Report

> Critical-enhancement review of the on-chain `program/` only. Produced by re-reading
> the full source with a security-first, performance-obsessed lens across four
> subsystems (DFBA clearing · money/PnL/liquidation/funding · oracle/admin · scalability).
> Findings marked **[verified]** were confirmed by direct source inspection during this
> review; the rest are located precisely (`file:line`) for you to confirm.

Severity legend: **Critical** = fund loss / systemic insolvency / corrupt accounting on a
real deployment · **High** = serious safety, economic, or scaling defect · **Medium** =
exploitable-but-bounded or correctness-under-stress · **Low** = defense-in-depth / hygiene.

---

## 1. Executive Summary

Tempo's **clearing core is genuinely strong**: `find_cross` / `compute_marginal_fill` are
deterministic, order-independent (commutative folding), conserve volume exactly via a
telescoping cumulative-floor that rounds against the user, and are fuzz- and Kani-guarded.
The completeness check (`all_active_orders_accumulated`) is a real, slab-anchored
anti-censorship guarantee, the freeze model cannot be wedged by a griefer withholding their
own settle, and the money path's per-event conservation (`conserve_and_socialize` fails
closed, never mints) is sound to the integer. The architecture and conventions are
disciplined. This is a high-quality MVP of a hard mechanism.

**But it is not yet safe to hold real value.** The single most important bug corrupts the
price of *every* trade on any oracle-centered market (CR-1). The access-control and
market-creation model is unfinished: market creation is permissionless and all markets of a
given mint **share one insurance pool**, which — combined with no minimum-margin floor and a
self-chosen per-market authority — is a direct honeypot to drain honest markets (CR-2). The
collateral ledger is not mint-scoped, a latent cross-collateral drain the moment a second
collateral mint ships (CR-3). On performance, the measured write-lock contention on the
shared `Market` account is largely **self-inflicted by redundant counters** and is cheap to
remove, while the 10 KB single-account ceiling (≈128 orders / 256 ticks) is the real
structural limit that only sharding fixes.

### Top 5 risks right now

| # | Risk | Severity |
|---|------|----------|
| 1 | `finalize_clear` publishes the wrong **clearing price** (`(tick+1)·tick_size`, ignores `window_floor_price`) → every settled trade gets a fabricated entry price; PnL/margin/liquidation/fees all corrupt. **[verified]** | Critical |
| 2 | **Permissionless market creation + shared per-mint insurance + no min-margin floor + arbitrary oracle feed** → honeypot market drains honest markets' pooled insurance. | Critical |
| 3 | **`UserCollateral` not mint-scoped** (`[b"collateral", owner]`) while vaults are per-mint → cross-collateral vault drain as soon as a 2nd mint exists. **[verified]** | Critical (latent) |
| 4 | **ADL socializes the wrong side** on close/flip fills (`settle_fill`/`settle_maker_quote` pass post-fill `oi_new`, not pre-fill size like `liquidate`) → broken loss conservation. **[verified]** | High |
| 5 | **No governance / no emergency pause / no timelock**; upgrade-authority god-mode; `force_reset` gated only by self-chosen market authority and runnable mid-settle → selective erasure of trades, no kill-switch during an exploit. | High |

### Biggest architectural / DFBA opportunities

- **Stop write-locking `Market` on the hot path.** `submit_order` and `process_chunk` write
  `Market` *only* to bump counters the histogram/slab already maintain. Delete the mirrors
  and most measured contention disappears with no layout change.
- **Shard the slab + histogram.** Folding is commutative, so the histogram shards trivially
  by tick range; this breaks both the 128-order ceiling and the per-market write-lock,
  unlocking real within-market parallelism — the only path to the "thousands of orders" goal.
- **A pre-settlement freeze sub-window + strict deadline** closes the last-look/spoofing gap
  that currently undermines the DFBA's core "no speed advantage" claim.

---

## 2. Critical Issues (Must Fix Before Real Value on Testnet/Mainnet)

### CR-1 — `finalize_clear` publishes the wrong clearing price (ignores `window_floor_price`) **[verified]**

- **Location:** `instructions/finalize_clear/processor.rs:109-118` (`cross_price` closure); corrupted value consumed in `settle_fill/processor.rs` (`settle_price` → `apply_fill`, fees, margin re-lock, `last_bid/ask_fill_price`).
- **Description:** The clearing tick is mapped to a price with the legacy zero-anchored
  formula `(clearing_tick + 1) * tick_size`. The canonical inverse used everywhere else is
  `Market::tick_to_price = tick*tick_size + window_floor_price` (`market.rs:722-730`). These
  agree **only** when `window_floor_price == tick_size` (the genesis default). But
  `recenter_window` resets the floor to an oracle-centered value (`≈ oracle − (num_ticks/2)·tick_size`)
  at **both** `initialize_market` and **every** `start_auction`. So on any real market the
  published `bid/ask_clearing_price` is wrong by `(window_floor − tick_size)` — potentially
  orders of magnitude off. Tick *matching* math is unaffected (it's internally consistent);
  the **price label** of the cross is fabricated.
- **Why dangerous:** `settle_fill` feeds this price into `position.apply_fill()` as the VWAP
  entry, into `signed_protocol_fee` as notional, into the initial-margin re-lock, and into
  `last_bid/ask_fill_price` (which `mark.rs` then consumes for funding/liquidation). Every
  settled position's PnL, margin, liquidation line, and fees are computed against a price that
  never existed. Conservation against true economic value is broken.
- **Severity:** Critical.
- **Fix:** Use the canonical inverse. Capture `window_floor` in the first market borrow:
  ```rust
  let cross_price = |c: &CrossResult| -> Result<u64, ProgramError> {
      if !c.crossed { return Ok(0); }
      (c.clearing_tick as u64)
          .checked_mul(tick_size)
          .and_then(|off| off.checked_add(window_floor))   // <-- the fix
          .ok_or(TempoProgramError::MathOverflow.into())
  };
  ```
  Equivalently `market.tick_to_price(c.clearing_tick)`. Add a regression test that recenters
  the window and asserts `published_price == tick_to_price(clearing_tick)`.
- **Impact if unfixed:** Every oracle-anchored market settles at a fictitious price. The
  protocol is unusable for any non-toy deployment.

### CR-2 — Permissionless market creation drains the shared per-mint insurance pool (honeypot)

- **Location:** `instructions/initialize_market/accounts.rs` (no admin gate), `data.rs:124-140`
  (min-margin floor missing), `processor.rs:31-77` (oracle bound best-effort, feed_id
  caller-chosen); insurance keyed `[b"vault", collateral_mint]` (`vault.rs`), drawn in
  `liquidate/processor.rs:192-195`.
- **Description:** `initialize_market` has **no protocol authority check** — `authority` is
  whatever signer the caller passes, and `collateral_mint`, `oracle`, and `oracle_feed_id` are
  caller-chosen. The insurance pool is shared by *every* market of the same mint. Three
  enabling sub-defects compound it:
  - **No minimum maintenance-margin floor** — validation only rejects `0` or `> 5000` bps, so
    `maintenance = initial = 1 bps` (≈10000× leverage) is accepted (`data.rs:124-140`).
  - **Arbitrary oracle/feed binding** — nothing ties `oracle_feed_id` to the asset; a market
    can be labeled "SOL-PERP" but bound to any feed; the creation-time oracle read is
    best-effort, not required.
  - **Self-chosen authority** enables `force_reset` abuse (see HS-1).
- **Why dangerous:** An attacker creates a market with `collateral_mint = <real USDC>`,
  1-bps margin, bound to an easily-moved feed, sets themselves as authority, engineers bad
  debt on one of their own accounts, and the uncovered loss is drawn from the **shared** USDC
  insurance pool that honest markets funded. The paired account collects the gain. Net theft
  of pooled insurance + eventual protocol insolvency.
- **Severity:** Critical.
- **Fix (layered, all three):**
  1. **Gate creation:** `require(authority == GOVERNANCE)` (a hardcoded gov key / multisig
     PDA), or maintain a governance-curated allow-list of `(feed_id, mint)` pairs.
  2. **Isolate insurance per market:** change the vault/insurance seed to include the market
     (`[b"vault", market]` or a per-market insurance sub-account) so a rogue market can only
     burn its own insurance, never honest markets'.
  3. **Floor the risk params:** `require(maintenance_margin_bps >= MIN_MAINTENANCE_BPS)` (e.g.
     50) and a max-leverage floor on `initial_margin_bps`; require the oracle account be
     Pyth-owned and feed-matching *at creation*, not best-effort.
- **Impact if unfixed:** Direct, repeatable theft of insurance/collateral from honest markets.

### CR-3 — `UserCollateral` is not mint-scoped → cross-collateral vault drain **[verified]**

- **Location:** `state/user_collateral.rs:63-67` (seeds `[b"collateral", owner]`, no `mint`
  field) vs `state/vault.rs` (per-mint vaults); `deposit/processor.rs`, `withdraw/processor.rs`,
  `withdraw_cross/processor.rs`.
- **Description:** There is exactly one untyped collateral ledger per owner, but token
  balances live in per-mint vaults. Deposit credits the single ledger 1:1 with `amount` and
  only checks `vault.vault_token_account` + `uc.owner`. Nothing binds a balance to the mint it
  was deposited under.
- **Why dangerous:** The moment a second collateral mint is deployed (the per-mint vault seeds
  show this is intended), a user deposits the cheap mint (credits the shared ledger), then
  `withdraw`s against the **expensive** vault, debiting the same fungible balance and pulling
  out the expensive token. Balances, realized PnL, and insurance across mints are summed in
  one integer.
- **Severity:** Critical (latent — not exploitable while only one mint exists; trivially
  exploitable the instant a second ships). Fix **before** any multi-collateral work.
- **Fix:** Add `collateral_mint: Address` to `UserCollateral`, include it in seeds
  (`[b"collateral", owner, mint]`), and assert `uc.collateral_mint == vault.collateral_mint`
  in deposit/withdraw. One ledger per `(owner, mint)`.
- **Impact if unfixed:** Total cross-market insolvency once a 2nd mint is live.

### CR-4 — ADL socializes the wrong side on closing/flipping fills **[verified]**

- **Location:** `settle_fill/processor.rs:387` (passes `oi_new`) and
  `settle_maker_quote/processor.rs:332` (passes `oi_new`) vs the correct
  `liquidate/processor.rs:217` (passes pre-close `size_signed`); consumer
  `market.rs:494-523` (`winner_is_long = loser_signed_size < 0`).
- **Description:** `socialize_bad_debt` picks the cohort to charge from the **loser's** signed
  size. When a settle creates bad debt (the trader's realized loss exceeds their balance), the
  loss occurred on the position's **pre-fill** side — but `settle_fill`/`settle_maker_quote`
  pass the **post-fill** size `oi_new`. On a fill that **closes** the position `oi_new == 0`
  (→ always charges shorts); on a fill that **flips** it the sign inverts (→ charges the side
  that just *lost*, not the winners). `liquidate` does this correctly with the pre-close size.
- **Why dangerous:** The socialized-loss index is incremented on the cohort that did **not**
  profit from the loser; the cohort that actually profited is under-charged and later
  over-draws insurance. ADL conservation breaks; the wrong users are penalized.
- **Severity:** High (conservation-breaking, but only on bad-debt-generating settles with a
  closing/flipping fill).
- **Fix:** One line each — pass `oi_old` (already captured at `settle_fill:255`) instead of
  `oi_new`, matching `liquidate`. Add a conservation fuzz over close/flip + bad-debt.
- **Impact if unfixed:** Mis-socialized losses; insurance under-collection; wrong traders charged.

---

## 3. High Priority Enhancements

### 3A. Performance & Compute-Unit Optimization

| ID | Location | Issue → Recommendation | Effort |
|----|----------|------------------------|--------|
| PERF-1 | `submit_order:232-242`, `process_chunk:159-170`, `state/market.rs:75-165` | **`Market` write-locked on every submit/chunk only to bump `active_order_count`/`accumulated_order_count` — counters already mirrored by `AuctionHistogramHeader.accumulated_count` and the slab's `count`.** `finalize_clear` completeness already relies on the authoritative slab scan, using the Market counters only as an O(1) hint. **Delete the Market counter mirrors** and drive completeness off the histogram count + slab scan → removes `Market` from the submit and accumulate critical sections entirely (submits then serialize only on the slab). | Medium |
| PERF-2 | `state/order.rs:491-511` (`trader_resting_stats`), called `submit_order:89` | **Every submit does a full-slab O(n) scan** for anti-spam count + reduce-only headroom → O(n²)/round. `find_free_slot` and `find_order_by_id_hinted` are already O(1)-hinted; this is the last hot O(n). Track the per-trader resting count in O(1) (small counter on `Position`), and **gate the same-side sum behind `if reduce_only`** so the common open path is O(1). | Small–Medium |
| PERF-3 | `instructions/round.rs:41,69` | `start_auction` memsets the whole slab (~11 KB) + histogram (~8 KB) in one tx and write-locks all three accounts. Fine now, breaks the CU/size budget after sharding. **Lazily reset slots on reuse** (stamp `auction_id` per slot) or shard the reset into permissionless chunks. | Medium |
| PERF-4 | `finalize_clear` | Already efficient: one O(ticks) pass, ~102k CU at 256 ticks (7% of the per-tx limit). **Not a bottleneck — leave it.** | — |

### 3B. Scalability & Architecture

| ID | Location | Issue → Recommendation | Effort |
|----|----------|------------------------|--------|
| SCALE-1 | `order.rs:333-341`, `histogram.rs:159-167` | **10,240-byte single-account ceiling caps a market at ~128 orders / ~256 ticks** regardless of compute. The slab and histogram are also single per-market write-locks. **Shard both:** histogram by tick range (folding is commutative, so shard writes are independent and `finalize_clear` reads all shards, still O(ticks) total); slab by slot range (submits round-robin, chunks crank per-shard). Breaks the size cap **and** the within-market write-lock in one change. | Large |
| SCALE-2 | `Vault` written by `settle_fill`, `settle_maker_quote`, `liquidate`, `liquidate_cross`, `finalize_clear` | **The Vault is a second, under-documented serialization hot-lock** — every money-path settle write-locks the single per-mint insurance pool, so even after PERF-1 the 128 settles/round serialize on the Vault, not Market. **Accumulate per-fill conservation deltas and reconcile insurance in batch at round roll**, or shard insurance into per-shard sub-pools. At minimum document Vault as a co-equal lock in the CU model. | Large |
| SCALE-3 | `Position` seeds `[b"position", market, owner]`; off-chain discovery via `getProgramAccounts` | **No on-chain index of open positions** → liquidator/indexer must gPA-scan O(total positions) and are RPC-throttled; liquidation latency grows with protocol size, not at-risk subset. Emit position-lifecycle events (see SEC/IDX below) so an indexer maintains the open set incrementally; consider a per-market position bitmap for liquidator targeting. | Large (indexer) / Medium (events) |
| SCALE-4 | `state/market.rs:75-165` | `Market` mixes immutable config (tick_size, fees, bps, oracle, mint, feed_id) with hot mutable runtime (phase, deadline, OI, social indices, last-fill). Even after PERF-1, settle still writes OI/social on Market. **Split a small `MarketRuntime` PDA** off the read-only config so config is read-locked on the hot path. | Large |

### 3C. Security & Correctness Hardening

| ID | Location | Issue → Recommendation | Sev / Effort |
|----|----------|------------------------|--------------|
| HS-1 | `force_reset/processor.rs:19-48` | Gated only by the **self-chosen** `market.authority`, runnable in **any phase including mid-`Settling`**, and **emits no event**. Lets the authority settle favorable fills then wipe unfavorable unsettled ones — breaks OI/conservation, strands users, desyncs indexers. **Restrict to governance; allow only when provably wedged (`Settling` && deadline+grace exceeded); emit `MarketForceReset`.** | High / Small |
| HS-2 | program-wide; `entrypoint.rs` (no pause ix); `lib.rs` upgrade authority | **No governance, no timelock, no emergency pause/circuit-breaker.** Compromised upgrade key = instant total drain; no on-chain kill-switch during an oracle incident or exploit. **Add a `paused` flag (global or per-market) gating submit/deposit/withdraw/clearing, settable by a guardian; move upgrade authority to multisig+timelock.** | High / Medium |
| HS-3 | `funding/update_funding/processor.rs:75-91` | `period_fraction_bps` capped at **one** interval and `last_funding_ts` reset to `now` → hours of funding **permanently lost** across sparse cranks; the paying cohort is incentivized to keep updates sparse. **Carry leftover time forward: `last_funding_ts += consumed_secs`**, or accrue the full elapsed fraction (still clamping per-period *rate*). | High / Small |
| HS-4 | `margin.rs:98-131`, `liquidate/processor.rs:122-160` | **Liquidation is full-close-only, penalty capped at the position's (already-tiny) equity, paid as ledger credit, no minimum fee.** For a position near/under zero equity the penalty → 0, so rational keepers (who spend real lamports) won't act → recoverable losses become socialized bad debt. **Add a flat `min_liquidation_fee` taken from the maintenance buffer before equity is exhausted; add partial liquidation (close just enough to restore margin).** | High / Medium |
| HS-5 | `oracle.rs:211-236` | Single Pyth feed; the confidence gate only rejects *wide* intervals (a tight-but-manipulated print passes), and the solvency path has **no median/second-source and no deviation-vs-last-good cap**. A single misprint within confidence triggers mass liquidations with no veto. **Add a second oracle (Switchboard) with a band-agreement/median, and a max-deviation sanity cap vs last-good on the solvency price.** Pair with HS-2 pause. | High / Medium |
| HS-6 | `oracle.rs:220-223` | On **wide confidence** (exactly during crashes) `solvency_mark` returns an error instead of falling through to the frozen soft-stale path → **liquidations become impossible while positions go underwater fastest** → unbounded bad debt. **Allow liquidation to proceed on a frozen/last-good mark (or a widened conf tolerance) when confidence is wide.** | Medium / Small |
| HS-7 | `market.rs:174` (VWAP), `position.rs:174,181-185` | **VWAP entry is floored unconditionally**, which rounds *in the user's favor for longs* (lower entry → larger gain) — violating "round against the user." Realized PnL can exceed true value by up to `~closed_qty`, funded from insurance; repeatable with a maker rebate. **Store entry at higher precision (scaled numerator/denominator) or round entry up for longs / down for shorts.** Add an open→close conservation test. | Medium / Medium |
| HS-8 | `market.rs:512-516` + `position.rs:244-257` | **Bad-debt socialization double-floors** (index increment floored, per-position charge floored again) → `Σ charges < residual` → the **last winner's `settle_fill` reverts `InsuranceInsolvent`** on rounding dust, their owed gain stuck. **Round socialization against winners (`mul_div_ceil` for the index increment)** so `Σ charges ≥ residual`. | Medium / Small |
| HS-9 | `migrate_market/processor.rs:96-97`, `data.rs:17-32` | `migrate_market` writes `max_price_move_bps_per_slot` / `soft_stale_slots` with **no bounds**, bypassing `initialize_market`'s validation → an authority can disable the per-slot brake (`>10000`) or set an unbounded stale window. **Apply the same bounds as init, or retire the instruction.** | Medium / Small |
| HS-10 | `initialize_market/data.rs:114-118`, `oracle.rs:229-233` | `soft_stale_slots` is **unbounded at init** → solvency/funding can run off a frozen stale mark for an arbitrarily long outage, masking insolvency. **`require(soft_stale_slots <= MAX_SOFT_STALE_SLOTS)`** sized to a few minutes. | Medium / Small |
| HS-11 | `liquidate/processor.rs:155-160` | Liquidator's collateral ledger is credited after only `from_bytes_mut` — **no `validate_self` PDA check** (every other ledger write has one). Not exploitable today (runtime requires program ownership) but a defense-in-depth gap. **Add `lc.validate_self(...)`.** | Low / Small |
| HS-12 | `deposit/processor.rs:41-55`, `accounts.rs` | **No `token_program` id check, no mint check, credits face `amount`** — a Token-2022 transfer-fee mint would credit more than received, breaking `vault ≥ Σbalances + insurance`. Mostly mooted today by the legacy-only CPI (a Token-2022 account fails the call), i.e. safe by accident. **Explicitly assert the SPL token program id + mint == vault mint; if Token-2022 is ever wanted, use `TransferChecked` and credit `post−pre` actual received.** (Also closes part of CR-3.) | Low (High if T22 wired in) / Small |

### 3D. DFBA Mechanism Critique & Improvements

The clearing **arithmetic** is the best part of the codebase — I explicitly verified it is
conserving, order-independent, and rounds against the user (backed by the 20k-iter
`fuzz_full_book_conserves_oi` and Kani). The weaknesses are in the **mechanism around** the
math, not the math:

- **DFBA-1 — Last-look / spoofing around the 2-slot window (Medium).**
  `submit_order` and `cancel_order` are allowed right up to `phase_deadline_slot`, and
  `process_chunk` (permissionless) can freeze the batch in the **same slot** as the deadline
  (`process_chunk:58-65` gates on `slot >= deadline`, not `>`). Since the book is fully
  observable, a sophisticated actor can read it, insert/cancel a precisely-sized order, and
  immediately crank the freezing fold — effective last-look, plus classic spoofing (rest a
  large order to move the implied price, cancel at the edge). This undermines the **core
  "no speed advantage" value proposition.**
  **Fix:** add a no-submit/no-cancel **freeze sub-window** (`reject when slot >= deadline − FREEZE_SLOTS`),
  require the first `process_chunk` at `slot > deadline` (strictly), and widen
  `COLLECT_WINDOW_SLOTS` to a propagation-realistic value. For true sealed-batch semantics,
  consider commit–reveal order submission.

- **DFBA-2 — Mark (hence funding) steerable within ±`MARK_BAND_BPS` and *persistent* (Medium).**
  Mark is derived from the round's clearing prices clamped to ±5% of oracle, and
  `last_bid/ask_fill_price` **persist across rounds** until the next cross. One manipulated
  auction sets the funding mark for every subsequent funding update until the book crosses
  again — a trader can push clearing to `oracle·1.05` to collect funding on their own side.
  **Fix:** drive funding off a volume/time-weighted mark across rounds; decay stale last-fill
  prices toward oracle when a round doesn't cross; treat as a required pre-mainnet
  *simulation* item (the docs already flag funding stability as unproven).

- **DFBA-3 — Slab DoS for ~nothing (Medium).** `MAX_ORDERS_PER_TRADER = 8` and slab ≤ 128 →
  **16 funded wallets fill every round.** Dust orders reserve trivial margin that is fully
  refunded, so the only cost is tx fees; legitimate `submit_order` then reverts
  `OrderSlabFull` round after round. **Fix:** minimum order notional / minimum reserved
  margin, a small non-refundable anti-spam fee to insurance, or lowest-notional eviction when
  full.

- **DFBA-4 — Sell-side worst-case margin over-locks collateral (Medium, capital efficiency).**
  `submit_order:64` reserves a sell's initial margin at the **window top**
  (`≈ oracle + (num_ticks/2)·tick_size`), far above any plausible clearing price, so shorts
  over-reserve and can hit `InsufficientCollateral` at submit despite backing any realistic
  fill. Buys reserve at their own limit (tight), so the inefficiency is asymmetric against
  sell flow. **Fix:** bound the sell worst-case to a configurable band around oracle
  (`oracle·(1+max_clear_band_bps)`) instead of the full window top.

- **DFBA-5 — Maker-quote completeness rests on counters, not a structural scan (Low).**
  Order-side censorship is anchored by `all_active_orders_accumulated` (a slab scan that
  can't be faked); the maker side only checks `folded_maker_quote_count == active_maker_quote_count`
  (`finalize_clear:52-54`). If those `u64`s ever drift, discovery could run on an incomplete
  maker book with no safety net. **Fix:** add an analogous structural "all active quotes
  folded" check, or document+test the counter invariant explicitly.

- **DFBA-6 — `start_auction` window recenter uses an oracle print with no confidence check (Low).**
  `start_auction:107-119` recenters on `read_price(...).ok()` without `require_confidence`,
  and the caller chooses the slot → minor griefing of the next round's tradeable band
  (amplified by CR-1). **Fix:** apply the confidence gate before `recenter_window`, consistent
  with the funding/solvency paths.

---

## 4. Dead Code, Unused Components & Cleanup

| Location | What | Action |
|----------|------|--------|
| `state/order.rs:547-572` `cumulative_qty_before` | Zero callers — `settle_fill` now reads the per-order `cum_before` snapshot instead of re-scanning. | **Delete** (+ its tests). |
| `state/order.rs:448-461` `count_trader_orders` | Superseded by `trader_resting_stats` (its own doc says it fused the two old scans). Zero callers. | **Delete** (+ its tests). |
| `traits/instruction.rs:134-135` `Instruction::accounts()/data()` | Never called — processors use `ix.accounts`/`ix.data` fields directly. (`Instruction::parse` *is* used; keep it.) | Drop the two accessors or leave as harmless API. |
| `lib.rs:6-13` crate doc | **Wrong** — claims "no collateral movement, no SPL transfers, no margin/funding/liquidation yet," but the whole M3 money path is implemented and routed. | **Update** the doc. |
| `CLAUDE.md` "program id (placeholder!)" | Stale — `lib.rs:59` declares a real deployed devnet id. | Update note. |

> Note: `find_order_by_id` is **not** dead — it's the fallback inside `find_order_by_id_hinted`. Keep it.

---

## 5. Missing Critical & Important Features

### Tier 1 — Must Have (before meaningful TVL)

| Feature | Why it matters | Approach on Solana | New accounts / ix | Complexity |
|---------|----------------|--------------------|-------------------|-----------|
| **Governance + emergency pause + timelock** (HS-1/HS-2) | No kill-switch during an exploit/oracle incident; upgrade key is god-mode. | `GlobalConfig` PDA with `governance`, `guardian`, `paused`; gate hot ix on `!paused`; upgrade authority → Squads multisig + timelock. | `GlobalConfig` account; `pause`/`unpause`/`set_governance` ix. | Medium |
| **Curated or gated market creation + per-market insurance** (CR-2) | Permissionless creation + shared insurance = honeypot. | Admin/allow-list gate in `initialize_market`; insurance seed includes market. | none (seed change + migration). | Medium |
| **Mint-scoped collateral ledger** (CR-3) | Cross-collateral drain on 2nd mint. | Add `mint` to `UserCollateral` seeds + assert vs vault. | migrate `UserCollateral`. | Small–Medium |
| **Liquidation keeper economics: min fee + partial liquidation** (HS-4) | Underwater positions never close → insolvency. | Flat min fee from maintenance buffer; close-to-target-margin partial. | `min_liquidation_fee` field; modify `liquidate`/`liquidate_cross`. | Medium |
| **Robust oracle: second source + deviation cap** (HS-5/HS-6) | Single Pyth = SPOF; tight misprint passes; wide-conf blocks liquidation. | Switchboard as secondary; median/band agreement; last-good deviation cap; soft-stale liquidation path. | oracle account(s) per market; `oracle.rs` changes. | Medium–Large |
| **Comprehensive events for indexing** (M/IDX below) | A faithful indexer is currently impossible to build. | See IDX-1. | event structs only. | Medium |

### Tier 2 — High Value (safety / UX)

| Feature | Why | Approach | Complexity |
|---------|-----|----------|-----------|
| **Adaptive/clamped funding off a TWAP mark** (HS-3/DFBA-2) | Manipulable, lossy funding today. | VWAP/TWAP mark across rounds; carry-forward time accrual; decay stale fills to oracle. | Medium |
| **On-chain health-factor / position queryability** | Off-chain re-implementation of margin math drifts → failed/missed liquidations. | Read-only `position_health` ix reusing `liquidation_outcome`/`maintenance_margin`. | Small |
| **DFBA freeze sub-window / commit-reveal** (DFBA-1) | Restores the "no speed advantage" guarantee. | Reject submit/cancel in last `FREEZE_SLOTS`; strict deadline; optional commit-reveal. | Medium |
| **Slab anti-spam economics** (DFBA-3) | Cheap censorship of order flow. | Min notional + small non-refundable fee. | Small |
| **Per-position open-interest registry / bitmap** (SCALE-3) | gPA scans don't scale. | Per-market bitmap of live positions for liquidator targeting. | Medium |

### Tier 3 — Nice to Have / Future

- **Multi-collateral & Token-2022 support** — only *after* CR-3 + HS-12 (`TransferChecked`,
  credit actual received, reject/῾handle transfer-fee mints).
- **Slab + histogram sharding** (SCALE-1/2) — the real path past 128 orders; large, do it when
  throughput demand is proven (it's the project's own named M1 benchmark question).
- **Isolated vs cross margin** — already a clear, implemented design decision
  (`margin_mode`, `MarginAccount`); the backed-profit rule is correctly applied. Mostly done;
  needs the events + health view to be operable.
- **Insurance fund management ix** (top-up, fee routing, target ratio) — currently insurance
  only moves implicitly via fills/liquidations.

---

## 6. Prioritized Roadmap

| Phase | Contents | Effort |
|-------|----------|--------|
| **Phase 0 — Critical fixes (do first)** | CR-1 (price formula, ~1 line + test) · CR-4 (`oi_old` ADL fix, ~2 lines + fuzz) · CR-3 (mint-scope ledger) · CR-2 (gate market creation + per-market insurance + min-margin floor) · HS-1 (`force_reset` → governance, no mid-settle) | **Medium** (CR-1/CR-4 are tiny; CR-2/CR-3 carry migrations) |
| **Phase 1 — Performance & scalability wins** | PERF-1 (drop Market counter mirrors) · PERF-2 (O(1) submit) · HS-8 (socialization ceil) · dead-code cleanup (§4) | **Small–Medium** (high leverage, low risk) |
| **Phase 2 — Tier-1 missing features** | HS-2 (pause+timelock+governance) · HS-4 (keeper min-fee + partial liquidation) · HS-5/HS-6 (oracle hardening) · HS-3 (funding accrual) · IDX-1 (events) | **Large** |
| **Phase 3 — Architecture / DFBA improvements** | SCALE-1 (shard slab+histogram) · SCALE-2 (Vault lock) · SCALE-4 (Market config/runtime split) · DFBA-1 (freeze window) · DFBA-2 (TWAP funding) · DFBA-3/4 (anti-spam, sell-margin band) | **Large** |
| **Phase 4 — Polish & advanced** | HS-7 (VWAP precision) · HS-9/HS-10 (migrate/stale bounds) · HS-11 (validate_self) · multi-collateral + Token-2022 · insurance management ix · on-chain health view | **Medium** |

---

## 7. Additional Recommendations

### IDX-1 — Event design is the indexer blocker (do early)

The claim that "every state-changing instruction emits a typed event" is **false today**:

- `settle_maker_quote` emits **nothing** — maker fills are invisible.
- `deposit`, `withdraw`, `withdraw_cross`, `start_auction`, `force_reset`, `migrate_*` emit
  nothing — collateral flow, round rolls, and book wipes can't be tracked.
- `FillSettledEvent` (`events/fill_settled.rs:8-16`) carries only `{market, trader, order_id,
  auction_id, fill, side, is_maker}` — **no clearing price, no fee, no resulting size/entry,
  no realized PnL** → an indexer literally cannot compute notional or PnL from the stream.

**Fix:** add `settle_price, fee, new_size, new_entry_price, realized_pnl` to
`FillSettledEvent`; emit it from `settle_maker_quote`; add `Deposit`/`Withdraw`/`AuctionRolled`/
`MarketForceReset`/`Migrated` events; add a per-market monotonic event sequence number. This
unblocks the deferred API history endpoints and the SCALE-3 incremental position index.

### Testing strategy gaps (what to test that probably isn't)

- **Recentered-window settlement price** — the exact regression that would have caught CR-1
  (recenter, clear, assert published price == `tick_to_price(clearing_tick)`).
- **ADL conservation on close/flip + bad debt** — would catch CR-4 and HS-8 (assert
  `Σ social charges ≥ residual` and the correct side is charged).
- **Open→increase→close PnL conservation** — would catch HS-7 (assert realized PnL never
  exceeds true value for both long and short).
- **Multi-mint ledger isolation** — a LiteSVM test with two mints proving you cannot withdraw
  mint-B against a mint-A deposit (CR-3).
- **Funding accrual across sparse cranks** — assert total funding over a long gap equals
  sum-of-periods, not one period (HS-3).
- **Liquidation profitability in lamport terms** — assert the keeper nets > tx cost across the
  equity range, especially near-zero equity (HS-4).
- **Adversarial market-creation** — assert a 1-bps-margin / wrong-feed market cannot touch
  another market's insurance (CR-2).

### Observability / on-chain metrics worth adding

- Per-market: cumulative volume, open interest (already tracked — emit it), insurance balance,
  bad-debt-socialized total, number of ADL events.
- Per-round: clearing prices (both sides), matched volume, marginal-tick allocation, cranker.
- A `protocol_solvency` view: `vault_token_balance − (Σ balances + insurance)` should always
  be ≥ 0 — emit it so the invariant is monitorable, not just asserted in tests.

### Invariants to assert on-chain / document explicitly

- `vault_token ≥ Σ balances + insurance` (the conservation invariant — make it a runtime
  `debug_assert` at money-path exits, and a monitored emitted metric).
- `Σ oi_long == Σ oi_short` netting expectations once OI-netted PnL lands.
- `accumulated_count == active_count` is only a *hint*; the slab scan is truth — keep that
  comment, and add the maker-side structural check (DFBA-5).

---

### Closing note

The hard, novel part — the batch-auction clearing math — is done well and is defensible. The
gap to production is almost entirely in the **surrounding envelope**: access control / market
creation, the price-label bug, mint-scoping, liquidation economics, oracle robustness, and
event coverage. Phase 0 is small in code (CR-1 and CR-4 are a handful of lines) but decisive
for fund safety; do it before any value touches the program.
