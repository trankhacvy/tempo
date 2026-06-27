"use client";

import { useConnection, useWallet } from "@solana/wallet-adapter-react";
import { WalletMultiButton } from "@solana/wallet-adapter-react-ui";
import { useCallback, useEffect, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { explorerAddressUrl } from "@/lib/tx";
import { shortenAddress } from "@/lib/utils";

export function WalletBar() {
    const { connection } = useConnection();
    const { publicKey, connected } = useWallet();
    const [balance, setBalance] = useState<number | null>(null);

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
        <div className="flex flex-wrap items-center gap-3">
            <Badge variant="muted">devnet</Badge>
            {connected && publicKey ? (
                <div className="flex items-center gap-3 text-sm">
                    <a
                        href={explorerAddressUrl(publicKey.toBase58())}
                        target="_blank"
                        rel="noreferrer"
                        className="font-mono text-foreground underline-offset-2 hover:underline"
                    >
                        {shortenAddress(publicKey.toBase58())}
                    </a>
                    <span className="text-muted-foreground">
                        {balance === null ? "…" : `${balance.toFixed(4)} SOL`}
                    </span>
                </div>
            ) : null}
            <WalletMultiButton />
        </div>
    );
}
