# tempo-sim — devnet simulation

Synthetic, on-chain activity so the Tempo UI has a live market to render before
real participants arrive. **This is a devnet simulation, not organic volume.** The
order flow is generated; the *price* is real (the market's tick window recenters on
the live Pyth SOL/USD feed each round, so clearing prices track real SOL).

It reuses the existing reference services end-to-end — `tempo-keeper` (cranker),
`tempo-mm-bot` (liquidity), `tempo-liquidator` (risk backstop) — and adds only the
missing party (**traders**) plus a one-shot **provisioner**.

## Binaries

- `tempo-sim-provision` — one-shot. Creates the market and (Phase B) the money path,
  funds + initializes all agent accounts from a master keypair, writes
  `sim-artifact.json`. Idempotent: safe to re-run after a devnet reset.
- `tempo-sim` — one trader loop. Phase-aware (acts only in `Collect`), one order set
  per round, sizes within margin. `--once` runs a single tick and exits.
- `tempo-sim-orchestrator` — runs the keeper + market makers + liquidator + trader
  fleet in one process against an existing artifact (local `cargo run` demo).

## Two phases

- **Phase A (clearing-only):** `TEMPO_SIM_MAINT_BPS=0`, no collateral mint. Live book,
  both auctions crossing, real clearing-price chart, trade feed. No positions/PnL/
  liquidations. Fastest path to a live UI, lowest risk.
- **Phase B (money path):** `TEMPO_SIM_MAINT_BPS>0`. Adds a fake-USDC collateral mint,
  vault, per-agent deposits, positions, PnL, and liquidations (the `reckless` persona
  builds toward liquidation). Validate the unit scaling (see
  `docs/risk-model.md §1`, the single-base-unit assumption) with one hand-opened
  position before scaling the fleet.

## Local run

```bash
# fund a master keypair on devnet first, then:
export TEMPO_RPC_URL=https://api.devnet.solana.com
export TEMPO_SIM_MASTER_KEYPAIR=./keys/master.json
export TEMPO_SIM_MAINT_BPS=0          # Phase A
cargo run -p tempo-sim --bin tempo-sim-provision

# point the orchestrator at the artifact it wrote:
cargo run -p tempo-sim --bin tempo-sim-orchestrator
```

## Devnet deployment

See `ops/compose/docker-compose.sim.yml` and `ops/compose/.env.sim.example`.

## Config (`TEMPO_SIM_*`)

Trader: `PERSONA` (noise|momentum|passive|reckless), `SEED`, `POLL_MS`, `BASE_SIZE`,
`AGGRESSION_TICKS`, `INNER_SPREAD_TICKS`, `MAX_ORDERS` (≤8). Provisioner: `NUM_TRADERS`,
`NUM_MM`, `MAINT_BPS`, `INITIAL_BPS`, `PENALTY_BPS`, `TICK_SIZE`, `NUM_TICKS`, `CAP`,
`DEPOSIT`, `FUND_LAMPORTS`, `MASTER_KEYPAIR`, `KEYS_DIR`, `ARTIFACT`, `ORACLE`.
