# Tempo

An open-source **Dual Flow Batch Auction (DFBA)** perpetuals DEX on Solana.

Instead of matching trades one by one as they arrive — rewarding whoever has the fastest connection and giving bots a speed edge — Tempo collects orders over a short window and clears them all together at a single uniform price. Speed inside the window confers no advantage; the race disappears and competition shifts back to price.

The on-chain mechanism is Jump Crypto's [DFBA design](https://jumpcrypto.com/resources/dual-flow-batch-auction). Tempo's contribution is the **first open-source, fully-settling implementation for perpetuals** - with a permissionless, trustless-crank clearing design so that the clearing operator cannot rig the price, delay fills, or censor orders.

Docs (`docs/`), in reading order:
- [`overview.md`](docs/overview.md) — plain-language "why"
- [`tempo-clearing-protocol.md`](docs/tempo-clearing-protocol.md) — the core price-histogram mechanism
- [`system-design.md`](docs/system-design.md) — architecture, account model, instruction set
- [`risk-model.md`](docs/risk-model.md) — the perpetuals money and risk layer

Reference docs: [`verification.md`](docs/verification.md) (invariant → test matrix),
[`known-issues.md`](docs/known-issues.md) / [`missing-features.md`](docs/missing-features.md)
(current gaps), [`design-decisions.md`](docs/design-decisions.md) (dated design rationale),
[`plan.md`](docs/plan.md) (the sharding/resting-order/pipelining scaling plan).

## How it works

Each market repeats this cycle every auction round:

```mermaid
stateDiagram-v2
    direction LR
    [*] --> Collect
    Collect --> Accumulate : deadline passes
    Accumulate --> Discover : all orders folded
    Discover --> Settle : clearing price found
    Settle --> Collect : all fills claimed
```

- **Collect** — traders submit orders into the slab during the window
- **Accumulate** — orders are folded into a price histogram (permissionless, commutative — anyone can crank any slice in any order and the result is identical)
- **Discover** — one pass over the histogram finds the uniform clearing price and fill allocation
- **Settle** — each trader pulls their own fill; positions update, funding accrues

The histogram is fixed-size (O(ticks), not O(orders)), so clearing cost is independent of book depth and decomposes across many cheap transactions.

On top of this, the order book is sharded across several independent accounts so
submission and settlement run in parallel (each shard is folded into the one shared
histogram); an order that only partially fills carries forward into the next round
instead of being discarded; and submission is always-open — an order placed mid-round
simply joins the next one rather than being rejected. Market makers post a standing
ladder of price levels through a separate `MakerQuote` book instead of one-shot orders,
so maker and taker flow clear as two independent auctions (bid: makers-buy vs
takers-sell; ask: takers-buy vs makers-sell) rather than one combined cross. See
`tempo-clearing-protocol.md` and `docs/design-decisions.md` for the full reasoning.

## System architecture

```mermaid
graph TD
    subgraph on-chain["On-chain (Pinocchio program)"]
        P[Clearing engine\nOrder slab · Histogram · ClearingResult]
        M[Money & risk layer\nCollateral · Funding · Liquidation · Cross-margin]
    end

    subgraph crates["Off-chain (Rust crates/)"]
        K[tempo-keeper\nCrank driver]
        MM[tempo-mm-bot\nReference market maker]
        LQ[tempo-liquidator\nReference liquidator]
        API[tempo-api\nREST + WebSocket]
    end

    K -->|process_chunk · finalize_clear · settle_fill| P
    MM -->|submit_order · update_maker_quote| P
    LQ -->|liquidate · liquidate_cross| M
    API -->|reads state| P
    API -->|reads state| M
```

## Layout

| Path | What |
|---|---|
| `program/` | Pinocchio on-chain program — clearing engine + perpetuals money/risk layer |
| `tests/integration-tests/` | LiteSVM end-to-end + property tests |
| `trident-tests/` | Transaction-level fuzzing (Trident), excluded from the main workspace |
| `clients/typescript/` | Codama-generated TypeScript SDK (`just generate-clients`) |
| `idl/tempo_program.json` | Codama IDL (written by `program/build.rs`) |
| `crates/` | Rust off-chain services (see below), including `crates/sim` — the devnet simulation package |
| `apps/web/` | Next.js devnet trading dApp — demo/testing UI, not a production client |
| `docs/` | Design docs + benchmark artifacts |
| `ops/` | Docker, Compose, systemd, CI |

### `crates/` — Rust off-chain stack

| Crate | What |
|---|---|
| `tempo-math` | `no_std` mirror of the program's pure math — oracle reader, margin/liquidation, clearing, wide arithmetic |
| `common` | RPC pool with 429 failover, priority-fee tx sender, backoff, config, telemetry |
| `sdk` | `TempoClient` — typed account decoders, PDA helpers, instruction builders, `benign` race classifier |
| `keeper` | Stateless crank driver — pure `decide()` state machine drives `process_chunk` / `finalize_clear` / `settle_fill` / `start_auction` |
| `api` | Axum REST + WebSocket read API — `ArcSwap`-backed live state, no per-request RPC |
| `mm-bot` | Reference market maker — oracle-anchored, inventory-skewed maker-quote ladder |
| `liquidator` | Reference liquidator — `getProgramAccounts` scan, pure engine gates mirror on-chain math |
| `bench` | Host micro-benchmarks proving O(ticks) clearing; output in `docs/bench/` |

## Quickstart

**Prerequisites:** Rust (see `rust-toolchain.toml`), `cargo-build-sbf`, Solana CLI, Node (see `.nvmrc`), pnpm.

```bash
# On-chain program
just build             # cargo-build-sbf → target/deploy/tempo_program.so
just unit-test         # clearing math + state serde (201 tests, 2 ignored)
just integration-test  # LiteSVM end-to-end suite (needs a built .so)
just benchmark         # CU profile → docs/bench/cu_report.md

# Off-chain Rust services
cargo build -p tempo-keeper
cargo build -p tempo-mm-bot
cargo build -p tempo-liquidator
cargo build -p tempo-api

# TypeScript client
pnpm install
just generate-clients  # IDL → clients/rust + clients/typescript
```

## Devnet

Program deployed to **Solana devnet** at `8gpzMDNnKNz422jW3hs54TRmZK2H5uEwgfEQbjWAwnJD`, integrating the live **Pyth SOL/USD** push feed (`7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE`).

The `read_oracle` instruction parses `PriceUpdateV2` on-chain (owner + feed-id + staleness + confidence checks) and derives the mark price. The full perpetuals money path — collateral custody, deposit/withdraw, oracle-priced funding, liquidation, cross-margin — is live and was exercised against the real Pyth feed.

All three Rust off-chain services (`tempo-keeper`, `tempo-mm-bot`, `tempo-liquidator`) have been smoke-tested against live devnet.

```bash
# Redeploy after program changes
cargo-build-sbf
solana program deploy target/deploy/tempo_program.so \
  --program-id target/deploy/tempo_program-keypair.json

# Run the Rust services (requires TEMPO_RPC_URL, TEMPO_KEYPAIR, TEMPO_MARKET in env or tempo.toml)
just keeper
just mm
just liq
```

## Status

**Working today:**

- The **clearing engine** — dual auction (bid + ask), three-phase ACCUMULATE → DISCOVER → SETTLE, auction lifecycle, CPI events
- **Scaling**: the order book is sharded across independent accounts for parallel
  submission/settlement, unfilled orders carry forward as resting orders instead of
  being discarded, and submission is always-open (no dead time between rounds); a
  separate `MakerQuote` book lets market makers post a standing ladder instead of
  one-shot orders (`docs/plan.md`, `docs/design-decisions.md`)
- The **money/risk layer** (`docs/risk-model.md`) — SPL collateral custody, deposit/withdraw, oracle-priced funding and liquidation, insurance fund, open-interest tracking, socialized-loss/ADL, hard solvency gate, per-slot price brake, oracle soft-stale fallback, overflow-safe notional math, cross-margin
- **Verification** — LiteSVM end-to-end suites including randomized multi-round and liquidation stress tests, property fuzzes, and formal proofs (`cargo kani`); see `docs/verification.md` for the full invariant → test matrix
- **Off-chain Rust stack** — keeper, market maker, liquidator, read API — all smoke-tested on live devnet
- **Deployed to devnet** at `8gpzMDNnKNz422jW3hs54TRmZK2H5uEwgfEQbjWAwnJD`; the money path and full auction lifecycle are exercised against the real Pyth feed. In-place account migration handles layout upgrades.

**Still open before mainnet:**

- **Round-processing overlap** — the order-book sharding and throughput questions
  raised in `system-design.md §7` are now measured, not open (`docs/bench/cu_report.md`:
  16 shards × 90-order cap ⇒ ~160,542 CU finalize, 11.5% of the 1.4M CU/tx cap). What
  remains is true overlap between rounds (processing round N+1 while round N is still
  settling) — designed but deliberately not built, gated behind a benchmark showing
  it's actually needed (`docs/known-issues.md §2.14`)
- **Stage-B marketable-fill on a live chain** — the resting-order fill-after-recenter
  path is proven at the unit level, not yet end-to-end against a live counterparty on
  devnet (`docs/known-issues.md §2.13`)
- **Dual-auction end-to-end on devnet** — implemented and tested in LiteSVM; not yet driven fully on live devnet
- **Indexer + web UI** — deferred; API history endpoints return 501 until the indexer lands
- **Economic hardening** — batch-perp funding stability, true OI-netted PnL (see `docs/risk-model.md`)

Known issues and open design questions are tracked in [`docs/known-issues.md`](docs/known-issues.md) and [`docs/missing-features.md`](docs/missing-features.md).

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

MIT — see [`LICENSE`](./LICENSE).
