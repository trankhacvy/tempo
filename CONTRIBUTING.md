# Contributing to Tempo

## Before you start

Read the architecture docs in this order:

1. [`docs/overview.md`](docs/overview.md) — the "why" (2 min)
2. [`docs/tempo-clearing-protocol.md`](docs/tempo-clearing-protocol.md) — the core mechanism (10 min)
3. [`docs/system-design.md`](docs/system-design.md) §1, §6, §7, §8 — account model, instruction set, open constraints (15 min)
4. `CLAUDE.md` — module map, file-by-file architecture guide, and engineering conventions

## Engineering conventions

- **Validation in `TryFrom`** — `accounts.rs` validates accounts, `data.rs` parses data; `processor.rs` is business logic only
- **No floats** — `u64`/`u128`, checked/saturating ops, round against the user
- **`le_field!` for all multi-byte integers** in zero-copy state structs (alignment is 1; bare `u64` fields are UB — see `CLAUDE.md`)
- **New clearing logic** → `clearing.rs` with unit tests (include a commutativity test)
- **New instruction** → new dir under `program/src/instructions/` with `mod.rs` / `accounts.rs` / `data.rs` / `processor.rs`; add to `definition.rs`, `impl_instructions.rs`, `instructions/mod.rs`, `traits/instruction.rs`, and `entrypoint.rs`
- **New state field** → `le_field!`, update `DATA_LEN` + `assert_no_padding!`, bump `VERSION`

## Before submitting a PR

```bash
cargo test --features idl   # all unit tests must pass
cargo-build-sbf             # program must build for the SBF target
cd program && cargo clippy --all-targets -- -D warnings
```

If you touched the IDL, also run:

```bash
just generate-clients
```

and commit the regenerated files in `clients/` and `idl/`.
