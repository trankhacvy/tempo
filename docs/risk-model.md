# Tempo — Risk Model

This document describes the **money and risk layer** that sits on top of the clearing
engine: how collateral, PnL, funding, liquidation, and solvency are kept safe. The
matching engine is described in `tempo-clearing-protocol.md`.

Two rules hold everywhere:

- **No floating point.** All math is integer (`u64`/`u128`/`i128`), checked or
  saturating, and rounds **against the user** (margins round up, payouts round down).
  The pure math lives in `clearing.rs`, `margin.rs`, `funding.rs`, `mark.rs`,
  `wide_math.rs`, and `cross_margin.rs`, and is unit- and fuzz-tested.
- **Conservation.** The vault's token balance always satisfies
  `vault_token ≥ Σ user balances + insurance`. Every money path moves value *between*
  ledgers and the insurance fund — it never mints it.

---

## 1. Accounts

- **Vault** (per collateral mint) — holds the SPL tokens plus the insurance balance. A
  single vault-authority PDA owns the token account and signs withdrawals.
- **UserCollateral** (per owner) — the money ledger: `balance` (total) and `locked`
  (margin reserved); `free = balance − locked` is withdrawable.
- **Position** (per owner, per market) — signed `size`, volume-weighted `entry_price`,
  `collateral` (isolated margin reserved for it), `realized_pnl`, funding and
  socialized-loss checkpoints, and a margin mode (isolated or cross).

> Unit assumption: `collateral`, `realized_pnl`, and `|size|·price` share one base unit;
> the operator chooses the tick size and contract size so notional is denominated in the
> collateral mint's units. Reconciling this rigorously against the oracle's fixed-point
> scale is a planned refinement.

---

## 2. Open interest

Each market tracks total long and short open interest, updated on every fill and
liquidation. Open interest is the denominator used to socialize loss (§5) and is the
invariant `long == short` after a balanced round.

## 3. Funding

`update_funding` (permissionless) reads the oracle, derives a band-clamped **mark price**
(the round's clearing prices anchored to the oracle), and accrues `(mark − oracle)/oracle`
scaled by elapsed time into a monotonic funding index, clamped to a per-period cap. Each
position settles funding lazily against the index delta, paid out of realized PnL.

> Batch-perp funding stability is not yet proven — the per-period cap is the safety rail
> today; this remains an area to simulate further before mainnet.

## 4. Mark price and the per-slot price brake

The risk mark is a stored *effective price* that walks toward the fresh oracle by at most
a configured amount per slot. A gap-up therefore **cannot liquidate the whole book in one
slot** — a spike is recognized gradually. Liquidation prices off this effective price, not
the raw oracle.

## 5. Liquidation, bad debt, and auto-deleveraging

`liquidate` (permissionless) closes a position whose equity has fallen below its
maintenance margin:

- The owner keeps any residual, the liquidator earns a penalty, and the close is
  conserved through the insurance fund.
- **Hard solvency gate.** A winner's gain is funded from insurance; if insurance is short,
  the settle **fails closed** rather than minting money — *delay, not loss*. It can be
  retried after losses are collected or insurance is topped up.
- **Socialized loss (ADL).** Bad debt beyond insurance is spread across the *winning* side
  by open interest (a liquidated long's shortfall is charged to shorts, and vice-versa)
  via a per-side socialized-loss index; each position pays its share lazily. If there is
  no winning side to absorb it, the unbacked remainder is logged, never silently dropped.
- **Grief-proof.** A redundant or late liquidation reverts cleanly with no state change
  (Solana rolls back the transaction); liquidation never requires the owner's signature.

## 6. Oracle resilience

The program parses the Pyth price account directly and checks owner, feed id, staleness,
and confidence. There are three states:

- **fresh** → advance and use the effective price;
- **soft-stale** (within a configured window of the last good update) → risk-reducing
  liquidation still proceeds off the *frozen* effective price;
- **hard-stale** → reject; wind-down only.

Clearing and settlement are **oracle-independent** (they use tick prices, not the oracle),
so a dead oracle only pauses funding and liquidation — it never traps positions or funds.
No dedicated stale-wind-down instruction is needed (a permissionless one would itself be a
griefing surface); an admin escape hatch covers a wedged round.

## 7. Cross-margin

An owner can group several positions to share one collateral ledger. The group is judged
by **one combined equity vs one combined maintenance requirement**: profit on one leg
offsets loss on another. Grouped positions hold no isolated lock — their backing is the
combined-health check. Cross-margin withdrawal and liquidation require *every* member to
be supplied (omitting a losing leg fails closed), and each member market must have a fresh
effective price.

## 8. Overflow safety

A 256-bit-intermediate `mul_div` carries every `quantity · price · bps` product, so
notional math cannot overflow. Margin requirements round up and fees round down through it.

---

## Verification

- **Property fuzzes** (tens of thousands of iterations each): whole-book open-interest
  conservation, the histogram cross vs. a brute-force reference, liquidation conservation,
  funding sign and cap, mark-price band and step cap, and the wide-math path vs. a 256-bit
  reference.
- **End-to-end (LiteSVM) tests**: the solvency fail-closed gate, open-interest balance,
  bad-debt socialization, the price brake, soft-stale liquidation, backed-profit
  withdrawal, cross-margin health and liquidation, and account-layout migration.
- **Formal proofs** (`cargo kani`): panic/overflow freedom on the core arithmetic
  (`find_cross`, the marginal-fill allocation, the price-step clamp, and `mul_div`). A
  transaction-level fuzz target asserts the four load-bearing invariants across random
  instruction sequences.

## Honest open items

- Batch-perp funding-rate stability is unproven (§3).
- PnL is *conserving* (floated through insurance), not yet true continuous
  mark-to-market netting between longs and shorts.
- The single-unit assumption (§1) is an operator constraint, not yet enforced against the
  oracle's fixed-point scale.
- Collateral is single-mint by design; supporting multiple collateral mints is a separate
  redesign. **For now the only supported collateral is USDC** — all markets share one USDC
  vault/ledger (many markets, one collateral mint). See `known-issues.md` §2.3.
