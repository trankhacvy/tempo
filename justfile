# Install JS dependencies
install:
    pnpm install

# Generate the Codama IDL from the Rust program (writes idl/tempo_program.json)
generate-idl:
    cd program && cargo check --features idl

# Generate TypeScript + Rust clients from the IDL
generate-clients: generate-idl
    pnpm run generate-clients

# Build the on-chain program (.so)
build:
    cd program && cargo-build-sbf

# Run the program unit tests (pure math + state serde)
unit-test:
    cargo test -p tempo-program --features idl

# Run the LiteSVM integration tests (requires a built .so)
integration-test *args: build
    cargo test -p tempo-integration-tests {{ args }}

# Run every test
test *args: build unit-test (integration-test args)

# Run the integration suite with a compute-unit benchmark report (writes cu_report.md)
benchmark: build
    CU_REPORT=1 cargo test -p tempo-integration-tests -- --ignored --nocapture

# Format + lint Rust and TypeScript
fmt:
    cargo fmt
    cd program && cargo clippy --all-targets -- -D warnings
    pnpm run format

# Check formatting + types without writing
check:
    cd program && cargo check --features idl
    cargo check --workspace
    pnpm run format:check
    pnpm run typecheck

# Lint + test the off-chain service crates (tempo-math/common/sdk/keeper/api/mm-bot/liquidator/bench + client)
services-check:
    cargo clippy -p tempo-math -p tempo-common -p tempo-sdk -p tempo-keeper -p tempo-api -p tempo-mm-bot -p tempo-liquidator -p tempo-bench -p tempo-sim --all-targets -- -D warnings
    cargo test -p tempo-math -p tempo-common -p tempo-sdk -p tempo-keeper -p tempo-api -p tempo-mm-bot -p tempo-liquidator -p tempo-bench -p tempo-sim

# Run the keeper service (pass --help or rely on TEMPO_* env / tempo.toml for config)
keeper *args:
    cargo run -p tempo-keeper -- {{ args }}

# Run the read API (axum REST + WebSocket; reads TEMPO_* env / tempo.toml for config)
api *args:
    cargo run -p tempo-api -- {{ args }}

# Run the reference market maker (pass --deposit <amt> for the one-shot fund path)
mm *args:
    cargo run -p tempo-mm-bot -- {{ args }}

# Run the reference liquidator (pass --once for a single scan; reads TEMPO_* env)
liq *args:
    cargo run -p tempo-liquidator -- {{ args }}

# Build the four service Docker images (keeper/liquidator/api/mm-bot).
images:
    docker build -f ops/docker/Dockerfile --build-arg BIN=tempo-keeper -t tempo/keeper .
    docker build -f ops/docker/Dockerfile --build-arg BIN=tempo-liquidator -t tempo/liquidator .
    docker build -f ops/docker/Dockerfile --build-arg BIN=tempo-api -t tempo/api .
    docker build -f ops/docker/Dockerfile --build-arg BIN=tempo-mm-bot -t tempo/mm-bot .

# Bring up the devnet ops stack (services + Prometheus + Grafana). Fill ops/compose/.env first.
compose-up:
    docker compose -f ops/compose/docker-compose.yml up -d --build

# Regenerate the committed host-benchmark artifact (docs/bench/clearing_scaling.*).
# For criterion graphs run `cargo bench -p tempo-bench`; for on-chain CU use `just benchmark`.
bench:
    cargo run -p tempo-bench

# Run the formal-verification harnesses (panic/overflow freedom + OI conservation).
kani:
    cd program && cargo kani

# Fail if the committed clients are stale vs the program IDL (regenerate → no diff).
# Guards against the "stale client bundle" class of bug (known-issues §2.12).
clients-fresh: generate-clients
    git diff --exit-code -- crates/sdk/src/generated clients/typescript idl/tempo_program.json

# Verifiable build against the repo (requires solana-verify + docker)
verify-local:
    solana-verify build --library-name tempo_program

# Run the LiteSVM CU parameter sweep (writes sweep.csv)
sweep: build
    cargo test -p tempo-integration-tests --test sweep -- --ignored --nocapture
