# Tempo — Implementation Plan: All Open Issues & Missing Features

**Date:** 2026-07-07
**Scope:** every open item in `docs/known-issues.md` (Part A) and every absent/partial item in `docs/missing-features.md`, per the analysis in `docs/issues-and-features-analysis.md`.
**Grounding:** all snippets below are written against the actual code on `main` (Market v11, OrderSlab v6, MakerQuote v3, Vault v2, 32 instructions, 47 error codes). File/line references were verified by reading the sources.

---

## 0. Ground rules (read first)

### 0.1 How every new instruction is wired (the house checklist)

Adding instruction `Foo` always means touching exactly these places (mirror `init_shard/` — the smallest recent example):

1. `program/src/instructions/foo/` — new dir with `mod.rs`, `accounts.rs` (a `TryFrom<&[AccountView]>` doing ALL account checks), `data.rs` (a `TryFrom<&[u8]>` with `const LEN`), `processor.rs` (logic only).
2. `program/src/instructions/impl_instructions.rs` — `define_instruction!(Foo, FooAccounts, FooData);`
3. `program/src/instructions/mod.rs` — `pub mod foo; pub use foo::*;`
4. `program/src/traits/instruction.rs` — add `Foo = N` to `TempoInstructionDiscriminators` **and** to its `TryFrom<u8>`.
5. `program/src/entrypoint.rs` — add the match arm.
6. `program/src/instructions/definition.rs` — add the Codama variant with `#[codama(account(...))]` attributes and `= N`.
7. Regenerate clients: `just generate-clients`, commit `idl/` + `clients/` + `crates/sdk/src/generated/` diffs.
8. `cargo test --features idl` + `cargo-build-sbf` before claiming done.

### 0.2 Layout-bump batching (the most important scheduling rule)

Every `Market`/`MakerQuote`/`Vault` version bump forces a devnet **re-provision** (version byte fails old accounts loudly — by design). So:

- **`Market` v11 → v12 happens ONCE, in Phase 1**, and the v12 append block contains **every** field any later phase needs (pause, min-notional, OI cap, reward floor, close buffer, the staged-change block). Fields are inert until their instructions land — appending them early costs 108 bytes and saves three re-provisions.
- **`MakerQuote` v3 → v4 happens ONCE, in Phase 1** (quote-time margin + multi-quote seeds together).
- **`Vault` v2 → v3 happens ONCE, in Phase 2** (authority + user-balance aggregate + staged insurance withdraw).
- `Order`/`OrderSlab`/`Position`/`ClearingResult`/`Histogram` layouts are **not touched** by this plan.

### 0.3 New discriminators, errors, events (reserved here, used throughout)

**Instruction discriminators** (append to `TempoInstructionDiscriminators`, `TryFrom<u8>`, entrypoint, `definition.rs`):

| # | Instruction | Phase |
|---|---|---|
| 32 | `SetPause` | 1 |
| 33 | `UpdateMarketParams` (hot set, immediate) | 2 |
| 34 | `ProposeRiskUpdate` | 2 |
| 35 | `ApplyRiskUpdate` (permissionless after delay) | 2 |
| 36 | `ProposeAuthorityTransfer` | 2 |
| 37 | `AcceptAuthorityTransfer` | 2 |
| 38 | `ProposeSetOracle` | 2 |
| 39 | `ApplySetOracle` (permissionless, gated) | 2 |
| 40 | `SeedInsurance` (permissionless) | 2 |
| 41 | `ProposeInsuranceWithdraw` | 3 |
| 42 | `ApplyInsuranceWithdraw` | 3 |
| 43 | `CancelAllOrders` | 4 |
| 44 | `ClosePosition` (flat-position rent reclaim) | 5 |
| 45 | `CloseMarket` | 5 |

**Error codes** (append after `OrderAlreadyExpired = 46`; never renumber):

```rust
/// (47) No staged change of the expected kind is pending on this account.
#[error("No pending update of this kind")]
NoPendingUpdate,

/// (48) A staged change's effective slot has not been reached yet.
#[error("Pending update delay has not elapsed")]
PendingDelayNotElapsed,

/// (49) The market is not fully quiescent (round in flight, open interest,
/// live orders, or active maker quotes) for an operation that requires it.
#[error("Market is not quiescent")]
MarketNotQuiescent,

/// (50) The order would push the market's per-side open interest past
/// `max_open_interest` (soft cap, checked at submit — see plan §4.3).
#[error("Order would exceed the market's open-interest cap")]
OpenInterestCapExceeded,

/// (51) The vault token balance no longer covers user balances + insurance.
/// Fail-closed gate at token-outflow sites (plan §3.4).
#[error("Vault backing invariant violated")]
VaultInvariantViolated,
```

Also **wired up** (already reserved, currently dead): `MarketPaused` (2), `OrderBelowMinimum` (29), `LiquidationNoProgress` (34).

**Event discriminators** (append to `EventDiscriminators`, each a new file in `events/` mirroring `funding_updated.rs`):

| # | Event | Fields |
|---|---|---|
| 9 | `MarketParamsUpdated` | market, kind u8, payload [u8;64] |
| 10 | `MarketPauseChanged` | market, paused u8 |
| 11 | `OracleRepointed` | market, old_oracle, new_oracle, new_feed_id |
| 12 | `AuthorityTransferred` | market, old_authority, new_authority |
| 13 | `InsuranceSeeded` | vault mint, donor, amount u64 |
| 14 | `InsuranceWithdrawn` | vault mint, authority, amount u64 |

`PositionLiquidatedEvent` gets two **appended** fields (`closed_qty: u64`, `remaining_size: i64`) — prefix-compatible for existing indexer decoders; bump its `DATA_LEN` from 104 to 120.

---

## 1. Phase 0 — Hygiene (no program changes)

### 1.1 Regenerate the web vendor bundle + CI guard (known-issues §2.12)

```bash
pnpm generate-clients && pnpm bundle-client
grep -c "numSlabShards" apps/web/src/vendor/tempo-client.mjs   # must be > 0
```

Then extend the CI clients-fresh job (`.github/workflows/`) so the vendored bundle can never lag again:

```yaml
  - name: web vendor bundle fresh
    run: |
      pnpm generate-clients && pnpm bundle-client
      git diff --exit-code apps/web/src/vendor/tempo-client.mjs
```

Re-provision any devnet market still on a pre-v11 layout (re-provision, not migrate).

### 1.2 Doc corrections (analysis "Feature 0" + §2.11)

- `docs/missing-features.md` §1.1 and §2.2: replace the "reserves only the portion that would open new exposure" text — the code reserves the **full** worst-case margin (`submit_order/processor.rs:193-206`, DDR-3 Correction-2 item 3); `reduce_only`'s only job is forcing `Consumed` at settle.
- `docs/missing-features.md` §1.3: `orders_per_auction_cap ∉ (0, 128]` → `(0, 90]` (`MAX_ORDERS_PER_AUCTION_CAP = 90`, `initialize_market/data.rs:25`).
- `docs/known-issues.md` §2.11: mark closed once the SDK change below lands.
- `docs/missing-features.md` §7.2: change "absent (by design)" to a closed "won't build" note.

### 1.3 SDK: always attach the derivable ledger on settle (§2.11)

In `crates/sdk`'s settle-ix builder: on a money-path market, always derive and append `[b"collateral", order.trader, market.collateral_mint]` — it removes the "was this settle fully-consuming?" client-side guesswork entirely. Keeper inherits it via the SDK.

### 1.4 Stage-B marketable-fill end-to-end test (known-issues §2.13)

New `tests/integration-tests/tests/marketable_fill.rs`. Skeleton (the harness already writes synthetic Pyth accounts for the oracle tests — reuse that helper):

```rust
#[test]
fn recentered_window_fills_marketable_resting_sell_against_live_buy() {
    let mut ctx = TestContext::new_money_market(/* 64 ticks, tick 10 */);
    // 1. resting sell at price P (in window), GTC
    ctx.submit_order(seller, Sell, p, 100, /*reduce_only*/ false, shard0, GTC);
    ctx.run_full_round();                       // no counterparty -> rests
    // 2. move the synthetic Pyth price so the recentered floor > P
    ctx.set_oracle_price(p_plus_window);        // floor recenters above P
    ctx.start_auction();                        // recenter happens here
    // 3. live buy at the new window's mid
    ctx.submit_order(buyer, Buy, mid, 100, false, shard0, GTC);
    ctx.crank_round_to_settled();
    // 4. assertions
    let cr = ctx.clearing_result();
    assert!(cr.bid_matched_volume() == 100);    // sell folded at boundary tick 0
    assert!(ctx.fill_price(seller_order) >= p); // filled at >= its limit
    ctx.assert_oi_conserved();
    ctx.assert_margin_release_exact(seller_order);
}
```

Mirror test for a buy above the window top (`Marketable(num_ticks-1)`), plus the same scenario as a `crates/sim` devnet script. **This must pass before Phase 1's maker-margin work**, since §7.1 reuses the same `worst_price` snapshot idea.

---

## 2. Phase 1 — The safety release (one `Market` bump + one `MakerQuote` bump)

### 2.1 `Market` v12: the complete append block

`state/market.rs` — append at the end of the struct (alignment-1 stays intact; every multi-byte field is an LE byte array):

```rust
    // --- operability + risk depth (VERSION 12; appended, offsets stable) ---
    /// Pause bitflags: bit 0 = PAUSE_INTAKE (submit_order + maker-quote writes
    /// reject with MarketPaused; cancels/cranks/settles/withdrawals/liquidations
    /// keep running so the in-flight round drains and users can exit), bit 1 =
    /// PAUSE_ROLL (start_auction also rejects; the market winds down to a fully
    /// settled quiescent state). There is deliberately NO pause-withdraw bit.
    pub paused: u8,
    /// Minimum order notional (`quantity·price`), 0 = disabled (missing-features §2.6).
    pub min_order_notional_le: [u8; 8],
    /// Per-side open-interest soft cap, 0 = disabled (missing-features §1.2).
    /// Checked at submit against current OI (read-only — Design Z preserved);
    /// same-round races can overshoot by ≤ one round of individually-margined
    /// orders, documented in plan §4.3.
    pub max_open_interest_le: [u8; 16],
    /// Flat liquidation reward floor paid from insurance when the equity-capped
    /// penalty is smaller (missing-features §6.2). 0 = disabled.
    pub liquidation_reward_floor_le: [u8; 8],
    /// Partial-liquidation health buffer in bps above maintenance
    /// (missing-features §6.1). 0 = partial liquidation disabled (full close).
    pub liquidation_close_buffer_bps_le: [u8; 2],
    /// Staged admin change: 0=None 1=RiskParams 2=Oracle 3=Authority.
    pub pending_kind: u8,
    /// Slot at which the staged change may be applied (permissionlessly).
    pub pending_effective_slot_le: [u8; 8],
    /// Kind-specific payload: RiskParams = 4×u16 LE (maintenance, initial,
    /// penalty, close_buffer); Oracle = new oracle Address (32) + new feed id
    /// (32); Authority = new authority Address (32), rest zero.
    pub pending_payload: [u8; 64],
```

Bookkeeping (all in the same file):

- `assert_no_padding!` and `DATA_LEN`: append `+ 1 + 8 + 16 + 8 + 2 + 1 + 8 + 64` (= +108).
- `to_bytes_inner()`: extend with the eight new fields **in struct order**.
- `VERSION: u8 = 12;` with a doc comment in the house style ("append is prefix-compatible for readers but the account is sized by `DATA_LEN` → re-provision, version byte fails a stale account loudly").
- `Market::new(...)`: initialize all-zero (`paused: 0`, `pending_kind: 0`, `pending_payload: [0u8; 64]`, …).
- Accessors:

```rust
    le_field!(min_order_notional, set_min_order_notional, min_order_notional_le, u64);
    le_field!(max_open_interest, set_max_open_interest, max_open_interest_le, u128);
    le_field!(liquidation_reward_floor, set_liquidation_reward_floor, liquidation_reward_floor_le, u64);
    le_field!(pending_effective_slot, set_pending_effective_slot, pending_effective_slot_le, u64);

    #[inline(always)]
    pub fn liquidation_close_buffer_bps(&self) -> u16 {
        u16::from_le_bytes(self.liquidation_close_buffer_bps_le)
    }

    pub const PAUSE_INTAKE: u8 = 1 << 0;
    pub const PAUSE_ROLL: u8 = 1 << 1;

    #[inline(always)]
    pub fn require_not_paused(&self, flag: u8) -> Result<(), ProgramError> {
        if self.paused & flag != 0 {
            return Err(TempoProgramError::MarketPaused.into());
        }
        Ok(())
    }
```

`initialize_market/data.rs`: append four fields to the wire format (`LEN` 131 → **165**):

```rust
    // appended after num_slab_shards (offset 131..):
    let min_order_notional = u64::from_le_bytes(data[131..139].try_into().unwrap());
    let max_open_interest = u128::from_le_bytes(data[139..155].try_into().unwrap());
    let liquidation_reward_floor = u64::from_le_bytes(data[155..163].try_into().unwrap());
    let liquidation_close_buffer_bps = u16::from_le_bytes(data[163..165].try_into().unwrap());
    // validation: buffer only meaningful on a money market, and bounded
    if maintenance_margin_bps == 0 && liquidation_close_buffer_bps != 0 {
        return Err(TempoProgramError::MarketConfigOutOfRange.into());
    }
    if liquidation_close_buffer_bps > 10_000 {
        return Err(TempoProgramError::MarketConfigOutOfRange.into());
    }
```

Pass them through `Market::new` (extend its signature). Update `definition.rs`'s `InitializeMarket` variant with the four new data fields. **Breaking IDL change** — regenerate clients, re-provision devnet once for all of Phase 1–3.

Tests: extend `test_market_roundtrip` (new fields round-trip, version byte == 12), `initialize_market/data.rs` tests for the new bounds.

### 2.2 Pause / halt / resume (missing-features §3.2) — disc 32

New `instructions/set_pause/`:

```rust
// data.rs — LEN = 1
pub struct SetPauseData { pub paused: u8 }
// reject unknown bits:
if data[0] & !(Market::PAUSE_INTAKE | Market::PAUSE_ROLL) != 0 {
    return Err(TempoProgramError::MarketConfigOutOfRange.into());
}

// accounts.rs — [authority (signer), market (writable, program-owned),
//                event_authority, tempo_program]

// processor.rs
pub fn process_set_pause(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let ix = SetPause::try_from((data, accounts))?;
    let market_key = *ix.accounts.market.address();
    {
        let mut acct = *ix.accounts.market;
        let mut md = acct.try_borrow_mut()?;
        // validates PDA + version, then checks the signer is the authority
        {
            let market = Market::from_account(&md, ix.accounts.market, program_id)?;
            market.validate_authority(ix.accounts.authority.address())?;
        }
        Market::from_bytes_mut(&mut md)?.paused = ix.data.paused;
    }
    let event = MarketPauseChangedEvent { market: market_key, paused: ix.data.paused };
    emit_event(program_id, ix.accounts.event_authority, ix.accounts.tempo_program, &event.to_bytes())
}
```

Guards to add (one line each, right after the existing phase read):

- `submit_order/processor.rs` (inside the market-read block, after `let is_collect = ...`): `market.require_not_paused(Market::PAUSE_INTAKE)?;`
- `init_maker_quote/processor.rs` (after `require_phase(Collect)`): same.
- `update_maker_quote_mid/processor.rs` + `update_maker_quote_levels/processor.rs` (in the market-read block): same.
- `start_auction/processor.rs` (after the phase check): `market.require_not_paused(Market::PAUSE_ROLL)?;`

**Deliberately not guarded:** `cancel_order`, `process_chunk`, `finalize_clear`, `settle_fill`, `settle_maker_quote`, `reset_shard`, `deposit`, `withdraw*`, `liquidate*`, `update_funding`, `clear/close_maker_quote` — the in-flight round must drain and users must always be able to exit. A pause can never trap funds.

Tests (LiteSVM `phase_guards.rs` additions): pause mid-`Accumulating` → round drains to `Settling` and rolls; with `PAUSE_ROLL`, `start_auction` rejects and the market parks quiescent; submit/quote-write reject with `Custom(2)`; cancel + withdraw + liquidate still succeed while paused.

### 2.3 Minimum order notional (missing-features §2.6)

`submit_order/processor.rs` — add `min_order_notional` to the market-read tuple, then right after the expiry guard (line ~113):

```rust
    // Anti-dust (missing-features §2.6): bound the order's notional from below.
    // Plain u128 comparison — no division, no rounding question.
    if min_order_notional > 0 {
        let notional = (ix.data.quantity as u128) * (ix.data.price as u128);
        if notional < min_order_notional as u128 {
            return Err(TempoProgramError::OrderBelowMinimum.into());
        }
    }
```

`update_maker_quote_levels/processor.rs` — same idea per level, priced conservatively at the **window floor** (the lowest possible in-window price, so the check is mid-independent and can't be dodged by a later mid move). Add `window_floor` to the market-read block:

```rust
    let (num_ticks, window_floor, min_order_notional) = { /* market read */ };
    ...
    for i in 0..ix.data.num_bids as usize {
        let (_, size) = read_level_size(&ix.data.bid_levels, i);
        if size > 0 && min_order_notional > 0
            && (size as u128) * (window_floor as u128) < min_order_notional as u128
        {
            return Err(TempoProgramError::OrderBelowMinimum.into());
        }
        // (existing offset <= mid check stays)
    }
    // mirror for ask levels
```

Tests: dust order rejected with `Custom(29)`; `min_order_notional == 0` accepts everything (back-compat); maker dust level rejected.

### 2.4 Maker quote-time margin (missing-features §7.1) + multi-quote PDAs (known-issues §4.9) — `MakerQuote` v4

**The top open safety item.** One combined layout change.

#### 2.4.1 `state/maker_quote.rs` changes

```rust
/// Max concurrent quotes per (market, maker) — bounds quote_index (§4.9).
pub const MAX_QUOTES_PER_MAKER: u16 = 4;
```

Struct: append three fields (before the flat regions is NOT allowed — append at the **end**, after `ask_snapshots_le`):

```rust
    /// Which of the maker's concurrent quotes this is (`[0, MAX_QUOTES_PER_MAKER)`);
    /// the 4th PDA seed (known-issues §4.9).
    pub quote_index_le: [u8; 2],
    /// Standing worst-case margin locked in the maker's UserCollateral for this
    /// ladder (missing-features §7.1). Recomputed on every levels write; released
    /// by clear_maker_quote. The ladder is persistent (it re-folds every round at
    /// full size), so the reservation is a STANDING lock, not per-round.
    pub reserved_margin_le: [u8; 8],
    /// Window-top price snapshotted when the reservation was last computed —
    /// mirrors `Order.worst_price` (stable across window recenters, DDR-3).
    pub worst_price_le: [u8; 8],
```

- `assert_no_padding!` / `DATA_LEN`: `+ 2 + 8 + 8`. `VERSION = 4` (house-style doc comment). `to_bytes_inner` extended. `new(...)` takes `quote_index: u16`, zeroes the other two.
- Accessors: `le_field!(reserved_margin, set_reserved_margin, reserved_margin_le, u64);` etc. `quote_index` via inline `u16::from_le_bytes`.
- **Seeds** (this is what unlocks multiple ladders per maker):

```rust
impl PdaSeeds for MakerQuote {
    const PREFIX: &'static [u8] = b"maker_quote";

    fn seeds(&self) -> Vec<&[u8]> {
        vec![Self::PREFIX, self.market.as_ref(), self.maker.as_ref(), &self.quote_index_le]
    }

    fn seeds_with_bump<'a>(&'a self, bump: &'a [u8; 1]) -> Vec<Seed<'a>> {
        vec![
            Seed::from(Self::PREFIX),
            Seed::from(self.market.as_ref()),
            Seed::from(self.maker.as_ref()),
            Seed::from(self.quote_index_le.as_slice()),
            Seed::from(bump.as_slice()),
        ]
    }
}
```

(`init_maker_quote/processor.rs` builds a `[Seed; 5]` array now — change the `try_into` target.)

The market's completeness gate needs **zero changes**: `active_maker_quote_count` already counts *quotes*, and `quote_id` is still claimed per quote from `next_quote_id` (deterministic marginal-tick tie-break across all quotes).

#### 2.4.2 The reservation formula (new pure fn in `margin.rs`)

Mid-independent by design so `update_maker_quote_mid` stays O(1) and collateral-free: every level (both sides) is priced at the window top — a bid buys at ≤ its limit ≤ top; an ask's short-side worst-case in-window mark is the top. Sizes are the only ladder input.

```rust
/// Worst-case standing margin for a maker ladder (missing-features §7.1):
/// `initial_margin(Σ all level sizes, window_top)`. Mid-independent — moving
/// `mid_tick` never changes it, so the O(1) re-quote path stays collateral-free.
/// Rounds UP via initial_margin (never lock less than policy).
pub fn ladder_reservation(total_ladder_qty: u64, window_top_price: u64, initial_bps: u16) -> u64 {
    initial_margin(total_ladder_qty, window_top_price, initial_bps)
}
```

(Trivial wrapper — it exists so the formula has one name, one doc, one test site.)

#### 2.4.3 `init_maker_quote` — new data field

`data.rs`: append `quote_index: u16` (`LEN` 41 → 43); reject `quote_index >= MAX_QUOTES_PER_MAKER` with `MarketConfigOutOfRange`. Pass into `MakerQuote::new`. `definition.rs` variant gains the field + the fourth seed.

#### 2.4.4 `update_maker_quote_levels` — require + adjust the maker's ledger

`accounts.rs`: append two accounts — `user_collateral` (writable, program-owned, **the maker's** mint-scoped ledger) and keep the list fixed-order (no optionality: on a no-money-path market the processor skips the lock, but the account is still passed — one code path, no sentinel logic):

```rust
/// 0. `[signer]` writer — maker or delegate
/// 1. `[]` market
/// 2. `[writable]` maker_quote
/// 3. `[writable]` user_collateral — the MAKER's ledger (locked/released here)
```

`processor.rs` — after the existing ladder-validation and quote guards, before writing the ladder:

```rust
    // --- quote-time margin (missing-features §7.1) ---
    // Reserve the ladder's worst case NOW, so an unbacked ladder can never fold
    // into the histogram and steer the clearing price (price-manipulation +
    // insurance-drain vector). Delta-locked against the previous reservation.
    let (initial_bps, window_top, maintenance_bps) = { /* add to the market-read block:
        market.initial_margin_bps(), market.tick_to_price(market.num_ticks() - 1)?,
        market.maintenance_margin_bps() */ };

    let new_reserve = if maintenance_bps == 0 {
        0 // clearing-benchmark market: no money path, nothing to reserve
    } else {
        let mut total: u64 = 0;
        for i in 0..ix.data.num_bids as usize {
            total = total.saturating_add(level_size(&ix.data.bid_levels, i));
        }
        for i in 0..ix.data.num_asks as usize {
            total = total.saturating_add(level_size(&ix.data.ask_levels, i));
        }
        crate::margin::ladder_reservation(total, window_top, initial_bps)
    };

    let old_reserve = quote.reserved_margin(); // read inside the quote borrow

    if new_reserve != old_reserve {
        let mut uc = *ix.accounts.user_collateral;
        let mut uc_data = uc.try_borrow_mut()?;
        let ledger = UserCollateral::from_bytes_mut(&mut uc_data)?;
        // The ledger is always the MAKER's — a delegate reshapes the ladder
        // against the maker's collateral, never its own (and never moves funds).
        if ledger.owner != quote_maker {
            return Err(TempoProgramError::InvalidOrderOwner.into());
        }
        ledger.validate_self(ix.accounts.user_collateral, program_id)?;
        if new_reserve > old_reserve {
            // Clean pre-trade rejection, same as submit_order §1.1.
            ledger.lock(new_reserve - old_reserve)?;
        } else {
            ledger.release(old_reserve - new_reserve);
        }
    }
    // persist on the quote (inside the quote borrow, with the ladder write):
    quote.set_reserved_margin(new_reserve);
    quote.set_worst_price(window_top);
```

(Note on borrows: read `old_reserve`/`quote_maker` in the first quote borrow, drop it, do the ledger borrow, then re-borrow the quote mutably for the ladder write + reservation fields — the file already uses this read-then-write-borrow pattern.)

#### 2.4.5 `clear_maker_quote` — release the standing reservation

`accounts.rs`: append `user_collateral` (writable). `processor.rs`, inside the quote borrow capture `(reserved, maker)`, zero `quote.set_reserved_margin(0)`, then after the borrow:

```rust
    if reserved > 0 {
        // Reuses the shared release helper — validates the ledger belongs to the
        // MAKER (a reaper/delegate can never redirect margin), saturating release.
        crate::settle_money::release_order_reservation(
            ix.accounts.user_collateral, program_id, &maker, reserved,
        )?;
    }
```

`close_maker_quote`: add a guard `if quote.reserved_margin() != 0 { return Err(TempoProgramError::InvalidOrderStatus.into()); }` — clear (which releases) must run first; close only reclaims rent.

#### 2.4.6 `settle_maker_quote` — never-revert hardening

Two changes in `processor.rs`:

1. Replace the fallible re-lock (current lines 294-296) with the `lock_up_to` pattern `settle_fill` already uses — **today a maker with drained free balance wedges its own quote settle** (the `lock()` reverts, the quote never marks settled, next round's fold overwrites the snapshots and the round-N fills are silently lost). With the standing reservation this becomes a backstop, but it must exist:

```rust
    let current = { /* position.collateral() read, unchanged */ };
    let effective_collateral = if target_margin > current {
        // DDR-3 no-revert: lock what is available; a matched fill can't be
        // un-filled. Any shortfall leaves the position below target for the
        // liquidation backstop instead of wedging the quote settle (and losing
        // the round's fills when the next fold overwrites the snapshots).
        let locked = user_collateral.lock_up_to(target_margin - current);
        current + locked
    } else {
        user_collateral.release(current - target_margin);
        target_margin
    };
```

   …and set `position.set_collateral(effective_collateral)` instead of unconditionally `target_margin` (never over-report margin to the risk gates — same rule as `settle_fill`).

2. Now that quotes are reserved, raise the lock target from maintenance to the **initial** buffer, matching the taker path (the current maintenance-only compromise existed precisely because there was no reservation — see the in-code comment at lines 238-243): `let target_margin = initial_margin(new_abs_size, new_entry, initial_bps);` (add `initial_bps` to the market-read tuple).

#### 2.4.7 Off-chain lockstep (same PR)

- `crates/sdk/src/pda.rs`: maker-quote PDA takes `quote_index`; ix builders add the ledger account + `quote_index`.
- `crates/mm-bot`: deposit collateral before quoting; treat `InsufficientCollateral` on a levels write as "shrink the ladder", not an error; use `quote_index 0` by default.
- Migration: makers `clear_maker_quote` + `close_maker_quote` old v3 quotes (rent refunded), re-init at the new addresses. No on-chain migration instruction — the close/re-init path already exists.

#### 2.4.8 Tests

- Unit: `ladder_reservation` (rounds up; zero ladder → 0), v4 roundtrip (`quote_index`, `reserved_margin`, `worst_price`).
- LiteSVM (`maker_margin.rs`): unbacked ladder rejected at levels-write (`Custom(24)`); shrink releases exactly; clear releases all; close blocked until cleared; drained-maker settle no longer reverts (lock_up_to) and position under-margin is caught by `liquidate`; **re-run `two_makers_share_marginal_tick_and_conserve_oi` under reservations**; two concurrent quotes (index 0/1) by one maker fold + settle independently.
- Adversarial: delegate cannot point a foreign ledger (owner check); reservation unchanged by 1,000 `update_maker_quote_mid` calls (mid-independence).

---

## 3. Phase 2 — The admin release (one `Vault` bump; no `Market` bump — fields already exist)

### 3.1 The staged-change engine (shared by §3.2/§3.3, Phase-3 §4.4)

One tiny helper on `Market` (state file):

```rust
    pub const PENDING_NONE: u8 = 0;
    pub const PENDING_RISK_PARAMS: u8 = 1;
    pub const PENDING_ORACLE: u8 = 2;
    pub const PENDING_AUTHORITY: u8 = 3;

    /// Stage a change: kind + payload + the slot it becomes applicable.
    pub fn stage_pending(&mut self, kind: u8, payload: &[u8], effective_slot: u64) {
        self.pending_kind = kind;
        self.pending_payload = [0u8; 64];
        self.pending_payload[..payload.len()].copy_from_slice(payload);
        self.set_pending_effective_slot(effective_slot);
    }

    /// Take a staged change of `kind` if its delay has elapsed; clears the slot.
    pub fn take_pending(&mut self, kind: u8, now_slot: u64) -> Result<[u8; 64], ProgramError> {
        if self.pending_kind != kind {
            return Err(TempoProgramError::NoPendingUpdate.into());
        }
        if now_slot < self.pending_effective_slot() {
            return Err(TempoProgramError::PendingDelayNotElapsed.into());
        }
        let payload = self.pending_payload;
        self.pending_kind = Self::PENDING_NONE;
        self.pending_payload = [0u8; 64];
        self.set_pending_effective_slot(0);
        payload
    }
```

Delay constant (per kind, protocol-level): `pub const RISK_UPDATE_DELAY_SLOTS: u64 = 3_000;` (~20 min on devnet — tune later via this same mechanism if ever needed). `propose_*` is authority-gated; **`apply_*` is permissionless** — the delay is enforced by consensus, and anyone can complete a staged change (crank philosophy).

### 3.2 `update_market_params` (hot set, immediate) — disc 33, + staged risk params — discs 34/35

Hot set = parameters that are read at use-time and cannot strand in-flight state: `maker_fee_bps, taker_fee_bps, integrator_share_bps, crank_fee, max_price_move_bps_per_slot, soft_stale_slots, max_position_notional, min_order_notional, max_open_interest, liquidation_reward_floor`.

```rust
// update_market_params/data.rs — LEN = 2+2+2+8+2+8+16+8+16+8 = 72, all LE
pub struct UpdateMarketParamsData {
    pub maker_fee_bps: i16,
    pub taker_fee_bps: i16,
    pub integrator_share_bps: u16,
    pub crank_fee: u64,
    pub max_price_move_bps_per_slot: u16,
    pub soft_stale_slots: u64,
    pub max_position_notional: u128,
    pub min_order_notional: u64,
    pub max_open_interest: u128,
    pub liquidation_reward_floor: u64,
}
```

**Extract the bounds** from `initialize_market/data.rs` into shared `pub fn validate_fee_config(...)` / `validate_brake_config(...)` in that same module and call them from **both** parsers — single source of truth, the two paths can't drift. Processor: authority check → write the ten fields → emit `MarketParamsUpdated`.

Staged risk params (the set that can make live positions liquidatable):

```rust
// propose_risk_update/data.rs — LEN = 8: 4×u16
// payload = [maintenance, initial, penalty, close_buffer] each u16 LE
// processor: authority check; re-validate with the SAME shared bounds fns
// (maintenance ∈ (0,5000], maintenance ≤ initial ≤ 10000, penalty ≤ 5000,
//  buffer ≤ 10000 — reuse initialize_market's validation exactly);
// market.stage_pending(PENDING_RISK_PARAMS, &payload, now_slot + RISK_UPDATE_DELAY_SLOTS)

// apply_risk_update/processor.rs (permissionless — cranker signer only):
let payload = market.take_pending(Market::PENDING_RISK_PARAMS, now_slot)?;
market.maintenance_margin_bps_le = payload[0..2].try_into().unwrap();
market.initial_margin_bps_le    = payload[2..4].try_into().unwrap();
market.liquidation_penalty_bps_le = payload[4..6].try_into().unwrap();
market.liquidation_close_buffer_bps_le = payload[6..8].try_into().unwrap();
// emit MarketParamsUpdated { kind: PENDING_RISK_PARAMS, payload }
```

**Never changeable** (enforced by simply not existing in any update instruction): `tick_size, num_ticks, num_slab_shards, orders_per_auction_cap, collateral_mint`.

Tests: fee change applies immediately; risk propose→apply before delay → `Custom(48)`; apply wrong kind → `Custom(47)`; bounds enforced identically to init (property: same rejection table).

### 3.3 Authority transfer (discs 36/37) and set-oracle (discs 38/39)

Authority (two-step, no delay — the accept signature IS the safety):

```rust
// propose_authority_transfer: authority signs; stage_pending(PENDING_AUTHORITY,
//   new_authority.as_ref(), now_slot)  // effective immediately, kind-gated
// accept_authority_transfer: accounts [new_authority (signer), market (writable)]
let payload = market.take_pending(Market::PENDING_AUTHORITY, now_slot)?;
let staged = Address::new_from_array(payload[0..32].try_into().unwrap());
if staged != *ix.accounts.new_authority.address() {
    return Err(TempoProgramError::InvalidAuthority.into());
}
let old = market.authority;
market.authority = staged;
// emit AuthorityTransferred { market, old_authority: old, new_authority: staged }
```

Set-oracle (the most dangerous admin power — constrain hard):

```rust
// propose_set_oracle: authority; payload = new_oracle(32) ++ new_feed_id(32);
//   requires market.paused & PAUSE_ROLL != 0 (must already be winding down);
//   stage_pending(PENDING_ORACLE, payload, now_slot + RISK_UPDATE_DELAY_SLOTS)

// apply_set_oracle: permissionless. accounts: [cranker(s), market(w), new_oracle]
// Gates, in order:
// 1. delay elapsed + kind matches (take_pending)
// 2. still paused (PAUSE_INTAKE|PAUSE_ROLL) and QUIESCENT:
//    phase ∈ {Settling, Discovered} with shards_ready == num_slab_shards,
//    else MarketNotQuiescent — no round may straddle two price regimes
// 3. the new account is LIVE right now:
if ix.accounts.new_oracle.address() != &staged_oracle {
    return Err(TempoProgramError::AccountMarketMismatch.into());
}
if !ix.accounts.new_oracle.owned_by(&PYTH_RECEIVER_ID) {
    return Err(TempoProgramError::OracleInvalidAccount.into());
}
let od = ix.accounts.new_oracle.try_borrow()?;
let price = read_price(&od, &staged_feed_id, now_ts, MAX_AGE_SECS)?; // fresh or fail
price.require_confidence(DEFAULT_MAX_CONF_BPS)?;
// 4. commit atomically (address + feed id are checked as a pair by every reader):
market.oracle = staged_oracle;
market.oracle_feed_id = staged_feed_id;
market.set_last_good_oracle_slot(now_slot);
// window re-anchors on the new feed at the next start_auction (recenter_window)
```

Tests: apply blocked unpaused / un-quiescent / early / on a stale target feed; funding + liquidation read the new feed after apply; the window recenters on the new feed at the next roll.

### 3.4 `Vault` v3: authority + user-balance aggregate + on-chain invariant (missing-features §4.2)

`state/vault.rs` — append:

```rust
    /// Vault admin (recorded at init — v2 had NO stored authority, so
    /// insurance-withdraw had no one to gate on; see plan §3.5/§4.4).
    pub authority: Address,
    /// Running Σ of every UserCollateral.balance under this mint (u128). The
    /// conservation-counter exception to the "scans not counters" rule: there is
    /// no on-chain scan alternative, and drift is caught FAIL-CLOSED at the
    /// token-outflow sites (blocks withdrawals, never wedges rounds) — the
    /// opposite failure mode of the liveness counters Design Z removed.
    pub total_user_balance_le: [u8; 16],
    /// Staged insurance withdraw (plan §4.4): amount + effective slot.
    pub pending_withdraw_amount_le: [u8; 8],
    pub pending_withdraw_slot_le: [u8; 8],
```

`DATA_LEN`/`assert_no_padding!` `+ 32 + 16 + 8 + 8`; `VERSION = 3`; `new(...)` takes `authority`; `init_vault` records its existing `admin` signer there (finally used). Accessors via `le_field!`.

**Every balance-changing site updates the aggregate.** The full site list (verified — `lock`/`release`/`lock_up_to` move `locked` only and are exempt): `deposit` (credit), `withdraw`/`withdraw_cross` (debit/set_balance), `settle_fill` + `settle_maker_quote` (`apply_pnl`, integrator credit), `liquidate` + `liquidate_cross` (release+apply_pnl+credits), `finalize_clear` (crank-fee credit). Every one of those **already has the vault account in scope** at the moment balance changes (settle paths require it exactly when `balance_delta != 0`). Add one helper to `settle_money.rs`:

```rust
/// Mirror a user-balance change into the vault's aggregate (plan §3.4).
/// Saturating add / checked sub: an over-subtract means drift — surface it as
/// the invariant error rather than wrapping.
pub fn apply_user_balance_delta(vault: &mut Vault, delta: i128) -> Result<(), ProgramError> {
    let cur = vault.total_user_balance();
    let new = if delta >= 0 {
        cur.checked_add(delta as u128).ok_or(TempoProgramError::MathOverflow)?
    } else {
        cur.checked_sub((-delta) as u128)
            .ok_or(TempoProgramError::VaultInvariantViolated)?
    };
    vault.set_total_user_balance(new);
    Ok(())
}
```

…and call it beside every balance mutation (in `conserve_and_socialize` the delta is already computed; in `liquidate` it is `returned_to_owner + penalty − locked_release`-shaped — compute `after − before` per ledger touched, which each site already has).

**The fail-closed gate**, at the three token-outflow sites (`withdraw`, `withdraw_cross`, `apply_insurance_withdraw`), where the vault token account is already passed:

```rust
    // Fail-closed backing check (missing-features §4.2): tokens may only leave
    // while the vault still covers every user balance + insurance AFTER the op.
    let vault_token = TokenAccount::from_account_view(ix.accounts.vault_token_account)?;
    let backing_needed = vault
        .total_user_balance()
        .saturating_add(vault.insurance_balance() as u128);
    if (vault_token.amount() as u128) < backing_needed {
        return Err(TempoProgramError::VaultInvariantViolated.into());
    }
```

(Placed **after** the ledger debit + aggregate update, **before** the SPL transfer.)

Tests: property test over the whole existing money-path LiteSVM suite — after every instruction, `vault.total_user_balance == Σ scanned ledger balances` (the host can scan); outflow gate blocks when the aggregate is manually corrupted; `apply_pnl` losses (bad debt) reduce the aggregate by exactly the covered slice.

### 3.5 `seed_insurance` (permissionless) — disc 40

```rust
// accounts: [donor (signer), vault (writable), vault_token_account (writable),
//            donor_token_account (writable), token_program (pinned SPL, HS-12),
//            event_authority, tempo_program]
// data: amount u64 (reject 0)
pub fn process_seed_insurance(...) -> ProgramResult {
    // vault.vault_token_account binding check (same as deposit)
    // SPL Transfer donor_token_account -> vault_token_account, donor-signed
    // then, borrows re-taken:
    vault.set_insurance_balance(
        vault.insurance_balance().checked_add(amount).ok_or(MathOverflow)?,
    );
    // NOTE: total_user_balance is NOT touched — this is pool money, not a user
    // claim. Both sides of `vault_token ≥ Σ balances + insurance` grow together.
    // emit InsuranceSeeded { mint, donor, amount }
}
```

Face-amount crediting is valid for the same reason as `deposit` (token program pinned, no fee-on-transfer). Test: fresh market + seeded pool → first maker rebate no longer clamps to zero.

---

## 4. Phase 3 — The risk-depth release (no layout changes — fields exist since Phase 1/2)

### 4.1 Partial liquidation (missing-features §6.1)

#### 4.1.1 New pure math in `margin.rs`

Derivation (integer-only, one `mul_div_ceil`): closing `c` units at `mark` leaves equity unchanged (the realized slice equals the removed unrealized slice) minus the penalty on the closed notional; maintenance shrinks linearly. Solve smallest `c` with buffer `B = 10_000 + buffer_bps`:

```
E − c·mark·pen/1e4  ≥  (|s|−c)·mark·maint/1e4 · B/1e4
⇒ c ≥ (|s|·mark·maint·B − E·1e8) / (mark·(maint·B − pen·1e4))
```

```rust
/// Minimum quantity to close at `mark` so the position returns to health with a
/// `buffer_bps` cushion above maintenance (missing-features §6.1). Returns
/// `None` ⇒ full close (equity ≤ 0, degenerate config, or c ≥ |size|).
/// Rounds UP (closes slightly more — against the position holder, consistent
/// with the house rounding direction). Pure, division via mul_div_ceil only.
pub fn partial_close_qty(
    abs_size: u128,
    equity: i128,
    mark: u64,
    maintenance_bps: u16,
    penalty_bps: u16,
    buffer_bps: u16,
) -> Option<u64> {
    if equity <= 0 || buffer_bps == 0 {
        return None; // insolvent or feature disabled ⇒ full close
    }
    let b = 10_000u128 + buffer_bps as u128;          // buffered multiplier
    let maint_b = (maintenance_bps as u128) * b;       // maint·B      (≤ 5e3·2e4)
    let pen_scaled = (penalty_bps as u128) * 10_000;   // pen·1e4
    if maint_b <= pen_scaled {
        return None; // penalty eats the health gain ⇒ partial can't converge
    }
    // num = |s|·mark·maint·B − E·1e8   (256-bit safe via wide_math)
    let need = crate::wide_math::mul_div_floor(abs_size * (mark as u128), maint_b, 1)?;
    let have = (equity as u128).checked_mul(100_000_000)?;
    if have >= need {
        return Some(0); // already healthy at the buffered line
    }
    let den = (mark as u128).checked_mul(maint_b - pen_scaled)?;
    let c = crate::wide_math::mul_div_ceil(need - have, 1, den)?;
    if c >= abs_size {
        return None; // remainder wouldn't restore health ⇒ full close
    }
    u64::try_from(c).ok().map(Some)?
}
```

(Exact overflow envelope: `|s|·mark·maint·B ≤ 2^127` holds comfortably inside the same `2^48` operand envelope `proof_unrealized_pnl_no_overflow_in_envelope` pins; add a matching Kani harness — the formula is one multiply-divide, CBMC-tractable in the ceil form used by `socialize_bad_debt`.)

#### 4.1.2 `liquidate/processor.rs` changes

After the `is_liquidatable` gate (line 124), decide the close size; the full-close path is byte-for-byte today's code:

```rust
    let abs_size = size_signed.unsigned_abs();
    let buffer_bps = /* add to market-read tuple */ liquidation_close_buffer_bps;
    let close_qty = crate::margin::partial_close_qty(
        abs_size, outcome.equity, mark, maintenance_bps, penalty_bps, buffer_bps,
    );
    // Fall back to full close when: disabled, insolvent, degenerate, or the
    // remainder would violate the min-notional dust floor (plan §2.3).
    let close_qty = match close_qty {
        Some(c)
            if c > 0
                && (abs_size - c as u128) * (mark as u128) >= min_order_notional as u128 =>
        {
            c
        }
        _ => u64::try_from(abs_size).unwrap_or(u64::MAX), // full close (today's path)
    };
```

Partial branch (when `close_qty < abs_size`):

- Realize the closed slice into the position instead of zeroing it: `position.apply_fill(size_signed < 0, close_qty, mark, social_long, social_short)?` — an opposing fill at `mark` realizes exactly `(mark − entry)·closed` and shrinks `|size|`; `apply_fill` already handles VWAP/realized bookkeeping and can never flip here because `close_qty < |size|`.
- Penalty on the **closed** notional only: `penalty = mul_div_floor(close_qty·mark, penalty_bps, 10_000).min(equity_u)`.
- Collateral: release/return only the freed share — keep `target = initial_margin(remaining_abs, entry, initial_bps)`, release `collateral.saturating_sub(target)` from the ledger lock, leave the rest locked on the position. Owner ledger flow mirrors today's but with the partial amounts; insurance delta stays `released_slice − returned − penalty` (conservation identical in shape).
- OI: `market.apply_oi_delta(size_signed, new_signed)` (new signed size, not 0).
- **Progress guarantee** (wire the reserved error): recompute `maintenance_deficit` after; a correct `partial_close_qty` makes it 0, so this is a belt-and-suspenders assert:

```rust
    let deficit_after = crate::margin::maintenance_deficit(equity_after, maint_after);
    if deficit_after > 0 && close_qty < abs_size_u64 {
        return Err(TempoProgramError::LiquidationNoProgress.into());
    }
```

- Event: emit with the appended `closed_qty` + `remaining_size` fields.

`liquidate_cross`: same formula, with `combined_equity`/`combined_maintenance` as `E`/the maintenance term and the target leg's size — the leg closes partially, combined health restored, everything else unchanged.

#### 4.1.3 Tests

- `margin.rs` unit + 20k-iter fuzz: post-close `equity' ≥ maint'·(1+buf)`; `c` minimal (c−1 fails); conservation `returned + penalty + insurance_delta == released_slice`; full-close fallbacks (equity ≤ 0, `maint_b ≤ pen_scaled`, dust remainder).
- Kani: panic-freedom of `partial_close_qty` in the `2^48` envelope.
- LiteSVM (`partial_liquidation.rs`): 1%-underwater whale loses only a slice; repeated calls converge (≤ 2 rounds with buffer); crash scenario (big mark gap) still full-closes; `LiquidationNoProgress` unreachable under valid config (assert via fuzz harness, not skipped).

### 4.2 Liquidation reward floor (missing-features §6.2)

In `liquidate/processor.rs` (and `liquidate_cross`), after the penalty credit — exactly the `finalize_clear` crank-fee shape (verified pattern at `finalize_clear/processor.rs:248-280`):

```rust
    // Keeper-reward floor (missing-features §6.2): top the equity-capped penalty
    // up to `liquidation_reward_floor` FROM INSURANCE, capped at what insurance
    // has (conserving, fail-soft). Griefing-safe: a liquidation only executes
    // when equity < maintenance — an on-chain condition an attacker cannot
    // manufacture for free — so every floor payment buys work the protocol wanted.
    let floor = liquidation_reward_floor; // from the market-read tuple
    if floor > outcome.penalty {
        let top_up = {
            // vault borrow (already in scope in this section)
            let pay = (floor - outcome.penalty).min(vault.insurance_balance());
            vault.set_insurance_balance(vault.insurance_balance() - pay);
            pay
        };
        if top_up > 0 {
            // credit liquidator ledger + apply_user_balance_delta(vault, top_up)
        }
    }
```

**No per-call fees for `process_chunk`/`settle_fill`** (farmable by call-splitting — see analysis). Optional cheap extension, same PR or later: pay `crank_fee / num_slab_shards` per `reset_shard` and `crank_fee` at `start_auction` via the optional trailing `cranker_collateral + vault` accounts pattern — both intrinsically once-per-round, unfarmable.

Tests: penalty < floor → topped up; capped at insurance; conservation (`Σ balances + insurance` unchanged by the top-up transfer); floor 0 = disabled (back-compat).

### 4.3 Soft open-interest cap (missing-features §1.2 remainder)

`submit_order/processor.rs` — add `oi_long`/`oi_short`/`max_open_interest` to the market-read tuple (still **read-only** — Design Z preserved), then inside the money-path block after the notional cap (line ~225):

```rust
        // Per-side OI soft cap (missing-features §1.2). Read-only check: races
        // inside one round can overshoot by at most one round of orders, each
        // individually margin-reserved and notional-capped — the cap is a
        // risk-SIZING rail, not a solvency gate (solvency is margin's job).
        // A reducing order is never blocked (new_exposure_abs == 0 on a pure
        // reduce), so de-risking always works even over the cap.
        if max_open_interest > 0 && new_exposure_abs > 0 {
            let side_oi = if is_buy { oi_long } else { oi_short };
            if side_oi.saturating_add(new_exposure_abs) > max_open_interest {
                return Err(TempoProgramError::OpenInterestCapExceeded.into());
            }
        }
```

(`new_exposure_abs` is already computed for the notional cap — reuse it; hoist it out of the `max_position_notional > 0` branch.)

Tests: submit over cap rejected `Custom(50)`; reduce always passes; two same-round submits both under cap individually → both accepted (documented overshoot), next round's submit rejected.

### 4.4 Insurance withdraw (missing-features §4.1, staged) — discs 41/42

```rust
// propose_insurance_withdraw: [authority (signer, == vault.authority), vault (w)]
// data: amount u64. Guards: amount != 0, amount <= vault.insurance_balance().
vault.set_pending_withdraw_amount(amount);
vault.set_pending_withdraw_slot(now_slot + RISK_UPDATE_DELAY_SLOTS);

// apply_insurance_withdraw (permissionless): [cranker (s), vault (w),
//   vault_authority, vault_token_account (w), recipient_token_account (w),
//   token_program (pinned), event_authority, tempo_program]
let amount = vault.pending_withdraw_amount();
if amount == 0 { return Err(TempoProgramError::NoPendingUpdate.into()); }
if now_slot < vault.pending_withdraw_slot() {
    return Err(TempoProgramError::PendingDelayNotElapsed.into());
}
// recipient mint check (HS-12), insurance re-check (may have shrunk since propose):
let pay = amount.min(vault.insurance_balance());
vault.set_insurance_balance(vault.insurance_balance() - pay);
vault.set_pending_withdraw_amount(0);
// FAIL-CLOSED backing gate (plan §3.4) — after the insurance debit, before transfer
// then vault_authority-signed SPL Transfer of `pay`
// emit InsuranceWithdrawn { mint, authority: vault.authority, amount: pay }
```

The delay is non-negotiable: a compromised authority draining insurance instantly is the priced-in scenario; the delay gives users one window to exit. Recipient is unconstrained (authority's choice) — the gate protects *users'* backing, not the pool's destination.

Tests: propose→apply happy path; early apply `Custom(48)`; backing gate blocks when Σ balances + insurance-after would exceed vault tokens; `pay` clamps to shrunk insurance.

### 4.5 Mark-price honesty renames (missing-features §5.2)

No behavior change: rename local `mark` → `funding_mark` in `update_funding/read_oracle` processors and → `solvency_price` in `liquidate*/withdraw_cross`; `OraclePriceReadEvent` keeps its layout (fields already carry both `oracle_price_1e8` and `mark_price`). Update `docs/risk-model.md` with a "two prices, two names, two reasons" section lifted from the analysis. Revisit true unification only after §7.1 has soaked and a manipulation simulation exists.

---

## 5. Phase 4 — The trading-UX release

### 5.1 IOC orders (missing-features §2.3) — a one-comparison program change

`submit_order/processor.rs` line ~111 — the current guard rejects `expires <= auction_id`, which forces a minimum two-round lifetime (fill this round + rest one round). Allow expiry **at the arm round**:

```rust
    // IOC (missing-features §2.3): `expires_at_auction == arm_auction_id` is now
    // legal — the order participates in exactly ONE auction (its arm round) and
    // settle consumes any remainder there (`expires <= auction_id` at settle).
    // Still rejected: an expiry strictly before the round it would first fold in
    // (dead margin the reaper would have to collect). The cancel-reaper boundary
    // (strict `<`) is untouched: an IOC is reapable from the round after its arm.
    if ix.data.expires_at_auction != 0 && ix.data.expires_at_auction < arm_auction_id {
        return Err(TempoProgramError::OrderAlreadyExpired.into());
    }
```

No settle change needed — `settle_fill`'s existing `expired = expires_at_auction != 0 && expires_at_auction <= auction_id` already consumes it at the arm round. Update the doc comments in `submit_order/data.rs` + the `definition.rs` field docs.

SDK sugar (no program change): `submitIoc(side, price, qty)` sets `expires = armRound(marketPhase)`; **`submitMarketOrder(side, qty)`** sets `price = side == Buy ? windowTopPrice : windowFloorPrice` (both derivable from `Market` fields) + optionally IOC; **`closePosition(pct)`** composes `submitMarketOrder(opposite(position.sign), qty, reduce_only = true)` (missing-features §2.1 — the program already supports every ingredient; the auction *is* the venue, and there is deliberately no close-against-vault-at-oracle path).

Tests: IOC fills what crosses and consumes the rest in one round (never rests); IOC that misses the cross consumes with full margin release; mid-round IOC arms and expires at `current+1`.

**FOK / post-only: explicitly not built** — add a short "order types that don't map to uniform-price batch rationing" note to `docs/system-design.md` (FOK breaks the telescoping-floor conservation; post-only *is* the maker-quote book).

### 5.2 `cancel_all_orders` (missing-features §2.7) — disc 43

Scan-my-orders variant (no id list — self-limiting via the 8-per-trader-per-shard cap):

```rust
// accounts: mirror cancel_order exactly: [trader (signer), market (read-only),
//   order_slab (writable), event_authority, tempo_program,
//   user_collateral? (optional, writable — required iff any released margin > 0)]
// data: empty (LEN = 0)
pub fn process_cancel_all_orders(...) -> ProgramResult {
    // market read: phase()? validated, auction_id captured (any phase — same as cancel_order)
    let mut total_release = 0u64;
    let mut cancelled = 0u32;
    {
        let mut slab_data = /* order_slab borrow_mut */;
        // header market/PDA checks identical to cancel_order
        for slot in 0..capacity {
            let order = read_order(&slab_data, capacity, slot)?;
            if order.status != OrderStatus::Resting as u8 { continue; }
            if order.trader != trader { continue; }        // owner path only —
            // reaping strangers' expired orders stays on single cancel_order
            total_release = total_release
                .checked_add(order.reserved_margin)
                .ok_or(TempoProgramError::MathOverflow)?;
            write_order(&mut slab_data, capacity, slot, &Order::empty())?;
            cancelled += 1;
            // collect (order_id) for the per-order events emitted below
        }
        // header: count -= cancelled, resting_count -= cancelled (checked_sub)
    }
    if total_release > 0 {
        let uc = ix.accounts.user_collateral.ok_or(MissingSettleAccounts)?;
        settle_money::release_order_reservation(uc, program_id, &trader, total_release)?;
    }
    // one OrderCancelledEvent per cancelled order (indexers already decode it)
}
```

One shard per transaction (multi-shard cancel-all is a client loop over shards — bounded and parallel). CU: 90-slot scan + ≤ 8 cancels, trivial. Tests: cancels only mine, only `Resting`; single summed release; zero-order call is a no-op success.

---

## 6. Phase 5 — Benchmark-gated & polish

### 6.1 The round-latency benchmark (known-issues §2.14 + §2.15 — run FIRST, build maybe)

Extend `tests/integration-tests/tests/benchmark.rs` + a devnet run in `crates/sim`:

- Measure, at 16 shards × 90 orders with a live keeper: wall-clock per phase (collect window, accumulate, finalize, settle-all, reset-all, roll) over ≥ 50 rounds.
- **Decision rule:** if `settle-all + reset-all` (the serial tail C2 would overlap) < the collect window, **C2 is dead — close §2.14 as "measured, not needed"**. Otherwise implement per `docs/design-decisions.md`-to-be (double-buffered `[b"histogram", market, parity]` + `[b"clearing", market, parity]`, parity = `auction_id & 1` as a 1-byte seed; `arm_auction_id` + per-order status already disambiguate rounds — full design in the analysis doc, Issue 4).
- Keeper early-roll (§2.15) ships regardless (pure `crates/keeper` change): treat `shards_ready == num_slab_shards` as the roll trigger and pipeline `reset_shard` calls with the settle tail.

### 6.2 Structured `benign()` (known-issues §4.10) — SDK only

`crates/sdk/src/retry.rs`: parse the typed `TransactionError::InstructionError(_, InstructionError::Custom(code))` from the RPC response instead of substring-matching. Benign allowlist as an explicit const:

```rust
/// Crank races that mean "someone else did the work first" — retry/skip, not error.
const BENIGN_CODES: &[u32] = &[
    5,  // OrderNotFound        — settled/cancelled first
    17, // InvalidOrderStatus   — raced to a later status
    3,  // AuctionWrongPhase    — phase advanced under us
    16, // AuctionIdMismatch    — round rolled under us
    9,  // AuctionNotComplete   — crank raced the gate
    25, // NotLiquidatable      — liquidation raced
];
```

Keep the string matcher only for code-less transport errors (blockhash expiry, node behind); keep the existing format-drift regression test on that fallback.

### 6.3 EMA for funding (missing-features §5.1)

`oracle.rs` — the `PriceUpdateV2` byte layout already documented in the header comment puts `prev_publish_time` at `base+60`, `ema_price` at `base+68`, `ema_conf` at `base+76`; `MIN_LEN = 134` already covers `base+84`. Extend the reader:

```rust
    // in read_price, after publish_time:
    let ema_price = i64::from_le_bytes(data[base + 68..base + 76].try_into().unwrap());
    // OraclePrice gains: pub ema_price_1e8: u64  (0 when ema <= 0 — fall back to spot)
    let ema_price_1e8 = if ema_price > 0 { normalize_1e8(ema_price, exponent)? } else { price_1e8 };
```

`update_funding/processor.rs` line ~72+: the **index side** of the gap becomes the EMA; the band anchor stays spot (band = manipulation rail, EMA = noise rail):

```rust
    let mark = compute_mark_price(last_bid, last_ask, price.price_1e8, MARK_BAND_BPS)?;
    let rate = period_funding_rate(mark, price.ema_price_1e8, period_fraction_bps, MAX_FUNDING_RATE)?;
```

**Solvency stays on raw spot — do not touch** (`solvency_mark` unchanged; a lagging EMA in a crash is the §2.2 anti-liquidation bug reborn). Update `tempo-math::oracle` mirror + regenerate its goldens in the same PR. Tests: synthetic account with divergent ema/spot → funding uses ema, liquidation uses spot.

### 6.4 `close_position` (disc 44) + `close_market` (disc 45) (missing-features §3.4)

```rust
// close_position: [owner (signer, writable — rent recipient), position (writable)]
// Guards: owner match, validate_self, size == 0 && collateral == 0 &&
// realized_pnl == 0 (flat and drained), margin_mode == 0 (not in a cross group).
// Then close_pda_account(position, owner).

// close_market: [authority (signer, writable), market (w), histogram (w),
//   clearing_result (w), ..shards (variadic, w — ALL, force_reset-style count +
//   dedup mask)]
// Gates: validate_authority; paused == PAUSE_INTAKE|PAUSE_ROLL; quiescent
// (shards_ready == num_slab_shards, phase Settling/Discovered); oi_long == 0 &&
// oi_short == 0; active_maker_quote_count == 0; every shard count() == 0
// → else MarketNotQuiescent. Then close_pda_account on every shard, histogram,
// clearing_result, and finally the market itself, rent → authority.
```

Getting to zero OI is operational (pause intake, let funding + liquidation + user closes drain it — raise maintenance within bounds via §3.2 if needed); there is deliberately **no force-close-at-oracle**.

---

## 7. Cross-cutting test matrix (added across phases)

| Suite | New coverage |
|---|---|
| `clearing.rs` unit/fuzz | unchanged — **must stay green untouched** (nothing in this plan modifies clearing math) |
| `margin.rs` unit/fuzz | `ladder_reservation`, `partial_close_qty` (20k fuzz: minimality + conservation + health) |
| Kani | `proof_partial_close_qty_safe` (panic-freedom, `2^48` envelope) |
| State serde | Market v12, MakerQuote v4, Vault v3 roundtrips + version-byte pins |
| `pretrade_safety.rs` | min-notional, OI cap (incl. same-round overshoot), IOC arm/expiry boundaries |
| new `maker_margin.rs` | §2.4.8 list (unbacked-ladder rejection through mid-independence) |
| new `admin_lifecycle.rs` | pause matrix, staged risk/oracle/authority flows, delay/kind errors |
| new `treasury.rs` | seed/withdraw, aggregate == scanned Σ property, outflow gate |
| new `partial_liquidation.rs` | §4.1.3 list |
| new `marketable_fill.rs` | Phase 0 §1.4 (recenter × live counterparty) |
| `crates/sim` | devnet scripts for marketable-fill, pause-drain, oracle-repoint drills |

Adversarial constants throughout: every new gate tested with a hostile cranker ordering (pause mid-crank, apply-before-propose, double-apply, foreign ledger substitution on the maker path).

---

## 8. Rollout order & re-provision checklist

```
Phase 0  (no deploy)      : 1.1 bundle+CI → 1.2 docs → 1.3 SDK ledger → 1.4 marketable-fill test
Phase 1  (deploy + re-provision #1)
         : Market v12 block + init data 165B          (§2.1)
         : SetPause(32) + guards                       (§2.2)
         : min-notional checks                         (§2.3)
         : MakerQuote v4 + quote margin + multi-quote  (§2.4)  ← mm-bot in same PR
Phase 2  (deploy + re-provision #2 — vaults only)
         : staged-change engine                        (§3.1)
         : UpdateMarketParams(33) + Risk(34/35)        (§3.2)
         : Authority(36/37) + SetOracle(38/39)         (§3.3)
         : Vault v3 + aggregate + outflow gate         (§3.4)
         : SeedInsurance(40)                           (§3.5)
Phase 3  (deploy, no re-provision)
         : partial liquidation                         (§4.1)
         : reward floor                                (§4.2)
         : OI soft cap                                 (§4.3)
         : InsuranceWithdraw(41/42)                    (§4.4)
         : mark renames/docs                           (§4.5)
Phase 4  (deploy, no re-provision)
         : IOC guard change + SDK market/close sugar   (§5.1)
         : CancelAllOrders(43)                         (§5.2)
Phase 5  (benchmark first)
         : round-latency benchmark → C2 go/no-go       (§6.1)
         : keeper early-roll                           (§6.1)
         : structured benign()                         (§6.2)
         : EMA funding                                 (§6.3)
         : ClosePosition(44) + CloseMarket(45)         (§6.4)
```

Per-deploy checklist (every phase): `cargo fmt` → `cargo clippy --all-targets -- -D warnings` → `cargo test --features idl` → `cargo-build-sbf` → `just generate-clients` (commit `idl/` + `clients/` + `crates/sdk/src/generated/`) → `just integration-test` → deploy to devnet → re-provision if the phase bumps a layout → run the `crates/sim` drill for the phase's features → update `docs/known-issues.md` / `docs/missing-features.md` status tables.

### What this plan never touches (the preserved core)

`clearing.rs` (`find_cross` / `fill_against_cross` / `compute_marginal_fill`), the histogram fold path, the per-shard completeness scans, `Market`-read-only submit/cancel, the never-revert settle contract (it is *extended* to makers, §2.4.6), raw-oracle solvency pricing, and the `Order`/`OrderSlab`/`Position`/`ClearingResult` layouts. Any diff that touches these files beyond the specific lines named above is out of scope and should be treated as a red flag in review.

---

## 9. TODO list (designed for `/goal`)

### 9.0 Working protocol — read before starting any task

This section is written so a `/goal` run can drive the plan to completion. The `/goal`
evaluator is a small model that judges **only what appears in the conversation** — it
cannot run commands or read files itself. Therefore:

1. **Work one task at a time**, in order within a phase (a task may depend on the one
   above it). Read the referenced plan section (§) before touching code.
2. A task is done ONLY when its **"Done when"** command(s) have been **run and their
   output shown in the conversation** (paste the relevant lines — the test names and
   the pass/fail summary, not just "it passed").
3. After showing the output, mark the task's checkbox `- [x]` in this file (Edit tool).
   **Never tick a box without its Done-when output in the same or an earlier turn.**
4. Tasks tagged **(OP)** are operator tasks (devnet deploys, re-provisions, funded
   keys). They are **excluded from every goal condition** — leave them unchecked; a
   human runs them between phases.
5. Hard constraints, always in force:
   - `program/src/clearing.rs` is **never modified** (assert with
     `git diff --stat program/src/clearing.rs` → empty, at every phase gate).
   - No error code or discriminator is renumbered — only appended (§0.3).
   - Every state-struct field append updates `DATA_LEN` + `assert_no_padding!` +
     `to_bytes_inner` + `VERSION` together, with a roundtrip test pinning the new
     version byte.
6. If a Done-when command fails, fix and re-run — do not weaken the check, do not
   skip to the next task. If genuinely blocked (missing tool, needs a product
   decision), write a `> BLOCKED:` note under the task and move on; the goal
   condition treats a BLOCKED note as "surfaced, not silently skipped".
7. **Progress proof**: to show remaining work at any time, run
   `grep -n '^- \[ \]' plan.md` and paste the output. Phase-scoped:
   `awk '/^### 9.[1-6]/{p=0} /^### 9.2/{p=1} p' plan.md | grep -n '^- \[ \]'`
   (adjust the section number).

**Environment note:** on-chain build = `cd program && cargo-build-sbf`; host tests =
`cargo test --features idl`; integration = `just integration-test` (needs a built
`.so`); clients = `just generate-clients`. Plain host `cargo build` without dev-deps
is known-broken (pinocchio 0.11 limitation, see CLAUDE.md) — never use it as a check.

---

### 9.1 Phase 0 — Hygiene

- [x] **P0.1 — Regenerate the web vendor bundle** (§1.1)
  Run the regen and prove the sharding fields landed.
  **Done when:** `pnpm generate-clients && pnpm bundle-client` exits 0 AND
  `grep -c "numSlabShards" apps/web/src/vendor/tempo-client.mjs` prints ≥ 1 — both
  outputs shown.

- [x] **P0.2 — CI guard for the vendor bundle** (§1.1)
  *(Adapted during execution: the bundle is git-ignored and rebuilt by
  `predev`/`prebuild` hooks, so a `git diff --exit-code` guard cannot apply to an
  untracked artifact. The correct guard: CI builds the bundle from freshly
  generated clients and asserts the sharding field is present.)*
  **Done when:** `grep -n "bundle-client" .github/workflows/*.yml` shows a CI step
  that runs the bundle build AND a follow-up assertion
  `grep -q "numSlabShards" apps/web/src/vendor/tempo-client.mjs`.

- [x] **P0.3 — Doc corrections** (§1.2)
  Fix `docs/missing-features.md` (§1.1/§2.2 reduce-only full-reservation text, §1.3
  cap 90-not-128, §7.2 → "won't build") and note §2.11's SDK closure in
  `docs/known-issues.md` once P0.4 lands.
  **Done when:** `grep -c "(0, 128]" docs/missing-features.md` prints 0 AND
  `grep -n "full worst-case" docs/missing-features.md` shows the corrected §2.2 text
  AND `grep -n "won't build\|wont build" docs/missing-features.md` hits §7.2.

- [x] **P0.4 — SDK: always attach the derivable ledger on settle** (§1.3)
  Settle-ix builder derives `[b"collateral", order.trader, market.collateral_mint]`
  on money-path markets unconditionally.
  **Done when:** a new named unit test in `crates/sdk` (e.g.
  `settle_builder_always_attaches_ledger`) passes — `cargo test -p tempo-sdk` output
  shown with the test name green.

- [x] **P0.5 — Marketable-fill end-to-end tests** (§1.4)
  `tests/integration-tests/tests/marketable_fill.rs`: sell-below-floor and
  buy-above-top scenarios, both with a live counterparty after a recenter; assert
  fill at ≥ limit, OI conservation, exact margin release.
  **Done when:** `just integration-test` output shows both `marketable_fill` tests
  passing by name.

- [ ] **P0.6 (OP) — Devnet re-provision on v11 + sim marketable-fill drill** (§1.1/§1.4)
  Operator: re-provision stale devnet markets, run the `crates/sim` drill.

---

### 9.2 Phase 1 — Safety release (Market v12 + MakerQuote v4)

- [ ] **P1.1 — `Market` v12 append block** (§2.1)
  Eight new fields appended (`paused` … `pending_payload`), `DATA_LEN` +
  `assert_no_padding!` `+108`, `to_bytes_inner` extended in order, `VERSION = 12`
  with house-style doc comment, `new()` zero-inits, `le_field!` accessors,
  `PAUSE_INTAKE`/`PAUSE_ROLL` consts, `require_not_paused()`.
  **Done when:** `cargo test --features idl state::market` (or the market test
  filter) passes, including an updated roundtrip test that asserts
  `bytes[1] == 12` and round-trips `paused`, `min_order_notional`,
  `max_open_interest`, `liquidation_reward_floor`, `liquidation_close_buffer_bps`,
  `pending_*` — output with test names shown.

- [ ] **P1.2 — `initialize_market` data → 165 bytes** (§2.1)
  Four appended wire fields + bounds (`buffer ≤ 10000`; buffer requires money path),
  threaded through `Market::new`.
  **Done when:** `initialize_market` data tests pass including a new test asserting
  `InitializeMarketData::LEN == 165` and the two new rejection cases — output shown.

- [ ] **P1.3 — IDL + clients for the new init fields** (§2.1)
  `definition.rs` InitializeMarket variant gains the four fields; regenerate.
  **Done when:** `just generate-clients` exits 0 AND
  `grep -c "minOrderNotional" idl/tempo_program.json` prints ≥ 1 AND `git status
  --short` shows `idl/`, `clients/`, `crates/sdk/src/generated/` diffs — all shown.

- [ ] **P1.4 — `SetPause` (disc 32)** (§2.2)
  Full house wiring (§0.1 checklist): instruction dir, `impl_instructions.rs`,
  `instructions/mod.rs`, discriminator + `TryFrom`, entrypoint arm, Codama variant,
  `MarketPauseChangedEvent` (event disc 10), unknown-bits rejection in `data.rs`.
  **Done when:** `cargo test --features idl` passes (incl. a discriminator-routing
  test for 32) — summary line shown.

- [ ] **P1.5 — Pause guards + tests** (§2.2)
  `require_not_paused(PAUSE_INTAKE)` in `submit_order`, `init_maker_quote`,
  `update_maker_quote_mid`, `update_maker_quote_levels`; `PAUSE_ROLL` in
  `start_auction`. LiteSVM tests: paused submit/quote-write reject `Custom(2)`;
  round drains to settled while paused; cancel + withdraw + liquidate still work;
  `PAUSE_ROLL` parks the market quiescent.
  **Done when:** `just integration-test` shows the new pause tests passing by name.

- [ ] **P1.6 — Minimum order notional** (§2.3)
  Submit check (u128 compare, `OrderBelowMinimum`/29) + per-level maker check priced
  at the window floor.
  **Done when:** tests pass showing: dust order rejected `Custom(29)`, dust maker
  level rejected, `min_order_notional == 0` accepts all — names + summary shown.

- [ ] **P1.7 — `MakerQuote` v4 layout + seeds** (§2.4.1)
  Append `quote_index_le`/`reserved_margin_le`/`worst_price_le`, `VERSION = 4`,
  4-seed set (`+ quote_index_le`), `MAX_QUOTES_PER_MAKER = 4`, accessors, `new()`
  takes `quote_index`.
  **Done when:** state tests pass incl. roundtrip asserting `bytes[1] == 4` and a
  seeds test proving two quote_index values derive different addresses — shown.

- [ ] **P1.8 — `ladder_reservation` in `margin.rs`** (§2.4.2)
  Pure fn + unit tests (rounds up; empty ladder → 0; saturating sum).
  **Done when:** `cargo test --features idl margin` shows the new tests green.

- [ ] **P1.9 — Maker margin wiring across the five instructions** (§2.4.3–§2.4.6)
  `init_maker_quote` (+`quote_index`, LEN 43, `[Seed; 5]`), `update_maker_quote_levels`
  (+maker ledger account, delta lock/release, owner check),
  `clear_maker_quote` (+ledger, release via `release_order_reservation`),
  `close_maker_quote` (reserved-must-be-zero guard), `settle_maker_quote`
  (`lock()` → `lock_up_to` pattern + `effective_collateral`, target =
  `initial_margin`). Codama variants updated.
  **Done when:** `cargo test --features idl` passes AND `just integration-test`
  shows the new `maker_margin.rs` suite green by name: unbacked ladder rejected
  `Custom(24)`; shrink releases exactly; clear releases all; close blocked until
  cleared; drained-maker settle does not revert; re-run of
  `two_makers_share_marginal_tick_and_conserve_oi`; two concurrent quotes
  (index 0/1) fold + settle independently; foreign-ledger substitution rejected;
  reservation unchanged after repeated mid moves.

- [ ] **P1.10 — SDK + mm-bot lockstep** (§2.4.7)
  `crates/sdk/src/pda.rs` maker-quote PDA takes `quote_index`; ix builders add the
  ledger + `quote_index`; mm-bot deposits before quoting, treats
  `InsufficientCollateral` on levels as ladder-shrink, defaults `quote_index 0`.
  **Done when:** `cargo test -p tempo-sdk -p tempo-mm-bot` passes — summary shown.

- [ ] **P1.11 — Phase 1 gate**
  **Done when (all outputs shown):** `cargo fmt --check` exit 0 ·
  `cd program && cargo clippy --all-targets -- -D warnings` exit 0 ·
  `cargo test --features idl` all green · `cd program && cargo-build-sbf` exit 0 ·
  `just generate-clients` produces no uncommitted drift (`git status --short` on
  `idl/ clients/ crates/sdk/src/generated/` empty after commit) ·
  `just integration-test` all green ·
  `git diff --stat program/src/clearing.rs` empty.

- [ ] **P1.12 (OP) — Deploy + re-provision #1 + devnet maker drill**

---

### 9.3 Phase 2 — Admin release (Vault v3)

- [ ] **P2.1 — Staged-change engine on `Market`** (§3.1)
  `PENDING_*` consts, `stage_pending`/`take_pending`, `RISK_UPDATE_DELAY_SLOTS`.
  **Done when:** unit tests pass: stage→take happy path; wrong kind →
  `NoPendingUpdate`; early → `PendingDelayNotElapsed`; take clears the slot — shown.

- [ ] **P2.2 — Shared config validators** (§3.2)
  Extract `validate_fee_config`/`validate_brake_config`/`validate_risk_config` in
  `initialize_market/data.rs`; both init and update paths call them.
  **Done when:** a property-style test asserts init and update reject the same
  out-of-bounds table — test name + pass shown.

- [ ] **P2.3 — `UpdateMarketParams` (33)** (§3.2)
  Hot-set instruction (72-byte data), authority check, `MarketParamsUpdatedEvent`
  (disc 9), full §0.1 wiring.
  **Done when:** integration test shows a fee change applying immediately (next
  settle uses the new fee) and a non-authority caller rejected `Custom(1)` — shown.

- [ ] **P2.4 — `ProposeRiskUpdate`/`ApplyRiskUpdate` (34/35)** (§3.2)
  8-byte payload, shared bounds re-validated at propose, permissionless apply.
  **Done when:** tests show: apply-before-delay `Custom(48)`; apply-wrong-kind
  `Custom(47)`; post-delay apply updates all four bps fields (read back) — shown.

- [ ] **P2.5 — Authority transfer (36/37)** (§3.3)
  Two-step; accept signed by the staged new authority.
  **Done when:** tests show: accept by wrong signer `Custom(1)`; happy path flips
  `market.authority` and emits `AuthorityTransferred` — shown.

- [ ] **P2.6 — Set-oracle (38/39)** (§3.3)
  Propose requires `PAUSE_ROLL`; apply gates: delay + paused + quiescent
  (`MarketNotQuiescent`/49) + live/fresh/confident target feed; atomic
  address+feed-id commit; `OracleRepointedEvent`.
  **Done when:** tests show all four rejection gates by error code, plus the happy
  path where `update_funding` reads the NEW feed after apply — shown.

- [ ] **P2.7 — `Vault` v3 layout** (§3.4)
  Append `authority` + `total_user_balance_le` + `pending_withdraw_*`;
  `VERSION = 3`; `init_vault` records the admin signer as `authority`.
  **Done when:** vault roundtrip test asserts `bytes[1] == 3` + new fields — shown.

- [ ] **P2.8 — User-balance aggregate wiring** (§3.4)
  `settle_money::apply_user_balance_delta` + calls at every balance-changing site
  (deposit, withdraw, withdraw_cross, settle_fill, settle_maker_quote, liquidate,
  liquidate_cross, finalize_clear crank fee, integrator credit).
  **Done when:** the property test passes: after EVERY instruction in the money-path
  LiteSVM suite, `vault.total_user_balance == Σ` scanned ledger balances — test
  name + pass shown.

- [ ] **P2.9 — Fail-closed outflow gate** (§3.4)
  Backing check (`VaultInvariantViolated`/51) in `withdraw` + `withdraw_cross`
  after debit, before transfer.
  **Done when:** test shows a corrupted-aggregate withdraw rejected `Custom(51)`
  and a normal withdraw passing — shown.

- [ ] **P2.10 — `SeedInsurance` (40)** (§3.5)
  Permissionless donate; `InsuranceSeededEvent`; aggregate NOT touched.
  **Done when:** test shows: seeded pool → previously-clamped maker rebate now pays;
  invariant property (P2.8 test) still green — shown.

- [ ] **P2.11 — Phase 2 gate** — same seven checks as P1.11, all outputs shown.

- [ ] **P2.12 (OP) — Deploy + vault re-provision + admin drill on devnet**

---

### 9.4 Phase 3 — Risk depth (no layout changes)

- [ ] **P3.1 — `partial_close_qty` math** (§4.1.1)
  Pure fn in `margin.rs` + unit tests + 20k-iter fuzz (health restored with buffer;
  minimality `c−1` fails; full-close fallbacks: equity ≤ 0, `maint_b ≤ pen_scaled`,
  `c ≥ |size|`).
  **Done when:** `cargo test --features idl margin` shows the unit + fuzz tests
  green by name.

- [ ] **P3.2 — Kani harness** (§4.1.1)
  `proof_partial_close_qty_safe` (panic-freedom, `2^48` envelope), added to
  `kani_proofs.rs` scoped like the existing three.
  **Done when:** `cargo kani --harness proof_partial_close_qty_safe` prints
  `VERIFICATION:- SUCCESSFUL` — output shown. (If kani is not installed in this
  environment, write `> BLOCKED: cargo-kani unavailable` and leave for CI.)

- [ ] **P3.3 — Partial liquidation in `liquidate`** (§4.1.2)
  Close-size decision + partial branch (`apply_fill` at mark, penalty on closed
  notional, partial collateral release, OI delta to new size, `LiquidationNoProgress`
  backstop, event `closed_qty`/`remaining_size` appended — DATA_LEN 104→120).
  **Done when:** `partial_liquidation.rs` integration tests pass by name: slice-only
  close on a 1%-underwater position; repeated calls converge; insolvent → full
  close; dust remainder → full close; conservation assert green.

- [ ] **P3.4 — Partial liquidation in `liquidate_cross`** (§4.1.2)
  Same formula with combined equity/maintenance on the target leg.
  **Done when:** cross partial-liquidation tests pass by name — shown.

- [ ] **P3.5 — Liquidation reward floor** (§4.2)
  Insurance-funded top-up in both liquidate paths, capped at pool; conserving
  (aggregate updated).
  **Done when:** tests show: penalty < floor → topped up; capped at insurance;
  floor 0 = no-op; P2.8 property still green — shown.

- [ ] **P3.6 — OI soft cap** (§4.3)
  Read-only submit check (`OpenInterestCapExceeded`/50), reducing orders exempt,
  `new_exposure_abs` hoisted.
  **Done when:** tests show: over-cap increase rejected `Custom(50)`; pure reduce
  passes over the cap; same-round overshoot case documented-and-asserted — shown.

- [ ] **P3.7 — Insurance withdraw (41/42)** (§4.4)
  Staged on Vault; permissionless apply; backing gate before transfer; clamps to
  shrunk insurance; `InsuranceWithdrawnEvent`.
  **Done when:** tests show: early apply `Custom(48)`; no-pending `Custom(47)`;
  gate-blocked withdraw `Custom(51)`; happy path pays and clears pending — shown.

- [ ] **P3.8 — Mark-price renames + docs** (§4.5)
  `funding_mark` / `solvency_price` local renames; `docs/risk-model.md` "two prices"
  section.
  **Done when:** `grep -rn "solvency_price" program/src/instructions/liquidate/` hits
  AND `cargo test --features idl` still green AND the doc section exists
  (`grep -n "two prices" docs/risk-model.md`) — shown.

- [ ] **P3.9 — Phase 3 gate** — same seven checks as P1.11, all outputs shown.

- [ ] **P3.10 (OP) — Deploy (no re-provision) + partial-liquidation devnet drill**

---

### 9.5 Phase 4 — Trading UX

- [ ] **P4.1 — IOC boundary change** (§5.1)
  Submit guard `<= auction_id` → `< arm_auction_id`; doc comments in
  `submit_order/data.rs` + `definition.rs` updated.
  **Done when:** tests show: `expires == arm` accepted, fills-or-consumes in exactly
  one round (never rests); `expires < arm` rejected `Custom(46)`; mid-round IOC arms
  and expires at `current+1`; reaper strict-`<` boundary unchanged — names shown.

- [ ] **P4.2 — SDK order sugar** (§5.1)
  `submitIoc` / `submitMarketOrder` (window-boundary price) / `closePosition`
  (opposite-side, reduce-only market order).
  **Done when:** `cargo test -p tempo-sdk` shows the three builders' tests green.

- [ ] **P4.3 — `CancelAllOrders` (43)** (§5.2)
  Owner-only shard scan, summed single release, per-order events, header counters.
  **Done when:** tests show: cancels only the signer's Resting orders; one summed
  release equals Σ reserved; zero-order call succeeds as no-op; stranger's expired
  order untouched — names shown.

- [ ] **P4.4 — Order-types design note** (§5.1)
  `docs/system-design.md` note: FOK breaks telescoping-floor conservation;
  post-only = the maker-quote book.
  **Done when:** `grep -n "FOK" docs/system-design.md` hits the new note — shown.

- [ ] **P4.5 — Phase 4 gate** — same seven checks as P1.11, all outputs shown.

- [ ] **P4.6 (OP) — Deploy (no re-provision)**

---

### 9.6 Phase 5 — Benchmark-gated & polish

- [ ] **P5.1 — Round-latency benchmark + C2 decision record** (§6.1)
  Extend `benchmark.rs` + a sim run; measure per-phase wall-clock over ≥ 50 rounds;
  write `docs/bench/round_latency.md` with the numbers AND the explicit go/no-go
  against the decision rule (settle+reset tail vs. collect window).
  **Done when:** the report file exists with real measured numbers and a stated
  decision (`grep -n "decision" docs/bench/round_latency.md` shown), and the
  benchmark run output appears in the conversation.

- [ ] **P5.2 — Keeper early-roll** (§6.1)
  `crates/keeper` `engine::decide`: roll on `shards_ready == num_slab_shards`;
  pipeline `reset_shard` with the settle tail.
  **Done when:** `cargo test -p tempo-keeper` shows the updated `decide` tests green.

- [ ] **P5.3 — Structured `benign()`** (§6.2)
  Typed `InstructionError::Custom(code)` parsing + `BENIGN_CODES` allowlist; string
  matcher demoted to transport-error fallback; regression test kept.
  **Done when:** `cargo test -p tempo-sdk retry` shows: each allowlisted code
  classified benign, a non-listed code classified real, and the fallback test —
  names shown.

- [ ] **P5.4 — EMA funding** (§6.3)
  `read_price` gains `ema_price_1e8` (offset base+68, spot fallback when ≤ 0);
  `update_funding` uses EMA as the index side; solvency path untouched;
  `tempo-math::oracle` mirror + goldens regenerated.
  **Done when:** tests show: divergent synthetic ema/spot → funding rate computed
  off EMA while `solvency_mark` returns spot; tempo-math goldens green;
  `git diff --stat program/src/oracle.rs` shown (reader change only) — all shown.

- [ ] **P5.5 — `ClosePosition` (44)** (§6.4)
  Flat-and-drained + isolated-mode guards; rent to owner.
  **Done when:** tests show: non-flat rejected; cross-member rejected; happy path
  closes and refunds rent — names shown.

- [ ] **P5.6 — `CloseMarket` (45)** (§6.4)
  Full quiescence gates (`MarketNotQuiescent`/49), force_reset-style shard
  count+dedup, closes shards → histogram → clearing_result → market.
  **Done when:** tests show each gate rejecting by code and the happy path
  reclaiming rent for every account — names shown.

- [ ] **P5.7 — Status-table sweep** (§8)
  Update `docs/known-issues.md` Part A + `docs/missing-features.md` status tables to
  reflect everything shipped by this plan.
  **Done when:** `grep -n "absent" docs/missing-features.md` output shows no
  remaining `absent` rows for items this plan implemented (2.1/2.6/2.7/3.1/3.2/3.3/
  3.4/4.1/5.1/6.1/7.1) — shown with the table excerpt.

- [ ] **P5.8 — Phase 5 gate** — same seven checks as P1.11, all outputs shown.

- [ ] **P5.9 (OP) — Deploy + C2 go/no-go executed per the P5.1 decision record**

---

### 9.7 Ready-to-use `/goal` conditions

Run **one goal per phase** (bounded, evaluator context stays sharp), not one
mega-goal for the whole plan. Suggested conditions (adjust turn caps to taste):

```
/goal Every unchecked checkbox in plan.md section 9.1 (Phase 0) that is not tagged (OP) is now checked, and each box was checked only after its "Done when" command output appeared in the conversation. Prove the end state by running: awk '/^### 9.1/{p=1} /^### 9.2/{p=0} p' plan.md | grep '^- \[ \]' — the output must contain only (OP) lines or be empty. Constraint: program/src/clearing.rs is never modified. Stop after 25 turns.
```

```
/goal Every unchecked checkbox in plan.md section 9.2 (Phase 1) not tagged (OP) is checked, each only after its "Done when" output was shown, and the P1.11 gate outputs (fmt, clippy, cargo test --features idl, cargo-build-sbf, generate-clients drift check, integration tests, empty clearing.rs diff) all appear green in the conversation. Prove with: awk '/^### 9.2/{p=1} /^### 9.3/{p=0} p' plan.md | grep '^- \[ \]' showing only (OP) lines. Stop after 60 turns.
```

For phases 2–5, reuse the Phase-1 template with the section number (9.3–9.6) and the
phase's gate task id (P2.11 / P3.9 / P4.5 / P5.8). A full-plan master goal, if you
really want one session to run everything:

```
/goal grep '^- \[ \]' plan.md returns only lines tagged (OP), every checked task's "Done when" output appeared in the conversation before its box was ticked, and all five phase gates (P1.11, P2.11, P3.9, P4.5, P5.8) show green command output. Constraint: program/src/clearing.rs never modified; no error codes or discriminators renumbered. Stop after 250 turns.
```

Notes for the operator:
- The evaluator only sees the conversation — if a session is resumed/compacted, the
  checkbox state in this file is the durable record; ask Claude to re-run the grep
  proof at the start of a resumed goal.
- (OP) tasks between phases (deploys/re-provisions) are yours; start the next
  phase's goal after running them.
- `/goal` needs auto-approved tool calls (auto mode) to run unattended; the builds,
  tests, and file edits here are all local and non-destructive.
