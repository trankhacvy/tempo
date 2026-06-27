# Verification matrix

> The correctness evidence for the Tempo program and its off-chain keeper, unified
> as a document (build-plan §D5) rather than a folder. Every **load-bearing
> invariant** maps to a named unit test, LiteSVM integration test, host fuzz, or
> formal (Kani) proof. Empty cells are the honest, current gaps — the matrix doubles
> as the to-do list.

Tooling locations (each dictated by its toolchain):
- **Unit / fuzz** — `program/src/*.rs` and `crates/tempo-math/src/*.rs` `#[cfg(test)]`
  modules (`cargo test -p tempo-program --features idl`, `cargo test -p tempo-math`).
- **Integration** — `tests/integration-tests/tests/*.rs` over LiteSVM (`just integration-test`).
- **Formal** — `program/src/kani_proofs.rs`, `#[cfg(kani)]` (`cd program && cargo kani`).
- **Benchmark** — host: `crates/bench` (`cargo run -p tempo-bench`, `cargo bench -p tempo-bench`);
  on-chain CU/throughput: `tests/integration-tests/tests/benchmark.rs` (`just benchmark`).

---

## Clearing-engine invariants (the crown jewel)

| Invariant | Unit | Integration (LiteSVM) | Fuzz | Formal (Kani) |
| --- | --- | --- | --- | --- |
| Histogram fold is commutative (order-independent) | `state/histogram.rs` fold tests | `determinism.rs` | — | — |
| Clearing price/volume correct on a known book | `clearing::test_known_book_clearing_price_and_rationing` | `happy_path.rs` | — | — |
| **OI / settlement conserves** (`Σ buy == Σ sell == V`) | `clearing` tests | `conservation.rs`, `stress_conservation.rs`, `positions.rs` | `fuzz_full_book_conserves_oi` (20k, program + `tempo-math`) | **`proof_settlement_conserves`** ✅ (new) |
| Marginal fill rounds against the user; never over-fills | `clearing::test_marginal_fill_*`, `test_non_rationed_side_marginal_fills_fully` | `marginal_allocation.rs` | `fuzz_full_book_conserves_oi` | `proof_find_cross_safe` (bounds) |
| Completeness gate / censorship is the only residual crank attack | — | `censorship.rs` | — | — |
| Dual auction crosses independently (bid vs ask) | `clearing::test_dual_auction_independent_crosses` | `happy_path.rs`, `maker_quote*.rs` | — | — |
| Oracle-anchored tick window maps price↔tick | `market::test_recenter_window_*`, `tick` tests (`tempo-math`) | `window.rs`, `window_recenter.rs` | — | — |
| No overflow/panic in `find_cross` raw arithmetic | — | — | differential fuzzes | `proof_find_cross_safe` ✅ |

## Money-path & risk invariants

| Invariant | Unit | Integration (LiteSVM) | Fuzz | Formal (Kani) |
| --- | --- | --- | --- | --- |
| Solvency: `vault_token ≥ Σ balances + insurance` | — | `solvency.rs`, `stress_liquidations.rs`, `cross_margin.rs` | — | — |
| Position VWAP / realized-PnL math | `position.rs` tests | `positions.rs` | — | — |
| Maintenance / liquidation outcome math | `margin.rs` tests | `liquidate.rs`, `stress_liquidations.rs` | — | — |
| `unrealized_pnl` no-overflow in the unit envelope | `margin.rs` tests | — | — | `proof_unrealized_pnl_no_overflow_in_envelope` ✅ |
| 256-bit notional never overflows (`q·price·bps`) | `wide.rs` tests (program + `tempo-math`) | — | `fuzz_wide_vs_u256_reference` (50k) | `proof_wide_mul_no_overflow_and_correct` ✅ |
| Funding rate signed, clamped | `funding.rs` tests (program + `tempo-math`) | `funding.rs` | `fuzz_period_rate_clamped_and_signed` (20k, `tempo-math`) | — |
| Pre-trade margin reserved at submit; released on cancel/settle | `order.rs` tests | `pretrade_safety.rs`, `margin_gate.rs` | — | — |
| Cross-margin combined equity/maintenance | `cross_margin.rs` math tests | `cross_margin.rs` | — | — |

## Lifecycle / protocol-safety invariants

| Invariant | Unit | Integration (LiteSVM) | Fuzz | Formal (Kani) |
| --- | --- | --- | --- | --- |
| Phase guards reject wrong-phase instructions | `market::test_require_phase` | `phase_guards.rs` | — | — |
| Round rolls only from `Settling`/empty `Discovered` with empty slab | — | `lifecycle.rs`, `wedge.rs` | — | — |
| Vault binding (a foreign vault is rejected) | — | `vault_binding.rs` | — | — |
| Anti-spam order cap per trader | `order::count_trader_orders` | `anti_spam.rs` | — | — |
| Account migration (v4→v5 / v1→v2) preserves state | — | `migration.rs` | — | — |
| Security regressions C1–C5 | — | `security_c1_c5.rs` | — | — |

## Off-chain keeper invariants (Phase 1)

| Invariant | Unit | Integration (LiteSVM) | Fuzz | Formal |
| --- | --- | --- | --- | --- |
| `decide` drives the full phase machine correctly | `keeper::engine` tests (12) | `keeper_loop::keeper_drives_full_round` | — | — |
| Crank actions are idempotent ⇒ replicas are safe (D3) | `keeper::actions::benign` tests | `keeper_loop::keeper_actions_are_idempotent_for_replicas` | — | — |
| Account decoders match on-chain byte layout | `sdk::accounts` golden tests (market/slab/clearing/maker-quote) | (exercised live by `keeper_loop`) | — | — |
| Freeze watchdog trips on no-progress while work pending | `keeper::health` tests (3) | — | — | — |
| Instruction builders target the right program/disc/accounts | `sdk::ix` tests | (exercised live by `keeper_loop`) | — | — |

## Benchmark evidence (the grant headline)

| Claim | Artifact |
| --- | --- |
| Clearing cost is **O(ticks), not O(orders)** | `crates/bench` → `docs/bench/clearing_scaling.md` (host: runtime ∝ ticks, flat in order count); `cargo bench -p tempo-bench` (criterion graphs) |
| Per-instruction CU + max orders/auction under the write budget | `tests/integration-tests/tests/benchmark.rs` → `cu_report.md` (`just benchmark`); `sweep.rs` → `sweep.csv` (`just sweep`) |

---

### Honest gaps (open cells above)

- **Histogram write-lock contention / max orders-per-auction** are *measurements the
  M1 benchmark produces*, not invariants to assert — tracked under the benchmark row,
  reported as numbers (CLAUDE.md "Known gaps").
- **Conservation: discovery-level vs settlement-level.** `proof_settlement_conserves`
  proves *exhaustively and division-free* that `find_cross`'s published rationing
  constants decompose `matched_volume` exactly on both sides. The per-order
  cumulative-floor split *within* the marginal bucket (`compute_marginal_fill`) stays
  on the 20k host fuzz `fuzz_full_book_conserves_oi`, because that function's `u128`
  division is CBMC-intractable (documented in `kani_proofs.rs`).
- **Solvency** has strong integration coverage but no formal proof — the `u128`/token
  arithmetic is out of practical CBMC reach; the invariant test is the guard.

---

### Phase 2 surfaces (api + mm-bot) — what is and isn't covered

These are the chain-backed read API and the reference market maker (the indexer +
web UI remain deferred). They are *not* load-bearing for clearing correctness, so
they carry service-level tests, not invariant proofs.

| Surface | Coverage |
| --- | --- |
| SDK decoders (histogram, position, user-collateral, extended clearing/maker-quote) | Golden offset tests in `crates/sdk/src/accounts.rs` (synthesize bytes at documented offsets, assert read-back) |
| SDK ix wrappers (maker-quote lifecycle, init-position, collateral/deposit, `encode_levels`) | Unit tests in `crates/sdk/src/ix.rs` (discriminator + account count + byte-exact ladder layout) |
| API handlers / error mapping / WS push | `crates/api/tests/handlers.rs` against a hand-seeded `LiveState` (no RPC): every endpoint's status + JSON, cross-present-iff-finalized, 400/404/501/503 mapping, broadcast delivery |
| MM quoting strategy (skew, window-bounds clamp, collateral cap, levels) | Pure unit tests in `crates/mm-bot/src/strategy.rs` |
| MM ladder encoding + bounds match on-chain, end-to-end fill | `tests/integration-tests/tests/mm_loop.rs` — `build_quote` → `update_maker_quote_levels` → keeper crank → asserts the maker's posted bid folds and fills against a crossing taker, conserving |

**Honest gaps.** The **API serves current state from chain**, not an event index;
event-derived **history (fills, funding) is gated behind `HistorySource`** and
returns `501` until the indexer ships (build-plan D2 rejects RPC *event* streaming,
not current-state reads). The **MM is a permissionless reference** (build-plan D4):
the exchange's safety never assumes our instance runs. WebSocket coverage is at the
broadcast + frame-serialization level; a full socket-upgrade test needs a running
server and is left to the manual devnet smoke.

---

### Phase 3 surfaces (liquidator + ops) — what is and isn't covered

The reference liquidator is the risk backstop; `ops/` is the deploy/monitoring
stack. The liquidator is *not* load-bearing (build-plan D4) — the program is always
the final gate — so it carries service-level + agreement tests, not new invariant
proofs. The on-chain liquidation invariants themselves stay proven by the existing
`liquidate.rs` / `cross_margin.rs` suites.

| Surface | Coverage |
| --- | --- |
| Off-chain Pyth reader (`tempo_math::oracle`) matches the program | The program's own `oracle.rs` unit tests are copied **verbatim** into `crates/tempo-math/src/oracle.rs` (the golden guard) — feed-id match, staleness vs future-timestamp, confidence gate, partial/full verification offset, fresh/soft-stale/hard-stale `solvency_mark` |
| SDK decoders (`MarginAccountView`, `VaultView`) | Golden offset tests in `crates/sdk/src/accounts.rs` (synthesize bytes, assert read-back + disc/short/clamp rejects) |
| SDK `ix::liquidate` wrapper | Unit test in `crates/sdk/src/ix.rs` (discriminator + 9-account count) |
| Liquidation decision engine (isolated + cross combined-health) | Pure unit tests in `crates/liquidator/src/engine.rs` reusing the *program's own* margin/cross-margin scenarios over `tempo_math::margin` — so the off-chain gate computes the same numbers |
| Source seam + readiness | `source.rs` (`MockSource`) + `health.rs` (stale-scan / rpc-down) + `config.rs` (market-list parse) unit tests |
| Engine decision ⊆ on-chain gate, end-to-end close | `tests/integration-tests/tests/liquidator_loop.rs` — decode real on-chain state → `engine::isolated_liquidatable`/`cross_liquidatable` → fire the SDK `ix::liquidate`/`liquidate_cross`; asserts healthy-rejects, the position closes, the liquidator is paid the penalty, and `vault == Σ balances + insurance` holds |

**Honest gaps.** The local pre-filter scores `collateral + realized + unrealized` vs
maintenance at the raw mark but **omits unsettled funding/social** (the program's
`pending` term), which only ever makes a position *more* liquidatable on-chain — so
a funding-only-underwater account may wait one extra scan; the on-chain gate is
exact (phase-3-plan §5, first enhancement). The **`PositionSource` is a bounded
`getProgramAccounts` scan**, swapped for the deferred indexer behind the trait (D1).
**KMS/Vault signing** is a documented `TempoSigner` drop-in, not wired (D6). The
`ops/` stack (Docker/compose/Prometheus/Grafana/systemd/CI) validates on first CI
run / `docker compose up` — Docker + `actionlint` are not in the build environment.
