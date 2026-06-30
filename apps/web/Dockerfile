# Tempo web dApp (Next.js 15). Built + pushed to GHCR by .github/workflows/sim.yml.
#
# NEXT_PUBLIC_* are inlined at BUILD time, so the target market is baked into the
# image — set them as GitHub repo Variables and rebuild (CI) when the market changes.
# NEXT_PUBLIC_SOLANA_RPC_URL is shipped to the browser, so use a PUBLIC devnet RPC
# here, never a paid/secret key.
FROM node:20-bookworm AS build
WORKDIR /repo
RUN corepack enable
COPY . .
RUN pnpm install --frozen-lockfile
# Generate the TS client from the committed IDL (no Rust toolchain needed).
RUN pnpm generate-clients

ARG NEXT_PUBLIC_SOLANA_RPC_URL=https://api.devnet.solana.com
ARG NEXT_PUBLIC_TEMPO_MARKET=
ARG NEXT_PUBLIC_COLLATERAL_MINT=
ARG NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT=
ENV NEXT_PUBLIC_SOLANA_RPC_URL=$NEXT_PUBLIC_SOLANA_RPC_URL \
    NEXT_PUBLIC_TEMPO_MARKET=$NEXT_PUBLIC_TEMPO_MARKET \
    NEXT_PUBLIC_COLLATERAL_MINT=$NEXT_PUBLIC_COLLATERAL_MINT \
    NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT=$NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT
RUN pnpm --filter @tempo/web build

FROM node:20-bookworm-slim AS runtime
RUN corepack enable
ENV NODE_ENV=production
COPY --from=build /repo /repo
WORKDIR /repo/apps/web
EXPOSE 3000
# `next start` serves the prebuilt .next on port 3000.
CMD ["pnpm", "start"]
