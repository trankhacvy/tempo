# Tempo Trident fuzz target

Transaction-level fuzzing of the Tempo program with [Trident](https://github.com/Ackee-Blockchain/trident),
using the **trident-svm** execution backend (PR #27 enabled `get_sysvar` for
Pinocchio programs, so Tempo's no-Anchor program runs in a real SVM).

This crate is **excluded from the main workspace** (`exclude` in the root
`Cargo.toml`) so `cargo build` / `cargo test` / `cargo-build-sbf` stay
self-contained and never depend on the Trident toolchain. It is the automated,
sequence-level complement to:

- the host **property fuzzes** (`program/src/*.rs` `#[test] fuzz_*`), and
- the **LiteSVM integration tests** (`tests/integration-tests/`), which already
  assert the same invariants point-wise across hand-built scenarios.

## Invariants enforced (after every transaction / settled round)

1. **Solvency** — `vault_token_balance >= Σ user_collateral.balance + insurance`.
2. **OI balance** — `oi_long == oi_short` once a round is fully settled.
3. **Liveness** — no instruction panics (the SVM aborts on a real panic).
4. **No-leak** — a bad-debt shortfall only ever appears with a matching
   social-loss (ADL) index increase, never silently dropped.

These are encoded in `fuzz_tests/conservation/invariants.rs` against the same
fixed account byte offsets the LiteSVM harness uses.

## Run

```bash
# 1. Build the SBF artifact the fuzzer loads.
cd program && cargo-build-sbf
# 2. Install Trident (one-time) and run the target.
cargo install trident-cli
cd ../trident-tests && trident fuzz run conservation
```

## Status

The invariant assertions are the load-bearing, version-independent part and are
complete. The instruction-encoding / bootstrap glue is written against the
Trident API and may need a minor pin adjustment to the exact installed
`trident-fuzz` version (the macro surface evolves between releases) — the same
way the Kani harnesses (`program/src/kani_proofs.rs`) require the `cargo kani`
toolchain to build and run.
