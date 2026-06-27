"use client";

import { useWallet } from "@solana/wallet-adapter-react";
import { useMemo } from "react";

import type { WalletSigner } from "./tx";

/** Adapt the connected wallet-adapter wallet into the WalletSigner shape used
 *  by lib/tx.ts. Returns null when no wallet is connected or it can't sign. */
export function useWalletSigner(): WalletSigner | null {
    const { publicKey, signTransaction } = useWallet();
    return useMemo(() => {
        if (!publicKey || !signTransaction) return null;
        return { publicKey, signTransaction };
    }, [publicKey, signTransaction]);
}
