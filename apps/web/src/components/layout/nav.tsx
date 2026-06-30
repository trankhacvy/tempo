"use client";

import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import { WalletMultiButton } from "@solana/wallet-adapter-react-ui";
import { useCallback, useEffect, useState } from "react";

import { Wordmark } from "@/components/brand/logo";
import { HeaderMargin } from "@/components/header-margin";
import { Separator } from "@/components/ui/separator";

export function Nav() {
    const { connection } = useConnection();
    const { publicKey } = useWallet();
    const [balance, setBalance] = useState<number | null>(null);
    const [mounted, setMounted] = useState(false);

    useEffect(() => setMounted(true), []);

    const refresh = useCallback(async () => {
        if (!publicKey) {
            setBalance(null);
            return;
        }
        try {
            const lamports = await connection.getBalance(publicKey, "confirmed");
            setBalance(lamports / 1_000_000_000);
        } catch {
            setBalance(null);
        }
    }, [connection, publicKey]);

    useEffect(() => {
        void refresh();
        const id = setInterval(() => void refresh(), 15_000);
        return () => clearInterval(id);
    }, [refresh]);

    return (
        <header className="flex h-11 shrink-0 items-center gap-3 border-b border-border px-4">
            <a href="#top" className="shrink-0">
                <Wordmark />
            </a>

            <Separator orientation="vertical" className="h-5" />
            <span className="text-sm font-semibold">SOL-PERP</span>

            <div className="ml-auto flex items-center gap-3">
                <span className="hidden items-center gap-1.5 border border-border bg-secondary/40 px-2 py-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground sm:inline-flex">
                    <span className="h-1.5 w-1.5 animate-ticker bg-up" />
                    devnet
                </span>
                {mounted && <HeaderMargin />}
                {publicKey && (
                    <span className="hidden font-mono text-xs tabular-nums text-muted-foreground lg:inline">
                        {balance === null ? "…" : `${balance.toFixed(3)} SOL`}
                    </span>
                )}
                {mounted && <WalletMultiButton />}
            </div>
        </header>
    );
}
