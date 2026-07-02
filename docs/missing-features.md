# Tempo — Missing Features

This document lists functionality a production perps DEX needs that is **not yet
built**. It is separate from `known-issues.md` (defects in code that already
exists).

Context: the **matching and clearing engine is complete** (sharded order book,
price histogram, three-phase clearing, dual auction, resting orders,
always-open submission — `docs/plan.md`) and the **maker-quote book is real
and end-to-end**. What is missing is the trading/risk/admin layer that turns
a clearing engine into an operable exchange. Items are grouped by area; each notes
where the gap lives in code.

Status tags: ✅ **DONE** (built) · 🟡 **partial** (exists but incomplete) ·
⬜ **absent** (nothing exists yet).

### Status at a glance

| Item                                              | Status        | Note                                                                                  |
| ------------------------------------------------- | ------------- | ------------------------------------------------------------------------------------- |
| 1.1 collateral reservation at submit              | ✅ done        | `Order.reserved_margin`; rejected at submit (`InsufficientCollateral`)                |
| 1.2 position cap + initial-margin buffer          | 🟡 partial     | `initial_margin_bps` + `max_position_notional` done; **max-OI cap still absent**       |
| 1.3 `initialize_market` param validation          | ✅ done        | structural + fee + risk-config bounds in `data.rs` `TryFrom`                          |
| 2.1 close / reduce-position instruction           | ⬜ absent      | only exit is an opposing order into the next auction                                  |
| 2.2 reduce-only flag                              | ✅ done        | margin-reservation scope only (does not enforce non-flip at settle)                   |
| 2.3 order types beyond resting limit              | 🟡 partial     | GTC/GTT expiry (`expires_at_auction`) shipped with resting orders; still no market / IOC / FOK / post-only |
| 2.4 partial-fill carry-over                       | ✅ done        | resting orders (Stage B): unfilled/partial remainder re-arms `Resting` and carries to the next round        |
| 2.5 remove-from-group for cross margin            | ✅ done        | `RemovePositionFromMargin` (disc 28) + compacting `remove_member`                     |
| 2.6 minimum order size / notional                 | ⬜ absent      | only `quantity != 0`; dust flooding possible                                          |
| 2.7 cancel-all / batch cancel / expiry            | ⬜ absent      | only single-order `cancel_order`                                                      |
| 3.1 update-market / set-risk-params               | ⬜ absent      | params fixed at init (only `migrate_market` rewrites brake/stale-window)              |
| 3.2 pause / halt / resume                         | ⬜ absent      | `MarketPaused` error exists but is referenced nowhere                                 |
| 3.3 set-oracle / repoint feed                     | ⬜ absent      | oracle bound once at init                                                             |
| 3.4 close-market / delist / authority transfer    | ⬜ absent      | markets can't be wound down; no authority transfer                                    |
| 4.1 insurance seed / withdraw                     | ⬜ absent      | no admin seed/harvest; only outflow is `finalize_clear` crank fee                     |
| 4.2 insurance segregation                         | 🟡 partial     | bookkeeping `u64` shares the vault token account; invariant tested off-chain only     |
| 5.1 EMA / TWAP                                     | ⬜ absent      | spot price only; Pyth `ema_price` ignored                                             |
| 5.2 unified mark price                            | 🟡 partial     | funding = clearing midpoint, liquidation = raw oracle — two definitions               |
| 6.1 partial liquidation                           | ⬜ absent      | `liquidate` zeroes the whole position                                                 |
| 6.2 keeper-reward floor                           | 🟡 partial     | penalty caps to equity; most cranks unincentivized                                    |
| 7.1 maker collateral check at quote time          | ⬜ absent      | unbacked ladders can move the clearing price + drain insurance                        |
| 7.2 inventory / skew management                   | ⬜ absent      | by design — static quote; re-quoting is the maker's off-chain job                     |

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

A **reduce-only** flag (`SubmitOrderData.reduce_only`, see §2.2) reserves only the
portion that would open new exposure, so closing a fully-margined position is
never blocked. (Note: in a wide, non-oracle-anchored tick window a sell's
worst-case reservation can sit well above its limit-price margin; production
markets use a tight oracle-anchored window, so the over-reservation is small and
operator-tunable.)

### 1.2 Position cap + initial-margin buffer — DONE (max-OI cap deferred)

`initial_margin_bps` (Market v8, validated `≥ maintenance_margin_bps`) is the
initial-margin buffer locked at open/increase, so a position never opens exactly
on its liquidation line; its inverse is the market's implicit max leverage.
`max_position_notional` (Market v8, `0 = disabled`) caps a single position's
worst-case resulting notional, enforced at submit (`PositionLimitExceeded`).

Still **absent**: a *max open-interest* cap. OI (`oi_long`/`oi_short`) is tracked
but not bounded. Unlike per-position checks it is a global aggregate, so an
airtight cap needs OI-headroom reservation parallel to the margin one (or a soft
check) — a clean follow-up, not a wedge risk.

### 1.3 `initialize_market` parameter validation — DONE

Validation lives in `initialize_market/data.rs` (`TryFrom`). It rejects the
structural and fee params (`tick_size == 0`, `num_ticks ∉ (0, 256]`,
`orders_per_auction_cap ∉ (0, 128]`, `|maker/taker_fee_bps| > 1000`,
`integrator_share_bps > 10_000`, `max_price_move_bps_per_slot > 10_000`) and now
the **risk** config: a market is either a no-money-path clearing benchmark (every
risk bps zero) or a money market with `maintenance_margin_bps ∈ (0, 5000]`,
`initial_margin_bps ∈ [maintenance, 10000]`, and `liquidation_penalty_bps ≤ 5000`.
`crank_fee` / `soft_stale_slots` stay unbounded (harmless). The oracle account is
not hard-checked against `oracle_feed_id` at init (by design — a market may be
provisioned before its Pyth feed is warm; the feed is verified on every later read).

---

## 2. Position management (user-facing)

### 2.1 No close / reduce-position instruction — absent

The only way to exit is to submit an opposing order into the next auction and
wait for a cross. If the book is one-sided you are stuck. There is no
direct user-initiated close/flatten at oracle mark.

### 2.2 Reduce-only flag — DONE (margin-reservation scope)

`submit_order/data.rs` now carries a trailing `reduce_only` byte. It governs the
**margin reservation** (§1.1): a reduce-only order against an opposite position
reserves only the portion that would open new exposure (computed against the
position size minus the trader's already-resting same-side quantity), so a close
is never blocked by the worst-case reservation. Note it does NOT yet *enforce*
non-flipping at settle (the auction's matched volume must always settle to
conserve OI); the bind-time accounting guarantees a reduce-only set can only
reduce, never flip without reserving for it.

### 2.3 No order types beyond a resting limit — partial

`submit_order` now takes an `expires_at_auction` field (0 = good-till-cancelled,
else an absolute round id — Stage B, `docs/plan.md` §3.1/§3.2): a resting order
carries forward round after round (see §2.4) until it fully fills, is cancelled,
or expires, at which point it leaves the book and is permissionlessly reapable
(`cancel_order`, margin always returns to the owner). This closes the "no
time-in-force" half of this gap. Still **absent**: order *types* — no market /
IOC / FOK / post-only. Every order is still a single-shot-per-round limit price;
what changed is that an unfilled one no longer has to be resubmitted by hand.

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

### 2.6 No minimum order size / notional — absent

The only quantity check is `quantity != 0` (`submit_order/data.rs`). Dust-order
flooding is possible.

### 2.7 No cancel-all / batch cancel / stale-order expiry — absent

Only single-order `cancel_order` exists.

---

## 3. Admin / lifecycle (the program is an engine, not yet operable)

### 3.1 No update-market / set-risk-params — absent

Margins, fees, the price brake, and the stale window are set once at
`initialize_market` and never changeable by a live-market instruction.
`market.authority` is checked only in `force_reset` and `migrate_market` (the latter
re-sets the brake + stale-window during the one-time v4→v5 in-place upgrade,
`migrate_market/processor.rs:97-98` — a migration path, not a retune). No admin can
retune a live market.

### 3.2 No pause / halt / resume — absent

`TempoProgramError::MarketPaused` exists but is referenced nowhere; there is no
paused flag and no instruction checks one. Dead error marking an unbuilt
circuit-breaker.

### 3.3 No set-oracle / repoint feed — absent

The oracle is bound once at init. If a Pyth feed is deprecated, the market cannot
be moved.

### 3.4 No close-market / delist, no authority transfer — absent

Markets cannot be wound down or rent reclaimed; market `authority` has no
transfer instruction.

---

## 4. Treasury / insurance

### 4.1 Insurance fund cannot be seeded or withdrawn — absent

`set_insurance_balance` is called only inside settle/liquidate/finalize
conservation. No admin can bootstrap the pool or harvest accrued fees. The only
outflow is the flat per-clear `crank_fee` paid to the cranker
(`finalize_clear/processor.rs:216-217`); there is no admin seed/harvest path and no
withdrawal fee, so **protocol fees beyond that crank reward are economically
trapped** in `insurance_balance`.

### 4.2 Insurance is not segregated — partial

`Vault.insurance_balance` is a bookkeeping `u64` sharing the one vault token
account with user balances; the backing invariant
(`vault_token ≥ Σ balances + insurance`) is enforced only by host tests, not
on-chain.

---

## 5. Pricing / oracle

### 5.1 Spot price only — no EMA/TWAP — absent

`oracle.rs` parses spot price/conf/exponent/publish_time and ignores Pyth's
`ema_price`. Funding and liquidation mark off the single latest print (modulated
only by the per-slot brake).

### 5.2 Inconsistent mark price — partial

Funding marks off the last-clearing-price midpoint (`update_funding/processor.rs:72`,
banded around the oracle); liquidation marks off the **raw** confidence-checked oracle
via `oracle::solvency_mark` (`liquidate/processor.rs:76-95`). Two different definitions
of "mark" for the two core risk functions. (Liquidation used to price off the braked
`effective_price`; that was fixed in `known-issues.md` §2.2 — the braked value is now
only the soft-stale fallback.)

---

## 6. Liquidation depth

### 6.1 No partial liquidation — absent

`liquidate/processor.rs:212` zeroes the whole position. A 1%-underwater position
is fully closed. Real engines liquidate the minimum to restore margin.

### 6.2 No keeper-reward floor — partial

The liquidation penalty caps to tiny equity (`margin.rs:121`), so liquidating a
near-zero-equity position can net the liquidator ~0 while still costing gas.
Cranks for `process_chunk`/`settle_fill`/`update_funding`/`start_auction` are
otherwise unincentivized; only `finalize_clear` pays a flat `crank_fee`.

---

## 7. Maker-book hardening

### 7.1 No maker collateral check at quote time — absent

`init_maker_quote` / `update_maker_quote_levels` take no collateral account and
write arbitrary `size`. A maker can post a huge ladder with zero collateral; it
folds into the histogram and moves the clearing price for everyone, then an
under-margined fill produces a shortfall absorbed by insurance at settle. This is
both a price-manipulation and an insurance-drain vector.

### 7.2 No inventory / skew management — absent (by design)

The quote is static between explicit `update_*` calls; re-quoting is the maker's
off-chain job. (The old `sync_spread_ticks` placeholder hook was removed in
MakerQuote v2 — see `known-issues.md` §3.)

---

## 8. Suggested build order

1. ~~**Pre-trade safety** (§1)~~ — **DONE**: collateral reservation at submit,
   initial-margin buffer + per-position notional cap, full `initialize_market`
   validation, reduce-only (Market v8 / OrderSlab v3). Remaining sub-item: a
   max-open-interest cap (§1.2).
2. **Position management** (§2.1, §2.6, §2.7) — explicit close/reduce
   instruction + minimum order size + batch cancel (reduce-only §2.2,
   cross-margin remove-from-group §2.5, and partial-fill carry-over §2.4 are
   done; §2.3 order-type TIF/expiry is done, order *types* beyond a resting
   limit remain absent).
3. **Admin lifecycle** (§3) — update-params, pause, set-oracle.
4. **Treasury** (§4) — insurance seed/withdraw + protocol-fee withdrawal.
5. **Depth & pricing** (§5, §6) — partial liquidation, unified mark, EMA/TWAP.
6. **Maker hardening** (§7.1) — quote-time margin.

> Deliberately deferred (a design decision, not a loose end): multi-mint
> collateral support. Revisit only with an explicit decision on per-mint ledgers.
> **For now the system supports a single collateral mint — USDC.** Every market
> (SOL-perp, BTC-perp, …) settles in that one mint; the number of *markets* is
> unbounded, but they all share the one USDC collateral ledger/vault. See
> `known-issues.md` §2.3.
