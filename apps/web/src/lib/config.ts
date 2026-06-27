// Devnet-only configuration. Never targets mainnet.
export const RPC_URL: string =
    process.env.NEXT_PUBLIC_SOLANA_RPC_URL?.trim() || "https://api.devnet.solana.com";

export const CLUSTER = "devnet" as const;

export const DEFAULT_MARKET: string = process.env.NEXT_PUBLIC_TEMPO_MARKET?.trim() ?? "";

export const COLLATERAL_MINT: string = process.env.NEXT_PUBLIC_COLLATERAL_MINT?.trim() ?? "";

export const VAULT_TOKEN_ACCOUNT: string =
    process.env.NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT?.trim() ?? "";

export const USER_TOKEN_ACCOUNT: string =
    process.env.NEXT_PUBLIC_USER_TOKEN_ACCOUNT?.trim() ?? "";

export const PROGRAM_ID = "8gpzMDNnKNz422jW3hs54TRmZK2H5uEwgfEQbjWAwnJD";
