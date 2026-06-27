# Tempo — Known Issues

This document lists defects in **code that already exists** — things that are
broken, dead/redundant, or wrongly designed. It is deliberately separate from
`missing-features.md` (which lists functionality not yet built).

Every item cites a real `file:line`. Items are classified:

- **[bug]** — produces a wrong result or breaks an invariant.
- **[design]** — works today but the design will bite as the program scales or
  is used adversarially.
- **[dead]** — written but never read, or duplicated; safe to delete or wire up.

The clearing arithmetic itself (`find_cross` / `compute_fill` /
`compute_marginal_fill`) is correct and well-tested. The defects below cluster in
the **money/settlement glue** and the **lifecycle bookkeeping** around it.

> **Status legend:** ✅ **fixed** · 🟡 **partially fixed** · ⛔ **deferred**
> (blocked on a product decision and/or a devnet re-provision) · ⬜ **not started**.
> Each item below carries a `Status:` line. See `docs/plan.md` for the full
> implementation log.

### Status at a glance

| Item                                              | Status         | Note                                                                                                     |
| ------------------------------------------------- | -------------- | -------------------------------------------------------------------------------------------------------- |
| 1.1 maker mints money                             | ✅ fixed       | shared `settle_money::conserve_and_socialize`                                                            |
| 1.2 settle bad debt not absorbed                  | ✅ fixed       | now socializes, symmetric with `liquidate`                                                               |
| 1.3 `is_maker` steering                           | ✅ fixed       | Option A: `submit_order` is taker-only; makers only via the MakerQuote book                              |
| 1.4 cross-margin ignores unsettled funding/social | ✅ fixed       | pure per-leg dock                                                                                        |
| 1.5 ADL under-charges a flip                      | ✅ fixed       | re-snapshot inside `apply_fill`                                                                          |
| 1.6 maker marginal-tick over-allocation           | ✅ fixed       | fold-time `cum_before` snapshots; makers tile the marginal tick → Σ fills == V (MakerQuote v3, re-provision) |
| 2.1 hand-maintained counters                      | ✅ fixed       | slab-derived completeness gate                                                                           |
| 2.2 brake delays liquidations                     | ✅ fixed       | all paths price solvency off the raw per-leg oracle (`oracle::solvency_mark`); cross members now carry an oracle |
| 2.3 mint-agnostic collateral                      | ⛔ deferred    | USDC-only for now (1 mint, many markets); breaking layout + re-provision + decision                      |
| 2.4 cross-margin reconstruct-per-call             | ✅ fixed       | flat legs ride as a bare position (no market/oracle); live-leg ceiling is ALT territory; "maintain equity as state" dropped as infeasible |
| 2.5 `margin_mode` mutable mid-auction             | ✅ fixed       | in-flight-order guard                                                                                    |
| 2.6 migrate OI ordering                           | ✅ fixed       | empty-slab quiescence gate                                                                               |
| 2.7 tick window / O(n²) settle / read_oracle feed | ✅ fixed       | oracle-anchored window (re-snaps each round); O(1) slab access (bump cursor + slot hint + fold-time `cum_before`); feed already fixed. Re-provision required |
| 2.8 `withdraw_cross` masks ledger drift           | ✅ fixed       | `checked_sub` → hard `CollateralLedgerDrift`, no longer absorbed                                         |
| 2.9 low-severity batch (code-review)              | ✅ fixed       | 4 of 5 done (future-ts hard-reject, shared `leg_contribution`, error class, dead variant reserved); (d) same-market re-parse intentionally skipped (unmeasured micro-opt, bounded by 8) |
| 2.10 reduce-only settle race (code-review)        | ⬜ unverified  | likely a false positive — `trader_resting_stats` appears to bound it; needs a confirming conservation fuzz                |
| 2.11 cancel/zero-fill settle need the ledger      | ⬜ by-design   | releasing the §1.1 reservation requires `user_collateral`; non-wedge, client + doc-note follow-up                        |
| 2.12 stale client bundle after Market v8 / slab v3 | ⬜ operational | regen clients (`pnpm generate-clients && pnpm bundle-client`) + re-provision before any devnet money-path run            |
| §3 dead code                                      | ✅ fixed       | dead fields + `close_maker_quote` done; `compute_fill` removed + `classify_level` collapsed into one shared `clearing::fill_against_cross` |

> Note (code-review re-confirmations): the `/code-review` workflow also re-surfaced
> two items already tracked here — the isolated-`liquidate` raw-oracle vs cross
> braked-price asymmetry (was §2.2's "still open" half, **now fixed**: all three
> paths share `oracle::solvency_mark`) and `liquidate`'s bespoke insurance arithmetic
> overlapping `conserve_and_socialize` (the deliberate split documented in §4). Both
> are known, not new.

> Note on one false alarm: an earlier review flagged `liquidate/processor.rs`
> as double-debiting the owner's collateral. That is **not** a bug.
> `returned_to_owner = equity − penalty` is an absolute residual that already
> contains the collateral, so `balance − collateral + returned_to_owner` is the
> correct "seize all, refund the residual." `fuzz_liquidation_outcome_conserves`
> (`margin.rs`) proves `returned + penalty == equity`. Do not "fix" it.

---

## 1. Confirmed bugs (verified by reading the code)

### 1.1 Maker settle can mint money; taker settle cannot — [bug]

> **Status: ✅ FIXED.** Both settle paths now route through one shared primitive
> `settle_money::conserve_and_socialize`, which fails closed (`InsuranceInsolvent`)
> on an underfunded gain. The maker path can no longer mint money. The four
> copy-pasted money paths (the root cause) are collapsed for the two settle
> instructions; `liquidate`/`liquidate_cross` were left on their own (correct)
> conserving code — see the note in §4.

`settle_maker_quote/processor.rs:388-396` vs `settle_fill/processor.rs:416-418`.

When a **winner's** payout exceeds the insurance pool:

- `settle_fill` returns `InsuranceInsolvent` (fail-closed — never mints money).
- `settle_maker_quote` only `log!`s and `saturating_sub`s insurance to 0 — but
  the maker's balance was already credited via `apply_pnl`.

The maker path therefore breaks the core invariant
`vault_token ≥ Σ balances + insurance`. Root cause is code drift between two
copy-pasted money paths.

**Fix:** make the maker path fail closed identically, or extract a single shared
settle-money helper used by both processors.

### 1.2 `settle_fill` bad debt is logged, never absorbed — [bug]

> **Status: ✅ FIXED.** Both settle paths now socialize uncovered bad debt through
> `settle_money::conserve_and_socialize` (which calls `market.socialize_bad_debt`),
> symmetric with `liquidate`. The ADL residual uses the pre-accrual insurance
> exactly as `liquidate` does, so conservation matches.

`settle_fill/processor.rs:427-429`.

When a loser's realized loss exceeds their balance, the covered part accrues to
insurance but the uncovered `shortfall` is only `log!`ed. Unlike `liquidate`
(which calls `market.socialize_bad_debt`), settle has **no ADL fallback**. The
winning counterparty then either reverts (`InsuranceInsolvent`) or, if an
insurance cushion exists, draws real money the losing side never paid in.

Partially documented as a "v1.1 conserving" limitation, but the **asymmetry with
`liquidate`** is the real defect.

**Fix:** route settle bad debt through the same `socialize_bad_debt` path
`liquidate` uses.

### 1.3 `is_maker` was an unvalidated client flag that steered price formation — [bug] · [mechanism-design]

> **Status: ✅ FIXED (Option A).** `submit_order` is now **taker-only**: the
> `is_maker` byte was removed from its wire format (`data.rs` `LEN` 1+8+8=17), so a
> trader can no longer self-select which uniform cross they clear in or which fee
> tier they pay. `process_chunk` routes every slab order into a taker region by
> side alone (taker-sell→`BidSupply`, taker-buy→`AskDemand`); `settle_fill` always
> charges `taker_fee_bps` and settles taker-sell in the bid auction / taker-buy in
> the ask auction. **Maker liquidity comes exclusively from the on-chain
> `MakerQuote` book** (`init_maker_quote`→`process_maker_quote`→`settle_maker_quote`),
> where "maker" is a verifiable fact (you posted a standing, foldable quote) rather
> than a self-asserted flag — the price-steering vector is removed at the root.
>
> `Order.is_maker` is retained in the slab layout as a documented **always-0**
> field (no `OrderSlab` `VERSION` bump) for parity with `FillSettled.is_maker`,
> which the maker-quote settle path still sets to 1 so indexers can distinguish
> maker fills. **Breaking IDL change** → clients regenerated (`pnpm
generate-clients && pnpm bundle-client`, both bundles), the web trade panel's
> maker/taker toggle removed (retail orders are takers), the TS sim/bots maker
> flows moved to the quote book, and the integration suite rewritten to source
> maker liquidity from the `MakerQuote` book (a `post_maker_order` harness helper +
> `settle_maker_quote[_clearing]`). Requires the standing devnet re-provision.
>
> What follows is the original analysis (kept for the record).

**Symptom.** `submit_order/data.rs:34` rejects only `is_maker > 1`. That single
client-supplied byte then drives two things:

- `process_chunk/processor.rs:128-133` routes the order into a different
  histogram region by `(side, is_maker)` — i.e. it decides **which of the two
  uniform crosses (bid auction vs ask auction) the order clears in, and at which
  price**.
- `settle_fill/processor.rs:317` selects the fee tier (`maker_fee_bps` vs
  `taker_fee_bps`) from the same byte.

**Root cause.** There is no economic or account-based definition of "maker"
anywhere in the program. A trader self-declares the role, so they self-select
which cross they participate in and which fee tier they pay. The dual-auction
segregation — _the core DFBA mechanism_ — runs on the honor system. The attack is
not theoretical: a taker who wants to influence the maker-side clearing price (or
who wants the maker fee tier) simply sets the byte.

**Why a cheap fix is wrong.**

- You cannot "just validate the flag" — there is nothing on-chain to validate it
  against.
- You cannot derive maker/taker from "is the order marketable?" the way a CLOB
  does: in a batch auction there is **no resting book at submit time**. Orders are
  collected blind and cleared together, so "crossed the spread" is undefined. The
  maker/taker distinction in a batch auction must mean something _other_ than
  marketability.

**Option space (pick one — this is the product decision):**

| Option                                       | Definition of "maker"                                                                                                                                                                                             | Cost                                                                                   | Trade-off                                                                                                                                                                                                                                                                                                                   |
| -------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **A. Resting-quote = maker** _(recommended)_ | Only liquidity posted through the persistent `MakerQuote` book (`init_maker_quote`) folds into the maker regions (`BidDemand`/`AskSupply`). `submit_order` drops `is_maker` entirely and always produces a taker. | Medium — removes `is_maker` from the `submit_order` wire format (breaking IDL change). | Cleanest economically: "maker" becomes an on-chain fact (you committed a standing, foldable quote) instead of a self-asserted byte. The maker-quote book already exists end-to-end (`MakerQuote`, `process_maker_quote`, `settle_maker_quote`), so this is mostly a _removal_. Kills the price-steering vector at the root. |
| **B. Commit-to-stay = maker**                | A maker order must rest for ≥N auctions and/or post a bond; reneging is slashable.                                                                                                                                | High — new lifecycle + bond accounting.                                                | Models a real maker obligation, but heavy and net-new machinery.                                                                                                                                                                                                                                                            |
| **C. Fee-tier only, single price**           | Keep one flag for the _fee tier_ but collapse both auctions to one clearing price so the flag can no longer steer price.                                                                                          | Low.                                                                                   | Defeats the dual-flow thesis (the two-cross structure _is_ DFBA). **Reject.**                                                                                                                                                                                                                                               |

**Recommendation: Option A.** Maker liquidity should come _only_ from the
maker-quote book, which is already a first-class, tested path. `submit_order`
becomes taker-only. This makes "maker" mean "I posted a standing quote that folds
into the histogram" — verifiable on-chain — and removes the steering vector by
deleting the self-declared byte rather than adding new validation.

**Blast radius when implemented (Option A):**

- `submit_order/data.rs` — drop the `is_maker` field; shorten `LEN` (1+8+8); update
  the layout doc + tests. **Breaking IDL change** → regenerate clients
  (`pnpm generate-clients && pnpm bundle-client`) and the TS bots.
- `process_chunk/processor.rs:128-133` — `submit_order` orders route only to taker
  regions (`BidSupply` for sells, `AskDemand` for buys); maker regions are fed
  exclusively by `process_maker_quote`.
- `settle_fill/processor.rs:317` — a settled `submit_order` fill always pays
  `taker_fee_bps`; the maker tier applies only on the `settle_maker_quote` path.
- `Order` state — `is_maker` becomes always-0 for slab orders; confirm no other
  reader depends on it (e.g. `cumulative_qty_before` bucket keys).
- Tests — the dual-auction tests that submit "maker" orders via `submit_order`
  must be rewritten to post maker liquidity through the quote book.

**Interim mitigation (only if shipping before the decision):** at minimum, charge
the **taker** fee tier regardless of the flag so the cheap path cannot be gamed
for a rebate, and document that maker-side price influence via `submit_order` is a
known, unmitigated vector until Option A lands. This does **not** close the
price-steering hole (routing still honors the byte) — it only removes the fee
incentive. Prefer doing Option A properly.

### 1.4 Cross-margin health ignores unsettled funding/social-loss on read-only legs — [bug]

> **Status: ✅ FIXED.** A pure `pending_social_loss` (mirroring `settle_social_loss`)
> plus the existing pure `funding_payment` now dock unsettled funding + social loss
> on **every** member leg in both `withdraw_cross` and `liquidate_cross` (read-only,
> no writes — the target leg still gets the real settle before its close).

`withdraw_cross/processor.rs:77` reads `position.realized_pnl()` raw and never
calls `settle_funding`/`settle_social_loss` on any leg. `liquidate_cross` settles
only the _target_ leg, not the other legs it sums into the health gate.

A user with accrued funding debt can withdraw against equity that has not been
docked. Current tests miss this because they crank `update_funding` (advances the
_market_ index) but never carry an unsettled _position_ through a cross op.

**Fix:** settle funding + social-loss on **every** member leg before computing
combined equity in both `withdraw_cross` and `liquidate_cross`.

### 1.5 Socialized-loss (ADL) under-charges a flipped position — [bug]

> **Status: ✅ FIXED.** `Position::apply_fill` now takes the per-side social
> indices and re-snapshots the checkpoint whenever the fill **opens from flat OR
> flips sign** — making "the checkpoint always matches the current side" an
> invariant of the type. The settle processors no longer hand-snapshot.

`position.rs:217-242` tracks a single `last_social_index` for the position's
_current_ side; `settle_fill/processor.rs:284-285` re-snapshots only when
`oi_old == 0`. On a flip (long→short in one fill, `oi_old ≠ 0`) the snapshot is
skipped, leaving the checkpoint on the old side's index while the position now
sits on the other side — the next charge compares mismatched per-side indices.
The code comment at `position.rs:213` already admits "a flip can under-charge."

Funding itself is fine (it is settled _before_ the flip); only the side-indexed
social-loss drifts.

**Fix:** always re-snapshot the social index after any fill that changes the
sign of `size`, not only on `oi_old == 0`.

### 1.6 Maker-quote marginal-tick rationing mis-allocates when makers share the tick — [bug]

> **Status: ✅ FIXED.** Each maker quote now carries a per-level fold-time
> `cum_before` snapshot; the makers at a shared marginal tick tile `[0, Q)` exactly,
> so their fills sum to exactly `vol_alloc` and OI conserves. Regression test
> `two_makers_share_marginal_tick_and_conserve_oi`. **Breaking:** bumps
> `MakerQuote::VERSION` 2 → 3 (appended snapshot regions) — re-provision maker
> quotes.

**The bug.** The `cum_before` a maker quote used for marginal-tick rationing was
seeded only from resting orders (`order_total_bid/ask`, always `0` after §1.3 made
the slab taker-only) plus the quote's _own_ running prefix — it **omitted other
maker quotes resting at the same marginal tick**. With two makers at the exact
marginal tick on the rationed side, each computed its slice from `cum_before = 0`
as if it were the only maker, so the makers' fills did **not** tile the bucket:
their independent floors summed to `≠ vol_alloc` while the scarce (taker) side
filled the full `V`. The two sides then disagreed on matched volume — long OI ≠
short OI — and the gap was silently absorbed by the insurance pool (the old
processor comment conceded this was only "bounded by" insurance, not prevented).
It needed 2+ makers on the _exact_ same marginal tick in one round; a single maker
was already correct.

**The fix (fold-time snapshot, see `docs/plan.md`).** Because §1.3 made the maker
histogram regions (`BidDemand`/`AskSupply`) fed _exclusively_ by quotes (taker
orders go to the taker regions), the only contributors at a maker marginal tick are
the maker quotes themselves. `process_maker_quote` now records, per ladder level,
the maker-region bucket value **immediately before** that quote folds into it
(`MakerQuote.bid_snapshots_le` / `ask_snapshots_le`). That snapshot is exactly the
quote's `cum_before` among the makers at the tick, in fold order. `settle_maker_quote`
reads its own per-level snapshot and feeds it to the existing telescoping
`compute_marginal_fill`, so across all makers the slices tile `[0, Q)` and sum to
exactly `vol_alloc` for **any** crank fold order (only the ≤1-lot floor "dust" split
between makers is fold-order-dependent; aggregate OI never is). A sentinel snapshot
(`SNAPSHOT_UNFOLDED`) marks a level the histogram never saw (off-grid or an expired
quote) so it fills zero — which also closes a latent phantom-fill-on-expiry path.
This keeps `settle_maker_quote` a pure per-maker pull (no sibling-quote accounts, no
new censorship/liveness surface), matching the protocol's commutativity-not-trust
model. The now-dead resting-order slab scan was removed (the `order_slab` account is
retained in the layout but no longer read).

---

## 2. Wrong design (works today, will bite)

### 2.1 Three hand-maintained order counters, no derived invariant — [design]

> **Status: ✅ FIXED.** `finalize_clear` now additionally requires
> `all_active_orders_accumulated(slab)` — the completeness gate derives "every
> non-empty slot is folded" from the slab itself, so the censorship guarantee no
> longer rests on the parallel counters (kept as an O(1) fast-path hint). The
> counters are not yet _removed_ — that is a larger refactor — but they are no
> longer load-bearing for the gate.

`slab.count`, `Market.active_order_count`, `Market.accumulated_order_count` are
updated by _different_ instructions (`submit_order:113`, `cancel_order:81`,
`process_chunk:164`, `round.rs:79-80`). `settle_fill:242` decrements only the
slab count, never `active_order_count`.

Not a live bug (the completeness gate `finalize_clear:49` runs before settling
and the roll zeroes both), but the censorship-resistance guarantee rests on
flawless hand-bookkeeping across five instructions with nothing reconciling them.

**Better:** derive "all orders accumulated" from the slab itself (every non-empty
slot is `Accumulated`) instead of trusting parallel counters.

### 2.2 The per-slot price brake can delay liquidations during a crash — [design]

> **Status: ✅ FIXED.** All three solvency-pricing paths — `liquidate`,
> `liquidate_cross`, `withdraw_cross` — now price solvency off the **raw,
> confidence-checked per-leg oracle** via one shared resolver, `oracle::solvency_mark`
> (raw oracle when fresh; the frozen `effective_price` only when the oracle is
> *soft*-stale within `soft_stale_slots`; hard-stale → halt). The braked
> `effective_price` is still advanced/persisted for _funding_ + the soft-stale
> anchor, but no longer gates liquidation, so a crash is liquidatable immediately and
> the brake stays an anti-manipulation rail only. The cross paths now take a
> `(position, market, oracle)` **triple** per member (was a `(position, market)`
> pair) so every leg — not just the target — is priced at its real oracle; the dead
> `Market::risk_price` (the braked-mark solvency price) was removed. Regression tests
> `cross_liquidation_not_delayed_by_brake` + `cross_withdraw_not_inflated_by_brake`
> (a crash is healthy at the lagged mark, underwater at the raw oracle). No state
> layout changed → **no re-provision**; it is a breaking *calling-convention* change
> (one extra account per cross member), and `solvency_mark` unit tests cover the
> raw/soft-stale/hard-stale branches.
>
> **Interaction with §2.4 (honest tradeoff):** the extra oracle per leg pushes a
> fully-live `liquidate_cross` to `7 + 3·count` accounts (~31 at
> `MAX_CROSS_POSITIONS = 8`). §2.4's flat-leg `live_mask` path now lets any *flat*
> member ride as a single account (no oracle), so only genuinely-live legs pay the
> triple; the irreducible all-live ceiling is handled at the tx layer with an Address
> Lookup Table. Pricing correctness was never held hostage to account count.

`market.rs` (`advance_effective_price`) + `mark.rs:56` (`clamp_price_step`).
The effective price advances only when someone cranks. Before the fix, `liquidate`
priced off the raw oracle but the cross paths priced off the _braked_ value
(`risk_price`); in a fast move that mark trails the real price, so underwater
positions were not yet liquidatable (and over-withdrawal was permitted) at the
lagged mark — the anti-manipulation brake doubled as an anti-liquidation brake
exactly when bad debt accrues.

### 2.3 Mint-agnostic collateral ledger vs per-mint vaults — [design]

> **Status: ⛔ DEFERRED (gated).** Two blockers, neither resolvable in a code pass:
> (1) a **product decision** — seed-hardening (`[b"collateral", mint, owner]`) vs.
> true per-mint ledgers; (2) a **devnet re-provision** — the fix bumps
> `UserCollateral::VERSION` and relocates every collateral PDA, so deployed accounts
> become incompatible. It is also **latent-only**: the program is single-mint today
> (one vault, bound to the market's `collateral_mint`), so no cross-mint
> contamination can occur until a second mint is introduced. Recommendation when
> undertaken: seed-hardening, as its own milestone. See `docs/plan.md` Phase 6.
>
> **Operational constraint until then: the only supported collateral is USDC.**
> Many markets are allowed (SOL-perp, BTC-perp, …) but they must all settle in
> the single USDC collateral mint / vault — do **not** provision a market with a
> different `collateral_mint`.

`user_collateral.rs` seeds `[b"collateral", owner]` (no mint); `vault.rs` seeds
`[b"vault", collateral_mint]`. One global `balance` mixes mints, and `withdraw`
lets it be pulled from either vault. Single-mint today, but the data model
invites cross-mint contamination the moment a second mint exists.

### 2.4 Cross-margin is reconstruct-per-call, not maintained state — [design]

> **Status: ✅ FIXED.** Two complementary changes close the actionable problem; the
> residual is an irreducible transaction-layer concern, not a program defect.
>
> 1. **Funds-stuck trap (was the urgent half):** `MarginAccount::remove_member`
>    (compacting) + `RemovePositionFromMargin` (disc 28) free a flat member's slot,
>    so a churned group is never permanently full.
> 2. **Reconstruct-per-call account pressure (this pass):** a **flat** member
>    (`size == 0`) contributes zero unrealized PnL, zero maintenance, and zero
>    unsettled funding/social, so it needs only its stored `realized`/`collateral` —
>    **no market, no oracle**. `withdraw_cross`/`liquidate_cross` now take a
>    `live_mask` byte: bit `i` set ⇒ member `i` is a `(position, market, oracle)`
>    *live triple*; clear ⇒ a *flat* member supplied as a bare `position` account
>    (1 account, not 3). A member claimed flat that is not actually flat **fails
>    closed** (it would otherwise hide its loss + maintenance from the gate). So a
>    group of 1 live + 7 flat legs drops `liquidate_cross` from 31 → ~17 accounts.
>    Recompute-per-call is **kept** (no maintained aggregate → no new drift surface,
>    cf. §2.1); the close target is now the first *non-flat* member. No state-struct
>    layout change → **no migration / re-provision**; breaking *instruction-data*
>    change (the mask byte) → clients regenerated. Regression tests
>    `cross_withdraw_accepts_flat_member_as_bare_single` +
>    `cross_liquidation_accepts_flat_member_as_bare_single`.
>
> **Why "maintain netted equity/maintenance as state" was dropped (honest reframe):**
> it is **not implementable** — both combined equity and combined maintenance are
> functions of each leg's *live* oracle price (`unrealized_pnl(size, entry, mark)`,
> `|size|·mark·bps`), so there is no static scalar to store, and any cached mark
> reintroduces exactly the §2.2 stale-pricing bug. The corollary: **a correct
> combined-health check must mark every *live* leg to its current oracle**, which is
> irreducibly `(position, market, oracle)` per live leg. The flat-leg path removes
> the only avoidable cost; the remaining live-leg ceiling (≈8 live legs ⇒ ~31
> accounts) is a **transaction-layer** concern, solved off-chain with an Address
> Lookup Table (a tx can then reference up to 256 accounts) — no program change. The
> `migrate_margin_account` milestone is therefore retired, not deferred.

Before this pass, every `withdraw_cross`/`liquidate_cross` had to supply **all**
members as full triples (`members.len() == count*3`). At `MAX_CROSS_POSITIONS = 8`
that is ~31 accounts, near the per-tx ceiling. Combined with the append-only group
(pre-§2.4-removal), a group that churned through 8 positions became
**un-withdrawable even if only one leg was live**.

### 2.5 `margin_mode` is mutable mid-auction — [design]

> **Status: ✅ FIXED.** `add_position_to_margin` now takes the market + order slab
> and rejects the bind (via `count_trader_live_orders`) when the owner has any
> in-flight `Resting`/`Accumulated` order this round, so a position can't be flipped
> to cross while a fill sized as isolated is pending.

`add_position_to_margin/processor.rs:32-35` requires only `size()==0`, so a
position with an in-flight _accumulated_ order (size still 0 pre-settle) can be
flipped to cross. `settle_fill/processor.rs:368` then locks **no** isolated
margin for a fill that was sized as isolated.

**Fix:** also reject positions that have an in-flight accumulated order when
adding to a group.

### 2.6 `migrate_position` OI rebuild depends on an unenforced ordering — [design]

> **Status: ✅ FIXED.** `migrate_position` now takes the order slab and gates the v1
> OI rebuild on an **empty slab** (`count == 0`, the `start_auction` quiescence
> condition), so no in-flight settle can race the OI counters. Combined with the
> existing exact-version checks (which already enforce market-then-position ordering
>
> - once-per-position idempotency), the double-count is closed without a new Market
>   field. See `docs/plan.md` Phase 4 for the recorded decision.

`migrate_position/processor.rs:80` re-adds a v1 position's size to market OI,
assuming `migrate_market` already zeroed OI. Nothing enforces market-then-position
ordering; run against an already-live v5 market and OI double-counts, corrupting
the ADL denominator.

### 2.7 Structural, documented-but-real — [design]

> **Status: ✅ FIXED (all three).** Breaking layout/IDL change → **fresh devnet
> re-provision required** (bumps `Market` VERSION 6→7 and `OrderSlabHeader` VERSION
> 1→2; widens `Order` 72→80; adds the `oracle` account to `start_auction` and a
> `slot_hint` arg to `settle_fill`/`cancel_order`). Clients regenerated (both
> bundles).

- ✅ **Oracle-anchored tick window** (`market.rs` `window_floor_price` +
  `recenter_window`): the window is no longer pinned at genesis. A new
  `window_floor_price` field anchors tick 0; `price_to_tick` is now
  `(price − floor)/tick_size`. The floor is re-snapped onto the oracle (centered:
  `oracle − (num_ticks/2)·tick_size`, snapped to the grid) at every `start_auction`
  and at `initialize_market`. It is **frozen for the whole round** (the histogram is
  tick-indexed, so the anchor must not move mid-round) — safe because the
  collection window is only `COLLECT_WINDOW_SLOTS = 2` slots, so the price is
  effectively still within a round and re-anchoring happens ~every round. A
  stale/missing oracle **carries the previous floor forward** (never blocks the
  permissionless roll); `start_auction` hard-rejects only a *wrong* oracle account
  (must equal `market.oracle`). The genesis default (`floor = tick_size`) reproduces
  the old mapping exactly. Tests: `market.rs` recenter unit tests + the
  `window_recenter.rs` integration suite (init/roll recenter, carry-forward,
  out-of-window rejection).
- ✅ **O(n²) settle → O(1) slab access**: the slab stays a flat array, but the
  per-tx scans are gone. (1) A **bump cursor** (`OrderSlabHeader.next_free_hint`)
  makes `submit_order` allocation O(1) (forward-fill), wrapping to reclaim
  cancel-holes only when the tail is full. (2) A **validated slot hint** on
  `settle_fill`/`cancel_order` (emitted in `OrderSubmitted.slot`) makes the order
  lookup O(1) — validated against the slot's `order_id`, never trusted, with a scan
  fallback on a stale hint. (3) A **fold-time `cum_before` snapshot** on each
  `Order` (recorded by `process_chunk`, mirroring the §1.6 MakerQuote fix) replaces
  the marginal-tick `cumulative_qty_before` rescan; the telescoping prefixes tile
  `[0, total_qty)` for any crank fold order, so Σ fills == `vol_alloc` (aggregate
  conserves; only ≤1-lot dust is fold-order-dependent). Settle now does **zero**
  slab scans on the happy path.
- ✅ **`read_oracle` hardcodes the SOL feed** — **FIXED** (earlier): `read_oracle`
  reads the market's `oracle_feed_id`, matching `update_funding`/`liquidate`.

### 2.8 `withdraw_cross` masks collateral-ledger drift with `saturating_sub` — [design]

> **Status: ✅ FIXED.** `foreign_locked` now uses `checked_sub` and returns a new
> hard `CollateralLedgerDrift` error when `member_locked > uc.locked()`, so a member-set
> ↔ ledger drift surfaces loud instead of clamping the foreign-locked floor to 0. On
> the normal (non-drift) path `checked_sub` is identical to the old `saturating_sub`,
> so no behavior changed for healthy accounts; the error is unreachable through the
> public instruction set today (which is why it stayed low-severity), so it has no
> dedicated integration test — triggering it requires inducing the very invariant
> violation the guards prevent.

`withdraw_cross/processor.rs:147`. `foreign_locked = uc.locked() - member_locked`
uses `saturating_sub`, so if the summed member collateral ever **exceeds** the
ledger's total `locked` (i.e. the `MarginAccount` member set and `UserCollateral`
have drifted), the foreign-locked floor silently collapses to `0` instead of
erroring — letting the owner withdraw collateral that should stay reserved for
positions _outside_ the group, under-collateralizing them.

**Fix:** treat `member_locked > uc.locked()` as an invariant violation
(`checked_sub` → a hard error), not a clamp — a drift here is a bug to surface,
never to absorb.

### 2.9 Low-severity batch — fix together in one cleanup pass — [design] · [dead] · [reuse]

> **Status: ✅ FIXED (4 of 5; (d) intentionally skipped).** Four of the five minor
> items are fixed in this pass; (d) is intentionally skipped (an unmeasured CU
> micro-optimization bounded by `MAX_CROSS_POSITIONS = 8` — adding parsed-market
> caching + the attendant state-aliasing care is not worth it pre-benchmark, the
> same measurement-first principle that keeps the §2.7 O(n²) item deferred). None of
> the five is a present-tense fund-loss/mint bug. (The same review's two
> genuinely-actionable findings — the stale web vendor bundle and a missing
> `live_mask`↔`MAX_CROSS_POSITIONS` compile guard — were fixed in the §2.4 pass; the
> headline "all-flat liquidation regression" was a false positive: old and new code
> both return `NotLiquidatable` for an all-flat group, and the new "first non-flat
> target" is strictly *more* permissive.)

- **(a) ✅ Oracle future-timestamp falls back to the frozen mark** (`oracle.rs`
  `read_price` / `solvency_mark`). **Fixed:** `read_price` now splits the staleness
  check — a `publish_time` beyond `now + MAX_AGE_SECS` returns a distinct new
  `OracleFutureTimestamp`, while only an honestly-old update returns `OracleStale`.
  Because `solvency_mark` falls back to the frozen mark *only* on `OracleStale`, a
  future timestamp now hard-rejects everywhere instead of entering the soft-stale
  window. Unit tests: `test_future_timestamp_rejected_distinctly`,
  `test_solvency_mark_future_timestamp_does_not_soft_stale`.
- **(b) ✅ Combined-health math is hand-inlined, not the tested helper**
  (`cross_margin.rs`). **Fixed:** a new pure `leg_contribution(leg, bps, realized,
  pending, credit_unrealized_gains)` is the single per-leg health primitive — it
  models the pending funding + social-loss dock the old `account_*` helpers lacked,
  and the one `credit_unrealized_gains` flag captures the genuine difference between
  the two callers (liquidation marks to the true price; withdrawal applies the
  backed-profit rule, crediting only losses). Both `withdraw_cross` and
  `liquidate_cross` now route through it, and `account_equity`/`account_maintenance`
  are reimplemented on top, so the unit-tested math **is** the executed math. Unit
  test `test_leg_contribution_credit_modes_and_pending`.
- **(c) ✅ Wrong error class for an all-flat group** (`liquidate_cross/accounts.rs`).
  **Fixed:** the account floor dropped from `< 10` to `< 8` (7 fixed + ≥1 bare
  member), so an all-flat group reaches the processor's combined-health check and
  returns the semantic `NotLiquidatable` rather than aborting as
  `NotEnoughAccountKeys`. The processor's exact `live_mask` length check still
  rejects a genuinely short account list.
- **(d) ⬜ Same-market legs re-parse the `Market` + oracle per leg**
  (`liquidate_cross/processor.rs` loop) — **intentionally skipped.** Re-deserializing
  a shared market twice is bounded by `MAX_CROSS_POSITIONS = 8` and sits well within
  the CU budget; caching parsed markets by key adds real state-aliasing care for a
  micro-optimization no benchmark has yet shown to matter. Left deferred under the
  same measurement-first principle as the §2.7 O(n²) settle item.
- **(e) ✅ Dead `MarginMarketStale` error variant** (`errors.rs`). **Fixed:** kept and
  marked **reserved** with a comment (mirroring `MarketPaused`) rather than removed —
  deleting a mid-enum variant would renumber every following error code (`e as u32`),
  needlessly breaking the stable client error map for a cosmetic gain.

### 2.10–2.12 Pre-trade-safety reservation follow-ups (code-review) — [design]

> Surfaced by the `/code-review` of the missing-features §1.1/§1.2 pre-trade margin
> reservation (Market v8 / OrderSlab v3). The **urgent** finding from the same review —
> `force_reset` permanently stranding a resting order's `reserved_margin` in
> `user_collateral.locked` on a money-path market (real fund loss) — is **not** filed
> here because it is being handled as an active fix, not an accepted limitation. The
> three items below are the non-urgent residue.

#### 2.10 Reduce-only settle race — unverified (likely a false positive) — [design]

> **Status: ⬜ UNVERIFIED.** A verifier flagged that a reduce-only order (which
> reserves margin only for the portion past its reduce headroom) could open more
> exposure than reserved if another of the trader's orders settles first and shrinks
> the offsetting position, reverting the relock and wedging the round. Re-derivation
> suggests it is **covered**: `submit_order` charges the trader's already-resting
> same-side quantity against the headroom (`trader_resting_stats`), so the *sum* of
> reducing orders can never exceed `|pos|` and any flip portion is reserved by the
> order that exceeds the headroom; whatever shrinks the position first releases a
> reservation ≥ what it frees, so settle always has enough (checked across both settle
> orderings). Kept open only until a focused conservation **fuzz** — random
> reduce-only / opposite-order interleavings, asserting no settle revert — confirms it.

`submit_order/processor.rs` (reduce headroom) · `settle_fill/processor.rs` (relock).

#### 2.11 Releasing a §1.1 reservation requires the collateral ledger on cancel/zero-fill settle — [design]

> **Status: ⬜ BY-DESIGN (client + doc parity).** The pre-trade reservation is held in
> `user_collateral.locked` and must be released when the order leaves the slab, so
> `cancel_order` and `settle_fill` now require the owner's `user_collateral` whenever
> the order carries `reserved_margin > 0` — **including a zero-fill settle** (the
> CLAUDE.md note "only a zero-fill order may settle without a ledger" is now stale and
> should be updated). Not a wedge: a permissionless cranker always knows the order's
> owner, so it can attach the PDA-derived ledger; a stale 5-account client simply gets
> `MissingSettleAccounts` and must append the account. No program fix needed beyond
> client regen + the doc note.

`cancel_order/processor.rs` · `settle_fill/processor.rs`.

#### 2.12 Devnet client bundle + IDL drift after the v8 layout bump — [design] · operational

> **Status: ⬜ OPERATIONAL (before any devnet money-path run).** The pre-trade-safety
> pass bumps `Market` VERSION 7→8 (`initial_margin_bps` + `max_position_notional`;
> `InitializeMarket` data 111→129 bytes) and `OrderSlabHeader` 2→3 (`Order` 80→88, adds
> `reserved_margin`), and adds optional `position`/`user_collateral` accounts + a
> `reduce_only` byte to `submit_order` (optional `user_collateral` to `cancel_order`).
> The generated TS client + `apps/bots/vendor/tempo-client.mjs` predate this, so the
> money-path bot scripts build the old call and fail at the first `submit_order`. Before
> any devnet run: `pnpm generate-clients && pnpm bundle-client` and **re-provision** the
> market (old `Market`/`OrderSlab` accounts fail the version check — re-provision, not
> migrate).

`apps/bots/vendor/tempo-client.mjs` · `apps/bots/src/*.ts`.

---

## 3. Dead / redundant code

> **Status: ✅ FIXED.** All dead-code items are done — the safe non-layout ones
> (`price→tick`, `MarketPaused`, `close_maker_quote`), the three dead struct fields
> (removed outright), **and** the final two: the `compute_fill` residual floor branch
> and the `classify_level` duplication are both closed at the root by a single new
> shared clearing primitive, `clearing::fill_against_cross`. Pure-math refactor only —
> no instruction/state-layout change, so **no client regen and no re-provision**.

- ✅ **FIXED** — Triplicated `price→tick` `-1` map: extracted to one
  `price_to_tick_raw`; `Market::price_to_tick`, `process_chunk`, and `settle_fill`
  all delegate, so it can no longer drift.
- ✅ **DOCUMENTED (kept)** — `TempoProgramError::MarketPaused`: kept with a comment
  marking it reserved for the unbuilt pause feature (`missing-features.md` §3.2).
- ❎ **NOT DEAD (kept)** — `AuctionHistogramHeader.accumulated_count`: asserted by
  the `happy_path`/`lifecycle` integration tests as an indexer-observable, so the
  write is not removed.
- ✅ **FIXED** — `compute_fill` floor pro-rata tail (`clearing.rs`) and the
  `classify_level` duplication (`settle_maker_quote`) were one problem and are closed
  together. A single new pure primitive `clearing::fill_against_cross(&AuctionCross,
  is_buy, tick, qty, cum_before)` is now the **only** fill classifier: it owns the
  side-correct marginal boundary (Buy ≥ P\* / Sell ≤ P\*), fills strictly-better and
  scarce-side-marginal fully, and routes only the rationed marginal tick to
  `compute_marginal_fill`. Both `settle_fill` (taker orders) and `settle_maker_quote`
  (maker ladder levels, `classify_level` deleted) call it, so the boundary can no
  longer drift between the two paths — the conservation-break footgun is removed at the
  root. With the rationed-marginal case handled by `compute_marginal_fill`,
  `compute_fill` had no production caller left (its only remaining branches returned
  full/zero, now inlined in `fill_against_cross`) and the dead floor pro-rata tail went
  with it: `compute_fill` is **deleted**. `fuzz_full_book_conserves_oi` (20k random
  books) now settles every order through `fill_against_cross` and asserts
  `Σ buys == Σ sells == V`, so the shared classifier is proven OI-conserving; a focused
  `test_fill_against_cross_classification` covers every side × position case.
- ✅ **FIXED** — `clear_maker_quote` deactivated but never closed the PDA, so rent
  was trapped **and** the maker could never re-init the deterministic
  `[b"maker_quote", market, maker]` address to re-quote. A new additive
  `close_maker_quote` (disc 29) closes a _cleared_ (`status == 0`) quote and refunds
  rent to the maker — maker-only (a write delegate cannot close), inactive-only (an
  active, folded quote must be cleared first so it can't be pulled out from under
  `finalize_clear`). No layout/`VERSION` change, no re-provision; covered by
  `maker_quote::maker_quote_close_reclaims_rent_and_frees_pda`.
- ✅ **FIXED (dead struct fields removed)** — three never-read fields were deleted
  outright (dev-phase decision: nothing released, so re-provision is free rather than
  a feared "batch with §2.3" event). Each was a layout change rippling through the
  IDL, both generated clients, and the test-harness offset readers — all updated in
  lockstep, verified by the serde roundtrip + full integration suites:
  - `Vault.maintenance_margin_bps` / `liquidation_penalty_bps` — pure duplicates of
    the **market's** copies (the only ones `liquidate`/`settle` read); dropped from
    `Vault`, `InitVault` data, and the IDL.
  - `Market.sync_fee_multiplier` — parsed + bounds-checked + stored by
    `initialize_market` but never applied; dropped (the `initialize_market` data
    layout is now 111 bytes, not 112).
  - `MakerQuote.sync_spread_ticks` — an "M5 sync-flow" placeholder with no instruction
    even setting it; dropped.

  > **Operational note:** these are breaking layout changes. `migrate_market`'s v4→v5
  > path can no longer match genuine pre-existing v4 accounts (it fails closed, never
  > corrupts) — irrelevant because the change **requires a fresh devnet re-provision**
  > (the deployed binary + all `Vault`/`Market`/`MakerQuote` accounts must be
  > recreated). The two "sync" fields were unbuilt-feature hooks; re-add them when the
  > sync-flow fee feature is actually built.

---

## 4. Off-chain service issues (off-chain crates — new findings)

These issues were found in the off-chain Rust crates (`crates/keeper`, `crates/mm-bot`,
`crates/liquidator`, `crates/api`, `crates/sdk`, `crates/tempo-math`). None are in the
on-chain program. Status legend matches §1–§3 above.

### Priority order

---

### 4.1 No priority fees — keeper and liquidator transactions won't land reliably on a busy cluster — [design] · [production-blocker]

> **Status: ✅ fixed.** `TEMPO_PRIORITY_FEE_MICRO_LAMPORTS` wired into `common::Config`, `TxSender::new`, and `TempoClient::new` across all four services. `.env.example` updated.

`crates/common/src/tx.rs`: `TxSender::send` prepends `set_compute_unit_limit` but
never `set_compute_unit_price`. On mainnet or a congested devnet, transactions without
a compute-unit price are deprioritised by validators and frequently dropped.

The keeper drives the clearing protocol; if its transactions don't land, the market
freezes. The liquidator fires time-sensitive liquidations; delays here mean bad debt
accumulates. Both services need a configurable `TEMPO_PRIORITY_FEE_MICRO_LAMPORTS`
knob wired into `TxSender`.

**Fix:** add `ComputeBudgetInstruction::set_compute_unit_price(micro_lamports)` to
`TxSender::send` alongside the existing limit instruction; expose the fee as a config
field defaulting to a conservative non-zero value.

---

### 4.2 API watcher runs `getProgramAccounts` for positions on every 400ms poll — [design] · [performance]

> **Status: ✅ fixed.** `fetch_positions` moved out of `LiveState::load` into a separate `run_positions` task (default 5s). `AppState` gains `positions: Arc<ArcSwapOption<...>>`.

`crates/api/src/state.rs:45`: `LiveState::load` calls `client.fetch_positions` (a full
`getProgramAccounts` scan with memcmp filters) on every watcher tick. GPA is
expensive, slow (often 1–5s), and unsupported or rate-limited by many RPC providers.
At 400ms poll cadence this saturates the RPC connection and makes position data the
bottleneck of the entire API.

This is the indexer's job (`HistorySource` / `IndexerSource` seam in `crates/api/src/history.rs`).
Until the indexer exists the immediate fix is to move position GPA behind a longer
cache TTL (e.g. 5s) or a separate lower-cadence task rather than running it on every
watcher tick.

**Fix (interim):** separate the position scan into its own task running on a longer
interval (e.g. 5s); watcher does not call `fetch_positions` every tick.
**Fix (durable):** route positions through `HistorySource` once the indexer lands.

---

### 4.3 ALL `TxFailed` errors classified benign — real instruction failures are swallowed — [design]

> **Status: ✅ fixed.** `TxFailed` arm now calls `is_race_error(err)` (same content check as the `Rpc` arm). New tests cover benign and non-benign `TxFailed` variants.

`crates/sdk/src/retry.rs:14`:
```rust
SdkError::Common(tempo_common::CommonError::TxFailed { .. }) => true,
```
Any on-chain execution failure — including a misbuilt instruction, a wrong account
structure, or a keeper bug — is treated identically to a benign phase-race. The keeper
and liquidator log a warning but do not back off, silently making no progress while
the watchdog takes `no_progress_slots` to fire.

`TxFailed` should check the error content the same way the `Rpc` arm does (contains
"custom program error" / "already" / "wrong phase" / "not found"). Other `TxFailed`
variants are real failures.

**Fix:** change the `TxFailed` arm to inspect `err` field content before returning
`true`; fall through to `false` for unrecognised error strings.

---

### 4.4 `liquidation_outcome` uses unchecked `u128 * u128` — panic on large notional — [bug]

> **Status: ✅ fixed.** Changed to `.saturating_mul` at `margin.rs:102`, consistent with `maintenance_margin`.

`crates/tempo-math/src/margin.rs:102`:
```rust
let notional = size_signed.unsigned_abs() * (mark as u128);  // unchecked
```
`maintenance_margin` at line 20 uses `.saturating_mul`; `liquidation_outcome` does
not. With `overflow-checks = true` in the workspace release profile this panics rather
than wrapping, crashing the liquidator and mm-bot on extreme size×mark values.

**Fix:** change to `.saturating_mul` (consistent with `maintenance_margin`).

---

### 4.5 `ensure_accounts` in mm-bot silently swallows all `init_position` errors — [bug]

> **Status: ✅ fixed.** `init_position` now uses the same `benign`-checked warn pattern as `init_maker_quote`.

`crates/mm-bot/src/lib.rs:129`:
```rust
match ctx.client.send(&ctx.maker, &[init_pos]).await {
    Ok(_) | Err(_) => {}  // swallows network-down, program panics, everything
}
```
Ten lines later `init_maker_quote` correctly checks `benign(&e)` and warns on
non-benign errors. If the RPC is down during startup, the position account may not
exist; subsequent `settle_maker_quote` calls will fail with a non-benign error every
round, silently leaving the market unquoted.

**Fix:** apply the same `benign` check used for `init_maker_quote`.

---

### 4.6 `last_quoted` set even when `post_quote` fails non-benignly — mm-bot goes silent for a full round — [bug]

> **Status: ✅ fixed.** `post_quote` returns `Result<bool, SdkError>`; `last_quoted` only set when it returns `true`.

`crates/mm-bot/src/lib.rs:184-185`:
```rust
post_quote(ctx, health, &market, &quote, key).await;  // returns ()
*last_quoted = Some(key);                              // always set
```
`post_quote` returns `()` regardless of outcome. A real `update_maker_quote_levels`
failure logs a warning inside `post_quote`, but `last_quoted` is marked done
unconditionally. The tick returns `Ok(())` — the caller has no visibility into the
failure — and the bot does not retry until the auction_id or window_floor changes
(next round). One transient error silently leaves the market without maker liquidity
for a full auction round.

**Fix:** make `post_quote` return `Result<(), SdkError>` and only set `last_quoted` on
success (or on a benign error, which means the quote landed via another instance).

---

### 4.7 Liquidator cross-account resolution is fully serial — [design] · [performance]

> **Status: ✅ fixed.** `resolve_cross` takes owned caches; cross owners resolved via `buffer_unordered(scan_concurrency)`.

`crates/liquidator/src/lib.rs:159`: isolated liquidations are fanned out with bounded
concurrency via `for_each_concurrent`, but cross accounts are resolved and fired
serially in a `for owner in &scan.cross_owners` loop. Each cross resolution does
multiple sequential RPC calls (margin account, collateral ledger, each member
position/market/oracle). With many cross accounts this blocks the scan, delaying
isolated liquidations queued behind it.

**Fix:** fan out `resolve_cross` + fire with the same bounded concurrency used for
isolated liquidations.

---

### 4.8 MakerQuote sequence falls back to 0 on any RPC error — stale-sequence loop — [bug]

> **Status: ✅ fixed.** Uses `fetch_account_data_opt`; propagates RPC errors instead of defaulting to sequence 0.

`crates/mm-bot/src/lib.rs:218-225`:
```rust
let on_chain_seq = ctx.client.fetch_account_data(&ctx.maker_quote).await
    .ok()
    .and_then(|d| MakerQuoteView::decode(&d).ok())
    .map(|q| q.sequence)
    .unwrap_or(0);
let next_seq = on_chain_seq + 1;
```
Any RPC error or decode failure silently resets the sequence to 0, sending seq 1.
If the on-chain sequence has advanced (e.g. a second instance sent seq 5), this
instance permanently sends stale sequences until it gets a successful fetch. Every
tick results in a benign rejection but no valid quote is posted.

**Fix:** propagate the fetch error (return early from `post_quote` with a warning)
rather than defaulting to 0; only use 0 when the account provably does not exist yet
(`fetch_account_data_opt` returning `None`).

---

### 4.9 One MakerQuote PDA per maker — single-ladder limit — [design]

> **Status: ⛔ deferred.** Requires program PDA change. Interim workaround documented in `ops/README.md` and `crates/sdk/src/pda.rs`.

The PDA seeds `[b"maker_quote", market, maker]` allow only one active quote per
maker per market. The mm-bot cannot widen its posted depth by running two ladders, and
if the single quote is folded mid-round (status transitions to folded) the bot cannot
re-quote until `clear_maker_quote` + next round. A multi-level PDA (e.g. include a
`quote_id` seed) would allow multiple concurrent quotes per maker.

---

### 4.10 `benign()` uses fragile string-matching — [design]

> **Status: ⬜ accepted limitation.** `TODO` comment + regression guard test added in `crates/sdk/src/retry.rs`.

`crates/sdk/src/retry.rs:15-21`: error classification relies on
`m.contains("custom program error")` / `m.contains("already")` etc. over the RPC
error string. If Solana or the RPC provider changes the error message format, the
classifier silently changes behaviour — becoming either too permissive (real errors
swallowed) or too strict (benign races cause backoff). No unit test covers an
unexpected message format.

The durable fix is for the program to surface structured error codes the SDK can
match numerically. Until that changes, the string-based approach is the only option;
document it as a known fragility.

---

## 6. Fix order — status (off-chain)

Done in this pass (✅): **1.1, 1.2, 1.4, 1.5** (the money-path collapse), **1.3**
(`is_maker` steering — Option A: `submit_order` taker-only, makers only via the
MakerQuote book), **2.1, 2.2** (full — raw per-leg oracle solvency across
`liquidate`/`liquidate_cross`/`withdraw_cross` via shared `oracle::solvency_mark`),
**2.4** (full — member removal + the flat-leg `live_mask` cheap path; the
"maintain equity as state" milestone was retired as infeasible, live-leg ceiling is
ALT territory), **2.5, 2.6**, **2.7** (full — oracle-anchored tick window +
O(1) slab access + read-oracle feed), and **all §3 dead-code items** (price→tick,
MarketPaused, `close_maker_quote`, the three dead struct fields removed outright, and
finally `compute_fill` + `classify_level` collapsed into the one shared
`clearing::fill_against_cross` classifier).

Remaining, in priority order:

1. **2.3** per-mint collateral seeds — gated on the seed-hardening product decision.
   (The §3 `[LAYOUT]` dead-field deletions that were previously batched here are now
   done; they already force the devnet re-provision this milestone would have shared —
   as does the **2.7** window/slab re-provision, so batch them.)

> Note: 1.1, 1.2, 1.4, and 1.5 shared one root cause — the settlement money-path
> was copy-pasted into four processors and the copies drifted. The durable fix
> collapsed the two **settle** paths into one shared, fuzzed `settle_money`
> primitive. `liquidate`/`liquidate_cross` were intentionally **left on their own
> conserving code**: their accounting is in `insurance_delta = collateral −
returned − penalty` (a positive delta means insurance _grows_), the sign-inverse
> of the helper's `balance_delta` (a positive delta means a winner _draws_ and must
> fail closed). Forcing them through the helper would change their proven semantics
> (`fuzz_liquidation_outcome_conserves`), so unifying all four is a deliberate
> follow-up, not a quick refactor.
