# Tempo Web (devnet dApp)

A Next.js (App Router, TypeScript, Tailwind v4, shadcn-style UI) trading frontend for the
Tempo Dual Flow Batch Auction perpetuals DEX, talking to the deployed program on
**devnet only** (`8gpzMDNnKNz422jW3hs54TRmZK2H5uEwgfEQbjWAwnJD`). It is a demo / testing UI,
not a production trading client.

## What the app does

- **Collateral + leverage trade entry** — pick a side and leverage; the panel derives a
  marketable order from your USD collateral and the live oracle price (an "advanced" mode
  exposes raw price/quantity in ticks and base units). Orders go into the current auction.
- **Live Pyth price chart** — streams the SOL/USD Pyth feed into a lightweight-charts candle
  view, anchored to the same oracle the program reads.
- **Positions panel with live PnL** — your perp position marked against the current mark
  price: size, entry, collateral, realized/unrealized PnL, equity, and liquidation price.
- **Collateral panel** — your UserCollateral ledger (balance / locked / free) with
  deposit / withdraw, plus a devnet faucet hint when your balance is zero.
- **Auction status strip** — the current auction id, phase (`Collect → Accumulating →
  Discovered → Settling`), a slot-based countdown to the phase deadline, and the last
  clearing prices.
- **Activity feed** — recent on-chain `OrderSubmitted` / `ClearingFinalized` / `FillSettled`
  events, decoded from the program's self-CPI event data (`lib/events.ts`), each linking to
  the devnet explorer.

## Wallet stack

`@solana/wallet-adapter-react` + `@solana/wallet-adapter-react-ui` (Wallet-Standard
auto-discovery) for connection and signing, `@solana/kit` + the Codama-generated client for
building instructions and reading accounts, and `@solana/web3.js` only as the signing/sending
boundary (Wallet-Standard wallets sign web3.js transactions). A dev-only burner adapter
(`lib/burner-adapter.ts`, gated behind `NEXT_PUBLIC_USE_BURNER`) can sign in-page with a local
keypair for automation — never enable it against mainnet.

## Generated client

The Codama client is bundled into `src/vendor/tempo-client.mjs` via esbuild
(`pnpm bundle-client`, run automatically on `predev`/`prebuild`). Types come from
`src/vendor/tempo-client.d.mts`.

### Important: account decoding

The generated Codama account decoders model only a **1-byte** discriminator, but the program's
zero-copy accounts have a **2-byte** prefix (discriminator + version). The generated decoders
are therefore off-by-one and produce garbage (verified on devnet: phase=67, tickSize=256 for a
market that is actually phase=3, tickSize=1). `src/lib/data.ts` decodes Market / UserCollateral
/ Position by raw little-endian byte offset, matching the authoritative layout in
`tests/integration-tests/src/lib.rs`. The generated **instruction builders, PDA finders, and
event decoders are correct** and are used as-is.

## Demo setup

The UI only shows live data once it points at a real devnet market and that market's orders
actually clear. Three steps:

### 1. Create (or point at) a devnet SOL/USD market

Provisioning now goes through the Rust `tempo-sim` package (`crates/sim/` — see its own
README for full config), not a TypeScript bots package. From the repo root:

```bash
export TEMPO_RPC_URL=https://api.devnet.solana.com
export TEMPO_SIM_MASTER_KEYPAIR=./keys/master.json   # a funded devnet keypair
export TEMPO_SIM_MAINT_BPS=0                         # 0 = clearing-only; >0 = full money path
cargo run -p tempo-sim --bin tempo-sim-provision
```

This is idempotent (safe to re-run) and writes `sim-artifact.json`, whose `market`,
`collateral_mint`, and `vault_token_account` fields are the addresses you need for
`.env.local` below. Everything is devnet-only.

### 2. Configure `.env.local`

Copy `.env.example` to `.env.local` and fill it in:

```bash
cp apps/web/.env.example apps/web/.env.local
```

- `NEXT_PUBLIC_SOLANA_RPC_URL` — devnet RPC (default `https://api.devnet.solana.com`).
- `NEXT_PUBLIC_TEMPO_MARKET` — the `market` address from `sim-artifact.json` (step 1).
- `NEXT_PUBLIC_COLLATERAL_MINT`, `NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT` — the `collateral_mint`
  and `vault_token_account` fields from `sim-artifact.json` (only present when
  `TEMPO_SIM_MAINT_BPS > 0`, i.e. the money path was provisioned).
- `NEXT_PUBLIC_USER_TOKEN_ACCOUNT` — the connecting wallet's own token account for the
  collateral mint (its associated token account); create it with standard SPL tooling if
  it doesn't exist yet.
- `NEXT_PUBLIC_USE_BURNER` / `NEXT_PUBLIC_BURNER_SECRET` — optional dev-only in-page signer.

### 3. Run the crank so orders clear and fill

Submitted orders only clear when someone runs the permissionless crank (ACCUMULATE → DISCOVER
→ SETTLE). Run the keeper against the same market so the auction advances and fills land
(and show up in the Activity feed):

```bash
just keeper   # or: cargo run -p tempo-keeper
```

Then start the web app:

```bash
pnpm --filter @tempo/web dev
```

## Scripts

- `pnpm --filter @tempo/web dev` — dev server (rebuilds the client bundle first).
- `pnpm --filter @tempo/web build` — production build (type-checked).
- `pnpm --filter @tempo/web devnet-read [marketAddress]` — read-only devnet smoke test.
