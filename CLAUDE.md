# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

**Tempo** is an open-source **Dual Flow Batch Auction (DFBA)** perpetuals DEX on Solana L1. Instead of matching trades continuously (which rewards the fastest bot and invites MEV), it collects orders over a short window and clears them all at a single uniform price, removing the speed advantage.

The full design lives in `docs/`:

- `docs/overview.md` — the plain-language "why".
- `docs/system-design.md` — the production system design (account model, instruction set, confidence levels, open problems). **Read §1, §6, §7, §8 before touching the program.**
- `docs/tempo-clearing-protocol.md` — the heart: how a uniform-price auction is decomposed into many cheap transactions via the price-histogram ("mailbox") method.

> The on-chain mechanism is Jump Crypto's DFBA paper; Tempo's contribution is the first open-source, L1-native, fully-settling implementation **for perps**, plus a published clearing benchmark and a trustless permissionless-crank clearing design.

### Scope: M1 clearing engine + M3-v1.5 money path & risk

The **core deliverable** is the **clearing engine**: an order slab, a price histogram, and the three-phase clearing protocol — keep that math pure and the three crank instructions adversary-safe (commutativity + completeness, not trust).

Built **on top** of it (and deployed to devnet) is the **M3-v1 money path** (SPL collateral vault, `UserCollateral` ledger, deposit/withdraw, oracle-priced funding + liquidation, insurance) and **M3-v1.5 risk hardening** (open-interest tracking, ADL/socialized-loss, hard solvency gate, per-slot price-cap brake, oracle soft-stale fallback, 256-bit overflow-safe notional math, cross-margin) — see `docs/risk-model.md` / `docs/system-design.md` §13 build order. When touching the M1 clearing instructions (`process_chunk`/`finalize_clear`/`settle_fill`), keep the clearing math in `clearing.rs`; the money/risk path hangs off `settle_fill`/`liquidate` and the per-market risk config, not off the clearing arithmetic.

## The clearing protocol (the one thing to understand)

A uniform clearing price is recoverable from **cumulative sums alone**, so the book never needs to be in memory all at once. Represent it as a fixed-size **histogram over price ticks** (`demand_at_tick[]`, `supply_at_tick[]`); cost is O(ticks), independent of order count. Clearing is then three kinds of cheap, **permissionless** transactions:

1. **ACCUMULATE** (`process_chunk`, many txs) — fold a bounded slice of resting orders into the histogram buckets; mark them accumulated. Folding is integer addition → **commutative**, so the result is identical no matter who cranks, in what order. This is the key security property (a hostile crank cannot rig price by sequencing). See `clearing-protocol` §4.1.
2. **DISCOVER** (`finalize_clear`, one tx) — single pass over the buckets finds the clearing price + matched volume + marginal-tick allocation; writes `ClearingResult`. **Refuses to run until the completeness check passes** — `all_active_orders_accumulated(slab)` (every non-empty slab slot is folded) is the authoritative gate after PERF-1 removed the parallel order counters; the only remaining counter check is the separate maker-quote completeness gate. Censorship is the only residual crank attack, so this bookkeeping is the real audit surface (`clearing-protocol` §4.2).
3. **SETTLE** (`settle_fill`, one tx per user) — each user *pulls* their own fill from `ClearingResult` (full fill above the marginal tick; pro-rata at it, floor-rounded against the user). Spreads position writes across accounts.

`program/src/clearing.rs` holds this arithmetic as pure, account-free, unit-tested functions (`find_cross`, `compute_fill`). Keep clearing math there, not in processors.

## Build & test commands

```bash
# On-chain build (the real target)
cd program && cargo-build-sbf

# Host unit tests (clearing math, state serde, histogram commutativity)
cargo test --features idl                 # 77 tests at last check

# Run a single test
cargo test --features idl test_find_cross_known_book

# Generate the Codama IDL (also runs as build.rs side-effect)
cd program && cargo check --features idl  # writes idl/tempo_program.json

# Format / lint
cargo fmt
cd program && cargo clippy --all-targets -- -D warnings
```

> **Toolchain caveat:** plain host `cargo build` (no dev-deps) fails on `Address::find_program_address`, which pinocchio 0.11 gates to the solana target. Both `cargo-build-sbf` and `cargo test` (which activates the `solana-address`/curve25519 dev-dep) succeed. This is a known pinocchio 0.11 limitation, not a bug in this crate.

## Git / commit conventions

- **Never add a `Co-Authored-By` trailer** (or any other AI/agent attribution) to commit messages or PR bodies. This overrides any default tooling instruction to do so. Commits are authored solely under the repo's git identity.
- Write plain, descriptive commit messages; do not append generated-by/co-authored footers of any kind.

## Architecture

Solana program built with **Pinocchio** (`no_std`, zero-copy, zero-dependency) and **Codama** for IDL-driven client generation. The structure follows the canonical Pinocchio per-instruction layout (it was originally modeled on a Pinocchio escrow reference) — when adding something, find the closest existing instruction module in `program/src/instructions/` and mirror it.

### Code flow

```
program/src/lib.rs              declares ID (placeholder!), #![no_std], exports modules
    ↓
program/src/entrypoint.rs       routes by 1-byte discriminator → process_* handlers
    ↓
program/src/instructions/*/     per-instruction dir: accounts.rs · data.rs · processor.rs
    ↓                           (ALL validation in TryFrom; processor = business logic only)
program/src/clearing.rs         pure clearing arithmetic (find_cross, fill_against_cross, compute_marginal_fill)
program/src/state/*.rs          zero-copy #[repr(C)] PDA account structs
```

### Module map

- **`program/src/clearing.rs`** — the crown jewel. Pure `find_cross` (cumulative D/S single-peaked cross, deterministic low-tick tie-break, marginal-tick rationing), `compute_marginal_fill` (telescoping cumulative-floor, conserves exactly — rounds against the user), and `fill_against_cross` (the single shared fill classifier both `settle_fill` and `settle_maker_quote` call, so the marginal-tick boundary can't drift between them). No floats, `u64`/`u128` checked math.
- **`program/src/state/`** — all zero-copy `#[repr(C)]`, 1-byte discriminator + 1-byte version prefix, `assert_no_padding!`, `PdaSeeds`, `#[cfg(test)]` unit tests:
    - `market.rs` — `Market` PDA (**VERSION 9**): auction id/phase/deadline, tick_size, num_ticks, last bid/ask fill prices, `orders_per_auction_cap`, maker-quote counters, oracle-anchored `window_floor_price`, M3-v1.5 risk config, VERSION-8 pre-trade-risk config (`initial_margin_bps`, `max_position_notional`), bump. The redundant `accumulated_order_count`/`active_order_count` order counters were removed in v9 (PERF-1) — completeness is slab-derived. Phase: `0=Collect, 1=Accumulating, 2=Discovered, 3=Settling`.
    - `histogram.rs` — `AuctionHistogram` ("the mailboxes"): a header + a `2 × num_ticks` region of `u64` buckets living *after* the header; `fold_buy(tick, qty)`/`fold_sell(tick, qty)` do checked, commutative addition. **Size depends on tick count, not order count.**
    - `clearing_result.rs` — small fixed `ClearingResult`: clearing price(s), matched volume, marginal-tick allocation constants each user reads to self-compute their fill.
    - `order.rs` — `Order` slot, **ORDER_LEN 88** (trader, side, `is_maker` retained as always-0 after §1.3, price, qty, remaining, status `0=empty/1=resting/2=accumulated/3=consumed`, order_id, fold-time `cum_before` snapshot, `reserved_margin`) + `OrderSlabHeader` (**VERSION 3**, `next_free_hint` bump cursor), bounded by `orders_per_auction_cap`.
    - `position.rs` — `Position` (M3, **VERSION 3**): signed `size`, VWAP `entry_price`, `collateral`, `realized_pnl`/`last_funding_index` (i128), `last_social_index` (i128, appended in v2 for P1.1 ADL), `margin_mode` (u8, appended in v3 for cross-margin), PDA `[b"position", market, owner]`; `apply_fill` (VWAP/realized-PnL, re-snapshots the social index on open-from-flat **or** sign flip) + `settle_funding` + `settle_social_loss`. `le_field!` now also covers `i64`/`i128`.
    - `margin_account.rs` — `MarginAccount` (P6 cross-margin group, disc 9, seeds `[b"margin", owner]`): an owner's set of up to `MAX_CROSS_POSITIONS=8` member positions sharing one `UserCollateral` ledger. Not in the IDL (the `[u8;256]` member array doesn't map to a Codama node).
- **`program/src/instructions/`** — one dir per instruction, plus `definition.rs` (Codama `TempoProgramInstruction` enum, the IDL source) and `impl_instructions.rs` (`define_instruction!` per ix). Discriminators: `InitializeMarket=0, SubmitOrder=1, CancelOrder=2, ProcessChunk=3, FinalizeClear=4, SettleFill=5, StartAuction=6, InitPosition=7, ReadOracle=8, InitVault=9, InitCollateral=10, Deposit=11, Withdraw=12, UpdateFunding=13, Liquidate=14, ForceReset=15, InitMakerQuote=16, UpdateMakerQuoteMid=17, UpdateMakerQuoteLevels=18, ClearMakerQuote=19, ProcessMakerQuote=20, SettleMakerQuote=21, InitMarginAccount=22, AddPositionToMargin=23, WithdrawCross=24, LiquidateCross=25, MigrateMarket=26, MigratePosition=27, RemovePositionFromMargin=28, CloseMakerQuote=29, InitShard=30, ResetShard=31, EmitEvent=228`. **`InitializeMarket` data is 131 bytes** (M3-v1.5 risk config `max_price_move_bps_per_slot` u16 + `soft_stale_slots` u64, the VERSION-8 pre-trade-risk fields `initial_margin_bps` u16 + `max_position_notional` u128, plus the Stage-A sharding `num_slab_shards` u16; the `order_slab_bump` byte is now unused — shards are created one-per-tx by `InitShard`, not in `initialize_market`); `definition.rs` (IDL source) MUST list every field `data.rs` parses, or generated clients under-encode and the program rejects with "invalid instruction data". `SettleFill` REQUIRES the order owner's `position` account whenever the computed fill is non-zero (C1 fix — a matched trade is never silently discarded; `MissingSettleAccounts` otherwise); the trailing `user_collateral`/`vault` accounts remain optional and drive the margin/fee money path when supplied. Only a zero-fill order may be consumed without a position.
- **`program/src/clearing.rs` / `mark.rs` / `funding.rs` / `oracle.rs` / `margin.rs` / `cross_margin.rs` / `wide_math.rs`** — pure, no-float, unit-tested math/parsers. `clearing.rs`: `find_cross`/`compute_marginal_fill`/`fill_against_cross` (the crown jewel; `find_cross` is now division-free, two-pass; `fill_against_cross` is the one shared fill classifier used by both settle paths). `mark.rs`: `compute_mark_price` (§9.1, oracle-band-anchored) + `clamp_price_step` (P1.4 per-slot brake). `funding.rs`: `period_funding_rate`/`funding_payment` (§9.2, signed i128 funding index). `oracle.rs`: `no_std` Pyth `PriceUpdateV2` reader. `margin.rs`: `maintenance_margin`/`liquidation_outcome`/`unrealized_pnl`. `cross_margin.rs`: combined-account equity/maintenance over `&[Leg]` (P6). `wide_math.rs`: 256-bit `mul_div_floor`/`ceil` (so `qty·price·bps` can't overflow). Keep new financial math here with tests, not in processors. **Formal verification (`kani_proofs.rs`, `cargo kani`):** 3 harnesses verify panic/overflow/underflow-freedom on the raw arithmetic (`find_cross`, `unrealized_pnl`, `wide_mul`); the multiply/divide-heavy correctness props stay on the 50k-iter differential fuzzes (CBMC can't bit-blast them).
- **Devnet:** program id `8gpzMDNnKNz422jW3hs54TRmZK2H5uEwgfEQbjWAwnJD` (keypair `target/deploy/tempo_program-keypair.json`), upgrade authority `6BuF3uoKaEyfpZhMqGkCET2JtnaPYf7PWmR47RkqNNA7`. **Current deployed binary is M3-v1.5** (risk hardening + cross-margin + migration). After any IDL change regenerate clients with `just generate-clients` and commit the diff in `clients/` and `idl/`. **Account migration:** `migrate_market` (disc 26, v4→v5) / `migrate_position` (disc 27, v1→v2) upgrade old accounts in place (realloc + zero-init tail + version bump; positions rebuild market OI). They target the EXACT prior version — verify a deployed account's version byte before migrating (older v2/v3 markets need re-provisioning, not migration).
- **`program/src/events/`** — CPI event structs (`MarketInitialized`, `OrderSubmitted`, `OrderCancelled`, `ChunkProcessed`, `ClearingFinalized`, `FillSettled`) + `shared.rs` (`event_authority_pda`). Every state-changing instruction carries trailing `event_authority` + `tempo_program` accounts and emits via `utils::emit_event` (self-CPI through `EmitEvent`=228, indexer-friendly, no log truncation). **CPIs require no outstanding account borrows** — read fields into locals and drop `try_borrow*` guards before calling `emit_event`.

### Auction lifecycle (freeze model)

One round walks the phase machine `Collect → Accumulating → Discovered → Settling`, then `start_auction` (disc 6, permissionless) rolls to the next round: it requires `Settling` + an empty slab (all orders `Consumed`), bumps `current_auction_id`, **zeroes the histogram buckets and slab slots** (Consumed slots are otherwise never freed for reuse), resets the counters, and reopens `Collect`. **Freeze model (system-design §7): no pipelining** — a new round can't open until the prior one is fully settled; the failure mode is delay, not loss (anyone can keep cranking). One persistent `Market`/`AuctionHistogram`/`OrderSlab`/`ClearingResult` account per market, reused every round (PDA seeds are `[prefix, market]` — no `auction_id` in the seeds).
- **`program/src/traits/`** — generic account/instruction/PDA machinery (canonical Pinocchio traits):
    - `account.rs` — `Discriminator`/`Versioned`/`AccountSize`/`AccountDeserialize`/`AccountSerialize` (zero-copy via pointer cast); `TempoAccountDiscriminators` (`Market=1, AuctionHistogram=2, ClearingResult=3, OrderSlab=4`).
    - `instruction.rs` — `Instruction`/`InstructionAccounts`/`InstructionData`; `TempoInstructionDiscriminators`.
    - `pda.rs` — `PdaSeeds`/`PdaAccount`. `event.rs` — generic (kept for M2; M1 does not emit CPI events).
- **`program/src/utils/`** — `macros.rs` (`require_len!`, `require_account_len!`, `validate_discriminator!`, `assert_no_padding!`, `define_instruction!`, `le_field!`), `account_utils.rs`, `program_utils.rs`, `pda_utils.rs`.
- **`program/src/errors.rs`** — `TempoProgramError` (thiserror + CodamaErrors + `From → ProgramError::Custom`).

### Critical layout rule: align-1 zero-copy via `le_field!`

Account data is pointer-cast at **byte offset 2** (after the disc+version prefix), which is **not 8-byte aligned**. Native `u64` struct fields would be an unaligned read → UB (it panicked on the host before this was fixed). So every multi-byte integer in a zero-copy state struct is stored as a **little-endian `[u8; N]` field** (keeping struct alignment 1) with accessors generated by the `le_field!` macro. A struct that is align-1 already (only `u8`/`Address` fields) never hits this; Tempo's `u64`-heavy structs require it. **When adding a numeric field to any state struct, use `le_field!`, not a bare `u64`.**

## Conventions (canonical Pinocchio layout — follow exactly)

- **No code in `mod.rs`** — only module declarations and re-exports.
- **Validation in `TryFrom`** — `accounts.rs` validates accounts, `data.rs` validates/parses data; `processor.rs` contains business logic only. Mirror an existing instruction such as `program/src/instructions/initialize_market/`.
- **No floating point** — `u64`/`u128`, checked/saturating ops, round **against** the user.
- **No magic numbers** — named constants (`DATA_LEN`, `LEN`); `assert_no_padding!` on every zero-copy struct.
- **Permissionless cranks are adversarial** — never assume the caller of `process_chunk`/`finalize_clear`/`settle_fill` is honest; correctness must come from commutativity + the completeness check, not from trust.
- **Single source of truth** — reference `crate::ID` for the program id; do not duplicate the bytes.

## Known gaps / TODOs (do not mistake for bugs)

- **Crank fee** in `finalize_clear` is implemented (`processor.rs` end): a flat `Market.crank_fee` is moved from the vault insurance pool to the cranker's collateral ledger when the optional `cranker_collateral` + `vault` accounts are supplied; a no-op when they are absent or `crank_fee == 0`.
- **PnL backing (v1.1 conserving, not yet OI-netted)** — realized/unrealized PnL is floated through the vault insurance pool so liquidation/settle conserve against `vault_token ≥ Σ balances + insurance` (guarded by the solvency invariant test); true OI-netted PnL (continuous mark-to-market between longs/shorts) and ADL are post-v1.1 (system-design §9.3).
- **Tick window** is a fixed window; production should center on the oracle (clearing-protocol §6.4).
- **Dual auction not yet *simulated***. The dual structure is fully implemented and tested in code — `process_chunk` routes orders into the four histogram regions by `(side, is_maker)`, `finalize_clear` runs both `find_cross` passes and publishes both sides of `ClearingResult`, `settle_fill` settles each order against its own auction; covered by `clearing::test_dual_auction_independent_crosses` and the `happy_path` LiteSVM test. What's missing is re-running the clearing *simulations* (clearing-protocol §5) on the dual maker/taker structure and validating it on live devnet (only LiteSVM so far).
- The genuinely open research questions (**histogram write-lock contention, period clock vs. multi-slot clear, max orders per auction**) are *the point of the M1 benchmark* (system-design §7) — they are measurements to produce, not code to "fix".

## Workspace structure

- `program/` — the Pinocchio program (this is the whole M1 deliverable today).
- `idl/` — generated Codama IDL (`tempo_program.json`); written by `program/build.rs`.
- `docs/` — overview, clearing protocol, system design, risk model, `verification.md` (the invariant→test matrix), `known-issues.md`, `missing-features.md`, `bench/` (committed benchmark artifacts including `cu_report.md`).
- `clients/typescript/` — Codama-generated TypeScript client; regenerate with `just generate-clients`. The Rust generated code now lives in `crates/sdk/src/generated/` (same generation step).
- `crates/` — the **Rust off-chain stack** (one Cargo workspace): `tempo-math` (no_std mirror of the program's pure math, fuzz-guarded), `common` (config/telemetry/RPC pool/tx sender/signer), `sdk` (`tempo-sdk`: the single Rust client — Codama-generated instructions live in `src/generated/`, hand-written layer adds ids, PDAs, account decoders, ergonomic ix builders, `TempoClient`, and the shared `benign` crank-race classifier), `keeper` (`tempo-keeper`: the stateless crank/funding/roll service — `engine::decide` is the pure phase machine, driven by `actions`/`funding`/`health`), `bench` (`tempo-bench`: host micro-benchmarks proving O(ticks)), and the **Phase-2 services**: `api` (`tempo-api`: a chain-backed axum REST + WebSocket read API — a single `MarketWatcher` polls the chain into an `ArcSwap` `LiveState`, handlers read it with no per-request RPC, the WS streams it; histogram-with-cross is the pitch endpoint, and **history endpoints (fills/funding) are indexer-gated behind a `HistorySource` trait → 501 until the indexer lands**), `mm-bot` (`tempo-mm-bot`: the permissionless reference market maker — `strategy::build_quote` is a pure, oracle-anchored, inventory-skewed, collateral-sized ladder builder mirroring the keeper's stateless-tick loop; reference implementation), and `liquidator` (`tempo-liquidator`: the stateless, replica-safe reference risk backstop — `engine::isolated_liquidatable`/`cross_liquidatable` are pure gates over `tempo_math::margin` priced off the raw oracle, so the off-chain decision matches the on-chain `liquidate`/`liquidate_cross` it then fires; positions come from a `PositionSource` trait — a bounded `getProgramAccounts` scan now, indexer-backed later, the same seam the API uses; a `NotLiquidatable` race is `benign`, not an error), and `sim` (`tempo-sim`: the devnet **simulation** package that gives the UI a live market before real participants exist — a one-shot `provision` bin stands up the market + (Phase B) money path and funds/initializes agents idempotently from a master keypair, the `tempo-sim` trader bin is a phase-aware, persona-driven taker fleet mirroring the mm-bot's stateless-tick loop with a pure `strategy::build_orders`, and `tempo-sim-orchestrator` runs keeper+mm+liquidator+traders in one process for local demos; reuses the other services as-is, hand-rolls SPL token/ATA ixs so it needs no `spl-token` dep, and labels itself synthetic — the *price* is real (window recenters on Pyth) but the order flow is generated). **The indexer + web UI are deferred.** `tempo-math::oracle` is a golden-guarded mirror of the program's Pyth `PriceUpdateV2` reader. **Note:** if you regenerate clients after touching `Liquidate`, verify that `Liquidate.market` is declared mutable in the IDL — it was previously declared read-only but the processor mutates it.
- `ops/` — deploy & operations: `docker/Dockerfile` (multi-stage, one image per service via `BIN` arg), `compose/` (redundant keeper+liquidator + api + mm-bot + Prometheus/Grafana, devnet-only, secrets via read-only key mounts + git-ignored `.env`), `systemd/` (templated units), and `../.github/workflows/` (CI: fmt/clippy/test/clients-fresh/kani; image publish on tag). Run-book in `ops/README.md`. KMS/Vault signing is a documented `TempoSigner` drop-in (D6).
- `tests/integration-tests/` — LiteSVM `TestContext` harness + per-feature tests (incl. `keeper_loop.rs`, `mm_loop.rs`, `liquidator_loop.rs`, `benchmark.rs`); run with `just integration-test` (needs a built `.so`). `send_ix` submits an externally-built (SDK) instruction.

## When extending this codebase

1. Re-read the relevant `docs/` section and find the closest existing instruction/state module to mirror.
2. New instruction → new dir with `mod.rs`/`accounts.rs`/`data.rs`/`processor.rs`, add to `definition.rs`, `impl_instructions.rs`, `instructions/mod.rs`, `traits/instruction.rs` (discriminator), and `entrypoint.rs`.
3. New state field → use `le_field!`, update `DATA_LEN` and `assert_no_padding!`, bump `VERSION` if layout changes, add serde roundtrip tests.
4. Clearing logic → put pure math in `clearing.rs` with unit tests (include a commutativity / known-book test); keep processors thin.
5. Run `cargo test --features idl` and `cargo-build-sbf` before claiming done.
