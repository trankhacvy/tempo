# `ops/` — deploy & operations

Production-grade running of the Tempo off-chain services (Phase 3, build-plan §2.11).
Everything here is **devnet-first** and **secret-free in git**: keypairs and RPC
endpoints arrive at runtime only.

## Layout

```
docker/Dockerfile          multi-stage build, one image per service (BIN arg)
compose/docker-compose.yml  redundant services + Prometheus + Grafana (devnet)
compose/.env.example        copy to .env (git-ignored) and fill in
compose/keys/               per-service keypairs (git-ignored)
compose/prometheus.yml      scrape config
compose/alerts.yml          alert rules (freeze, insurance, rpc, funding)
compose/grafana/            datasource + dashboard provisioning
systemd/*.service           templated units (the non-container path)
../.github/workflows/       CI (fmt/clippy/test/clients-fresh/kani) + image publish
```

## Quick start (compose, devnet)

```bash
cd ops/compose
cp .env.example .env                     # set TEMPO_RPC_URL (devnet) + TEMPO_MARKETS
# drop keypairs into ./keys: keeper-a.json keeper-b.json liquidator-a.json
#                            liquidator-b.json mm-bot.json
docker compose build
docker compose up -d
# Grafana → http://localhost:3000  (anonymous viewer), Prometheus → :9090
# API     → http://localhost:8088
```

Each image is one binary selected by the `BIN` build-arg
(`tempo-keeper | tempo-liquidator | tempo-api | tempo-mm-bot`). Build one directly:

```bash
docker build -f ops/docker/Dockerfile --build-arg BIN=tempo-liquidator -t tempo/liquidator ../..
```

## Redundancy (Decision D5 — free)

The keeper and liquidator hold no must-persist state: each tick/scan reconstructs
from chain. Two replicas racing the same work is safe — the loser gets a benign
`NotLiquidatable` / `already processed` / wrong-phase error. The compose file runs
**two keepers and two liquidators** by default. With systemd, enable templated
instances:

```bash
systemctl enable --now tempo-keeper@a tempo-keeper@b
systemctl enable --now tempo-liquidator@a tempo-liquidator@b
```

## Secrets & the signer seam (Decision D6)

- Keys are **never** in an image or in git. Compose mounts `./keys` read-only;
  systemd reads `EnvironmentFile=/etc/tempo/<svc>-<instance>.env`. `.gitignore`
  covers `ops/compose/.env` and `ops/compose/keys/*.json`.
- Every service signs through `tempo_common::signer::TempoSigner`. Today the file
  impl loads `TEMPO_KEYPAIR`. The KMS/Vault upgrade is a new `TempoSigner` impl
  selected by env — **no service code changes** (they already sign through the
  trait). Rotate by swapping the mounted key / KMS key id and restarting the unit.
- The **API carries no signing key** — it is read-only.

## Monitoring & paging

Prometheus scrapes every service's `/metrics` (the keeper/liquidator/mm-bot expose
it on their health port; the API on its bind port). Alert rules (`alerts.yml`):

| Alert                | Condition                                            | Severity |
|----------------------|------------------------------------------------------|----------|
| `KeeperFrozen`       | `keeper_slots_since_progress > 300`                  | page     |
| `AllKeepersDown`     | no keeper replica `up`                               | page     |
| `LiquidatorStalled`  | `rate(liquidator_scan_errors_total[5m]) > 0.2`       | page     |
| `LiquidatorDown`     | no liquidator replica `up`                           | page     |
| `InsuranceLow`       | `liquidator_insurance_balance` below floor           | warn     |
| `FundingStale`       | `keeper_funding_age_seconds > 300`                   | warn     |

The Grafana dashboard (`Tempo — clearing & risk`) shows the auction-phase timeline,
freeze headroom, settle latency, liquidations by result, the underwater count, and
the insurance gauge.

## ⚠️ Never point bot services at mainnet

`TEMPO_RPC_URL` in `.env` must be a **devnet** endpoint. The mainnet pool stays
commented out in `.env.example` (see the repo `CLAUDE.md`). Only the read-only API
is safe to run against any cluster.

## Scaling

- Keeper / liquidator: add more replicas (commutative-safe). Give each its own
  keypair so they don't share a nonce / rate limit.
- API: stateless reader — run several behind a load balancer.
- The `getProgramAccounts`-based liquidator scan is bounded per market; when the
  indexer (deferred Phase 2 item) lands, the liquidator's `PositionSource` swaps to
  it with no other change.

## Known limitations

- **Market-maker depth (known-issues §4.9):** each keypair can only hold one
  `MakerQuote` PDA per market. To get wider ladder depth run multiple `tempo-mm-bot`
  instances with separate keypairs:
  ```bash
  # In docker-compose.yml, duplicate mm-bot with a different keypair mount:
  # environment: { TEMPO_KEYPAIR: /keys/mm-bot-b.json }
  ```
  A multi-quote-per-maker design requires a program change and is tracked in
  `docs/known-issues.md §4.9`.
