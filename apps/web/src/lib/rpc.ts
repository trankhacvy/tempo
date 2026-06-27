import { createSolanaRpc, type Rpc, type SolanaRpcApi } from "@solana/kit";

import { RPC_URL } from "./config";

// Shared read-only RPC client (@solana/kit) used for account fetches/decoding.
// Transaction signing/sending goes through the wallet-adapter + web3.js path
// (see lib/tx.ts) because Wallet-Standard wallets sign web3.js transactions.
let cached: Rpc<SolanaRpcApi> | null = null;

export function getRpc(): Rpc<SolanaRpcApi> {
    if (cached === null) cached = createSolanaRpc(RPC_URL);
    return cached;
}
