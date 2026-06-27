"use client";

import { ConnectionProvider, WalletProvider, useWallet } from "@solana/wallet-adapter-react";
import { WalletModalProvider } from "@solana/wallet-adapter-react-ui";
import type { Adapter } from "@solana/wallet-adapter-base";
import { useEffect, useMemo, type ReactNode } from "react";

import { RPC_URL } from "@/lib/config";
import { BurnerWalletAdapter, parseBurnerSecret } from "@/lib/burner-adapter";

import "@solana/wallet-adapter-react-ui/styles.css";

const USE_BURNER = process.env.NEXT_PUBLIC_USE_BURNER?.trim() === "1";

/** When the burner is enabled, auto-select it as soon as it appears in the wallets list. */
function BurnerAutoConnect() {
    const { wallets, select, connected, connecting } = useWallet();
    useEffect(() => {
        if (!USE_BURNER || connected || connecting) return;
        const burner = wallets.find((w) => w.adapter.name === "Burner (devnet)");
        if (burner) select(burner.adapter.name as Parameters<typeof select>[0]);
    }, [wallets, select, connected, connecting]);
    return null;
}

export function AppWalletProvider({ children }: { children: ReactNode }) {
    const wallets = useMemo<Adapter[]>(() => {
        const secret = process.env.NEXT_PUBLIC_BURNER_SECRET?.trim();
        if (USE_BURNER && secret) {
            try {
                return [new BurnerWalletAdapter(parseBurnerSecret(secret))];
            } catch {
                return [];
            }
        }
        return [];
    }, []);

    return (
        <ConnectionProvider endpoint={RPC_URL}>
            <WalletProvider wallets={wallets} autoConnect>
                <WalletModalProvider>
                    <BurnerAutoConnect />
                    {children}
                </WalletModalProvider>
            </WalletProvider>
        </ConnectionProvider>
    );
}
