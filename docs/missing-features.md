# Tempo — Missing Features

This document tracks the trading/risk/admin functionality a production perps DEX
needs on top of the clearing engine. It is separate from `known-issues.md`
(defects/limitations in code that already exists).

**Status: the backlog is complete.** The matching and clearing engine was
already done (sharded order book, price histogram, three-phase clearing, dual
auction, resting orders, always-open submission); the whole trading/risk/admin
layer that turns it into an operable exchange has now been built across
`plan.md` phases 0–5 and adversarially reviewed. Every numbered item below is
**DONE** except two closed **by design**:

- **§7.2 inventory / skew management** — the maker's off-chain job, not on-chain state.
- **FOK / post-only order types** (part of §2.3) — FOK breaks the telescoping-floor
  conservation; post-only *is* the maker-quote book (`system-design.md` §8).

One thing stays **deliberately deferred** (a product decision, not a gap):
**multi-mint collateral** — the system runs on a single collateral mint (USDC);
the number of markets is unbounded but they share one ledger/vault (see
`known-issues.md` §2.3 and §8 below). The residual limitations and coverage
gaps of what *is* built live in `known-issues.md`.

Status tags: **done** (built) · **won't build** (closed by design).

### Status at a glance

| Item                                              | Status        | Note                                                                                  |
| ------------------------------------------------- | ------------- | ------------------------------------------------------------------------------------- |
| 1.1 collateral reservation at submit              | done        | `Order.reserved_margin`; rejected at submit (`InsufficientCollateral`)                |
| 1.2 position cap + initial-margin buffer          | done        | `initial_margin_bps` + `max_position_notional` + per-side `max_open_interest` soft cap (increment-gated at submit, never blocks de-risking) |
| 1.3 `initialize_market` param validation          | done        | structural + fee + risk-config bounds in `data.rs` `TryFrom`                          |
| 2.1 close / reduce-position instruction           | done        | SDK `close_position` (opposite-side reduce-only market order — the auction IS the venue); `ClosePosition` (44) reclaims a flat position's rent |
| 2.2 reduce-only flag                              | done        | forces `Consumed` at settle; reserves FULL worst-case margin (DDR-3 Correction-2)     |
| 2.3 order types beyond resting limit              | done        | GTC/GTT expiry + IOC (`expires == arm round`) + market orders (SDK, window-boundary price); FOK/post-only deliberately not built (system-design §8) |
| 2.4 partial-fill carry-over                       | done        | resting orders (Stage B): unfilled/partial remainder re-arms `Resting` and carries to the next round        |
| 2.5 remove-from-group for cross margin            | done        | `RemovePositionFromMargin` (disc 28) + compacting `remove_member`                     |
| 2.6 minimum order size / notional                 | done        | `min_order_notional` (`OrderBelowMinimum`, hot-updatable)                             |
| 2.7 cancel-all / batch cancel / expiry            | done        | `CancelAllOrders` (43): owner-only per-shard batch cancel, one summed release         |
| 3.1 update-market / set-risk-params               | done        | `UpdateMarketParams` (33, hot) + staged `Propose/ApplyRiskUpdate` (34/35, 3k-slot delay) |
| 3.2 pause / halt / resume                         | done        | `SetPause` (32): intake/roll bitflags; exits never pause                              |
| 3.3 set-oracle / repoint feed                     | done        | staged `Propose/ApplySetOracle` (38/39), paused-only + delay-gated                    |
| 3.4 close-market / delist / authority transfer    | done        | `CloseMarket` (45, quiescence-gated) + `ClosePosition` (44) + `Propose/AcceptAuthorityTransfer` (36/37) |
| 4.1 insurance seed / withdraw                     | done        | permissionless `SeedInsurance` (40) + staged, backing-gated `Propose/ApplyInsuranceWithdraw` (41/42) |
| 4.2 insurance segregation                         | done        | on-chain `total_user_balance` aggregate + fail-closed `VaultInvariantViolated` gate at every token outflow |
| 5.1 EMA / TWAP                                     | done        | funding's index side reads Pyth `ema_price` (spot fallback); solvency stays on raw spot |
| 5.2 unified mark price                            | done        | resolved as *two named prices by design*: `funding_mark` (banded mid) vs `solvency_price` (raw oracle) — risk-model "two prices" section |
| 6.1 partial liquidation                           | done        | minimal-slice close (closed-form `partial_close_qty`, Kani-verified); insolvent → full close |
| 6.2 keeper-reward floor                           | done        | `liquidation_reward_floor` tops the equity-capped penalty up from insurance (pool-capped) |
| 7.1 maker collateral check at quote time          | done        | standing ladder reservation (`MakerQuote.reserved_margin`, worst-price margined)      |
| 7.2 inventory / skew management                   | won't build | closed by design — static quote; re-quoting is the maker's off-chain job              |

---

## 1. Pre-trade safety (highest priority)

> **Status: DONE** (Market v8 / OrderSlab v3). A money-path `submit_order` now
> reserves the order's worst-case initial margin so a matched trade can always
> settle. Covered by `tests/pretrade_safety.rs` + the existing money-path suite.

### 1.1 Collateral reservation at order submission — DONE

`submit_order` now takes optional trailing `position` + `user_collateral`
accounts, REQUIRED on a money-path market (`maintenance_margin_bps > 0`). It
reserves an upper bound on the margin the fill could ever require — a buy clears
at ≤ its limit, a sell at ≤ the histogram window top — and locks it into the
ledger. Because the actual margin locked at `settle_fill` is always ≤ that
reservation, settlement only ever *releases*; it can never revert for lack of
collateral (which would wedge the round). An under-collateralized order is
rejected cleanly **at submit** (`InsufficientCollateral`). The reservation rides
on `Order.reserved_margin` and is released by `cancel_order` and `settle_fill`.

A **reduce-only** flag (`SubmitOrderData.reduce_only`, see §2.2) reserves the same
**full worst-case margin** as any normal order (DDR-3 Correction-2 item 3 —
reduce-only cannot be perfectly honored in a batch auction, since the fill is fixed
at fold but the position at settle can move, so it opens a *collateralized*
position instead of discounting the reservation); its sole remaining job is
forcing `Consumed` at settle so the order never rests across rounds. (Note: in a
wide, non-oracle-anchored tick window a sell's worst-case reservation can sit well
above its limit-price margin; production markets use a tight oracle-anchored
window, so the over-reservation is small and operator-tunable.)

### 1.2 Position cap + initial-margin buffer — DONE (max-OI cap deferred)

`initial_margin_bps` (Market v8, validated `≥ maintenance_margin_bps`) is the
initial-margin buffer locked at open/increase, so a position never opens exactly
on its liquidation line; its inverse is the market's implicit max leverage.
`max_position_notional` (Market v8, `0 = disabled`) caps a single position's
worst-case resulting notional, enforced at submit (`PositionLimitExceeded`).

The *max open-interest* cap is now DONE (`max_open_interest`, Market v12,
hot-updatable): a **soft** per-side cap checked at submit against the order's OI
*increment* (`qty` when same-side, `qty − |position|` on a flip), so it can be
raced past by in-flight orders but can never block de-risking — a pure reduce
always passes, even over the cap (`partial_liquidation.rs::oi_cap_blocks_increase_never_derisking`).

### 1.3 `initialize_market` parameter validation — DONE

Validation lives in `initialize_market/data.rs` (`TryFrom`). It rejects the
structural and fee params (`tick_size == 0`, `num_ticks ∉ (0, 256]`,
`orders_per_auction_cap ∉ (0, 90]` (the 10,240-byte single-`CreateAccount` ceiling
at `ORDER_LEN = 112` — `MAX_ORDERS_PER_AUCTION_CAP`), `|maker/taker_fee_bps| > 1000`,
`integrator_share_bps > 10_000`, `max_price_move_bps_per_slot > 10_000`,
`crank_fee > MAX_CRANK_FEE`, `num_slab_shards ∉ (0, 64]`) and the **risk**
config: a market is either a no-money-path clearing benchmark (every risk bps
zero) or a money market with `maintenance_margin_bps ∈ (0, 5000]`,
`initial_margin_bps ∈ [maintenance, 10000]`, and `liquidation_penalty_bps ≤ 5000`.
`crank_fee` is bounded (adversarial-review fix — it is an instant, authority-set
insurance outflow); `soft_stale_slots` stays unbounded (harmless). The oracle account is
not hard-checked against `oracle_feed_id` at init (by design — a market may be
provisioned before its Pyth feed is warm; the feed is verified on every later read).

---

## 2. Position management (user-facing)

### 2.1 Close / reduce-position — DONE (SDK composition + rent reclaim)

The exit *is* an opposing order — deliberately: the auction is the venue, and
there is no close-against-vault-at-oracle path (it would need a counterparty of
last resort). What shipped: the SDK's `close_position` composes an
opposite-side, **reduce-only market order** (window-boundary price + IOC), so
"flatten now" is one call that crosses any available liquidity at the uniform
clearing price; and `ClosePosition` (disc 44) closes a FLAT, drained, isolated
position account and refunds its rent (`close_lifecycle.rs`). A one-sided book
still means waiting for liquidity — that is the mechanism, not a gap.

### 2.2 Reduce-only flag — DONE (settle-consume scope; reserves FULL margin)

`submit_order/data.rs` carries a trailing `reduce_only` byte. Since DDR-3
Correction-2 item 3 it **no longer discounts the margin reservation**: a
reduce-only order reserves the same full worst-case initial margin as any normal
order (`submit_order/processor.rs` — the same-side headroom is still scanned for
anti-spam accounting but explicitly discarded, `let _ = already_same_side`).
Rationale: reduce-only cannot be perfectly honored in a batch auction — the fill
quantity is fixed at fold, but the position at settle can have moved (liquidation,
funding, other fills), so the order may open against intent; clamping the fill at
settle would break OI conservation (Correction #1). The only safe path is to let
it open a *collateralized* position. The flag's sole remaining job is forcing
`Consumed` at settle (never re-armed `Resting`), so a reduce-only order can never
carry across rounds into exposure the market gapped it into. Covered by
`tests/integration-tests/tests/pretrade_safety.rs::reduce_only_reserves_full_margin_no_discount`.

### 2.3 Order types — DONE (limit/GTC/GTT/IOC/market; FOK & post-only by design)

`submit_order` takes an `expires_at_auction` field (0 = good-till-cancelled,
else an absolute round id — Stage B): a resting order carries forward round
after round (see §2.4) until it fully fills, is cancelled, or expires, at which
point it is permissionlessly reapable (`cancel_order`, margin always returns to
the owner). **IOC** is the boundary case made legal by P4.1: `expires ==
arm_round` means exactly one auction — the remainder consumes at settle, never
rests (`tests/ioc.rs`). **Market orders** are SDK sugar (`submit_market_order`):
a buy at the window top / sell at the floor is marketable against all in-window
liquidity, and uniform-price clearing means the trader pays the cross, never
their limit. **FOK and post-only are deliberately not built** — FOK breaks the
telescoping-floor conservation under pro-rata rationing, and post-only already
exists structurally as the maker-quote book (`docs/system-design.md` §8).

### 2.4 Partial fills carry over across rounds — DONE (resting orders, Stage B)

> **Status: DONE.** `settle_fill` now re-arms an order that doesn't fully fill
> instead of discarding it. See `docs/design-decisions.md` DDR-2/DDR-3 for the
> design (the roll-gate change and the moving-tick-window interaction) and
> `docs/plan.md` §3 for the implementation. Covered by
> `tests/integration-tests/tests/resting_orders.rs`.

An order rationed at the marginal tick (or unfilled entirely) is no longer
consumed: if `fill == order.remaining` (fully filled) or the order has expired,
it's marked `Consumed` and leaves the book as before; otherwise it re-arms
`Resting` with its reduced `remaining` and a reset fold-time prefix, and the next
round's `process_chunk` folds it again automatically — no resubmission, and the
total filled quantity across rounds conserves exactly
(`resting_orders.rs::partial_fill_rests_then_completes_conserving`). A
reduce-only order is the one exception: it always force-`Consumed`s rather than
resting, so it can never drift a position's exposure across an intervening
`liquidate`/funding update (DDR-3 correction #1). Margin for the carried
leftover is held against a fixed `worst_price` snapshotted at submit, so it
stays stable even as the oracle-anchored tick window recenters between rounds
(`state/order.rs::Order.worst_price`); a resting order whose price the window
recenters past is folded at the boundary tick if it's now marketable, or left
parked (exempt from the completeness gate) if the market moved away from it
(`classify_resting_fold`, DDR-3).

### 2.5 Remove-from-group for cross margin — DONE

`RemovePositionFromMargin` (disc 28, `remove_position_from_margin/processor.rs`)
unbinds a flat, owner-matched, zero-collateral member and calls
`MarginAccount::remove_member` (`margin_account.rs:149-167`), which **compacts** the
member array and **decrements** `position_count` — so the set is neither append-only
nor monotonic, and a churned group is never permanently full. Covered by
`test_remove_member_compacts_and_frees_slot`. (See `known-issues.md` §2.4.)

### 2.6 Minimum order size / notional — DONE

`Market.min_order_notional` (v12, `0 = disabled`, hot-updatable via
`update_market_params`): `submit_order` rejects `quantity·price` below it with
`OrderBelowMinimum` (29) — a plain u128 comparison, no division. Partial
liquidation reuses it as the dust floor (a remainder below it full-closes).

### 2.7 Cancel-all / batch cancel / stale-order expiry — DONE

`CancelAllOrders` (disc 43): one transaction removes every still-`Resting`
order the signer owns in one shard, releases the summed reservation as ONE
credit, and emits per-order `OrderCancelled` events; zero matches is a no-op
success (multi-shard = a client loop; `tests/cancel_all.rs`). Owner-path only:
reaping strangers' expired orders stays on single `cancel_order`, keeping the
strict-`<` reap boundary in one place. Stale-order expiry itself is
`expires_at_auction` + the keeper's reap duty (§2.3).

---

## 3. Admin / lifecycle (the program is an engine, not yet operable)

### 3.1 Update-market / set-risk-params — DONE (hot + staged)

Two speeds by blast radius: `UpdateMarketParams` (disc 33) retunes the
operationally-hot, low-blast-radius params immediately (fees, crank fee, min
notional, OI cap, reward floor); the solvency-relevant risk params (margins,
penalty, brake, stale window, close buffer) go through the staged
`ProposeRiskUpdate`/`ApplyRiskUpdate` pair (34/35): authority proposes, ANYONE
may apply after `RISK_UPDATE_DELAY_SLOTS` (3,000 slots) — the delay is enforced
by consensus, not trust. Both share `initialize_market`'s validators so a
retune can never set values init would reject (`admin_lifecycle.rs`).

### 3.2 Pause / halt / resume — DONE

`SetPause` (disc 32) writes `Market.paused` bitflags: `PAUSE_INTAKE` blocks
`submit_order` + maker-quote writes, `PAUSE_ROLL` blocks `start_auction` (the
market winds down to a quiescent end-state). **Exits never pause**: cancel,
settle, withdraw, liquidate all keep working — a pause can trap no one
(`tests/pause.rs`). `MarketPaused` (2) is now a real, wired error.

### 3.3 Set-oracle / repoint feed — DONE (staged, paused-only)

`ProposeSetOracle`/`ApplySetOracle` (38/39): the authority stages a new oracle
account + feed id; the permissionless apply runs only after the delay AND while
the market is fully paused and quiescent — a feed repoint mid-round could move
the window under live orders (`admin_lifecycle.rs`).

### 3.4 Close-market / delist / authority transfer — DONE

`Propose/AcceptAuthorityTransfer` (36/37) is the two-step handoff (the staged
new authority must sign to accept — a typo'd key can never take a market).
`ClosePosition` (44) refunds a flat position's rent; `CloseMarket` (45) winds
down a fully quiescent market — paused, post-clearing, every shard reset and
empty, zero OI, zero active quotes, else `MarketNotQuiescent` (49) — closing
every shard, the histogram, the clearing result, and the market itself, rent to
the authority (`close_lifecycle.rs`). Draining OI is operational (pause intake,
let closes/funding/liquidation run); there is deliberately no force-close.

---

## 4. Treasury / insurance

### 4.1 Insurance seed / withdraw — DONE

`SeedInsurance` (disc 40) is a **permissionless donation**: anyone can move
tokens into the pool (this also fixed the real devnet bootstrap deadlock — a
zero-fee fresh market's first profitable settle failed `InsuranceInsolvent`
forever until the pool held one unit). The outflow is the program's ONLY
authority-controlled token exit and is double-gated:
`ProposeInsuranceWithdraw`/`ApplyInsuranceWithdraw` (41/42) — vault-authority
propose, delay, then a permissionless apply that re-clamps to the current pool
and runs the §4.2 fail-closed backing gate post-debit pre-transfer
(`treasury.rs`, `partial_liquidation.rs::insurance_withdraw_is_staged_delayed_and_backed`).

### 4.2 Insurance segregation — DONE (on-chain backing aggregate)

`Vault.total_user_balance` (v3) mirrors every user-balance mutation, so the
backing invariant `vault_token ≥ Σ balances + insurance` is now checkable — and
CHECKED — on-chain: every token outflow (withdraw, cross-withdraw, insurance
withdraw) passes the fail-closed `VaultInvariantViolated` (51) gate. Drift
stops money leaving; it never wedges rounds
(`treasury.rs::corrupted_backing_blocks_withdrawals_fail_closed`; live devnet
runs verify the sum exact to the unit).

---

## 5. Pricing / oracle

### 5.1 EMA for funding — DONE (P5.4)

`read_price` now parses Pyth's `ema_price` (`OraclePrice.ema_price_1e8`, spot
fallback when the feed carries none) and `update_funding` prices the **index
side** of the funding gap off it — the noise rail (one manipulated print barely
moves the EMA), while the mark band stays anchored on raw spot (the
manipulation rail). **Solvency deliberately stays on raw spot** — a lagging EMA
in a crash would recreate the §2.2 anti-liquidation bug. Mirrored in
`tempo-math::oracle` with the same goldens
(`funding.rs::funding_rate_prices_off_the_ema_not_spot`).

### 5.2 Mark price — DONE (two named prices by design)

Resolved as *naming honesty*, not unification: funding uses `funding_mark`
(banded clearing mid — smooth, manipulation-resistant) and
liquidation/withdraw use `solvency_price` (raw confidence-checked oracle —
never lagged, so a crash liquidates on time). The two serve different failure
modes and unifying them would reintroduce one of the §2.2 bugs; the risk-model
"two prices, two names" section documents the reasoning.

---

## 6. Liquidation depth

### 6.1 Partial liquidation — DONE

A mildly-underwater position loses only the minimal slice: `partial_close_qty`
(`margin.rs`, closed-form, integer-only, Kani-verified panic-free +
20k-iteration health/minimality fuzz) computes the smallest close restoring
equity ≥ maintenance·(1 + `liquidation_close_buffer_bps`); both `liquidate` and
`liquidate_cross` share it (the cross path feeds combined equity/maintenance).
Conservative fallbacks everywhere: insolvent, disabled (buffer 0),
penalty-eats-the-gain, dust remainder (< `min_order_notional`), or overflow →
the pre-existing full close. The remainder keeps FULL collateral (the realized
loss flushes to the ledger) and a `LiquidationNoProgress` (34) backstop makes
"still unhealthy after a partial" loud (`partial_liquidation.rs`).

### 6.2 Keeper-reward floor — DONE

`liquidation_reward_floor` (v12, hot-updatable): when the equity-capped penalty
comes in below the floor, insurance tops the liquidator up to it — capped at
the pool (conserving, fail-soft), the `finalize_clear` crank-fee shape.
Griefing-safe by construction: a liquidation only executes when equity <
maintenance, a condition an attacker cannot manufacture for free
(`partial_liquidation.rs::reward_floor_tops_up_a_tiny_penalty`).

---

## 7. Maker-book hardening

### 7.1 Maker collateral check at quote time — DONE (standing ladder reservation)

`update_maker_quote_levels` (and the maker settle/clear path) now carries the
maker's collateral ledger: posting a ladder locks its worst-case initial margin
(`MakerQuote.reserved_margin`, margined against a fixed `worst_price` snapshot),
a re-quote re-reserves the delta, `settle_maker_quote` swaps reservation for
position margin, and `clear_maker_quote` releases the standing lock. An
unbacked ladder is rejected before it can fold into the histogram, closing both
the price-manipulation and insurance-drain vectors (`maker_margin.rs`, 6 tests).

### 7.2 No inventory / skew management — CLOSED: won't build (by design)

Closed as a deliberate design decision, not open debt. The quote is static between
explicit `update_*` calls; re-quoting is the maker's off-chain job (the reference
`crates/mm-bot` `strategy::build_quote` already does oracle-anchored,
inventory-skewed ladders). On-chain auto-skew would add state and CU to the fold
path for something the off-chain loop does better. (The old `sync_spread_ticks`
placeholder hook was removed in MakerQuote v2 — see `known-issues.md` §3.)

---

## 8. Suggested build order — COMPLETE

Every numbered area above is now **done** (or explicitly closed as won't-build:
§7.2 inventory management, FOK/post-only order types). The build ran as planned
across `plan.md` phases 0–5: pre-trade safety (§1) → maker hardening +
admin/treasury (§3, §4, §7.1) → risk depth (§6, §1.2) → trading UX (§2) →
pricing polish (§5). What remains for this doc's successor is tracked in
`known-issues.md` (residual defects/limitations) and `plan.md` §9 (the
benchmark-gated C2 decision).

> Deliberately deferred (a design decision, not a loose end): multi-mint
> collateral support. Revisit only with an explicit decision on per-mint ledgers.
> **For now the system supports a single collateral mint — USDC.** Every market
> (SOL-perp, BTC-perp, …) settles in that one mint; the number of *markets* is
> unbounded, but they all share the one USDC collateral ledger/vault. See
> `known-issues.md` §2.3.
