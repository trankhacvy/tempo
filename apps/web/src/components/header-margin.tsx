"use client";

import { useWallet } from "@solana/wallet-adapter-react";
import { useCallback, useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { COLLATERAL_MINT } from "@/lib/config";
import { fetchCollateralView } from "@/lib/data";
import { requestFaucet } from "@/lib/faucet";
import { buildDepositIx, buildInitCollateralIx } from "@/lib/instructions";
import { sendInstructions } from "@/lib/tx";
import { useInterval } from "@/lib/use-interval";
import { useWalletSigner } from "@/lib/use-wallet-signer";
import { formatUnits } from "@/lib/utils";

const DECIMALS = 6;
const GRANT = 1_000_000_000n; // 1000 devUSDC (6 decimals)
const POLL_MS = 5000;

/** Header widget: the wallet's free margin + a one-click "Get free devUSDC" that
 *  mints 1000 test collateral (server faucet) and deposits it into the ledger so it
 *  is immediately usable as margin. */
export function HeaderMargin() {
    const { publicKey } = useWallet();
    const signer = useWalletSigner();
    const owner = publicKey?.toBase58() ?? null;

    const [free, setFree] = useState<bigint | null>(null);
    const [exists, setExists] = useState(false);
    const [busy, setBusy] = useState(false);
    const [err, setErr] = useState<string | null>(null);

    const enabled = Boolean(owner && COLLATERAL_MINT);

    const refresh = useCallback(async () => {
        if (!enabled || !owner) {
            setFree(null);
            return;
        }
        try {
            const c = await fetchCollateralView(owner);
            setFree(c?.free ?? null);
            setExists(c !== null);
        } catch {
            // keep last good
        }
    }, [enabled, owner]);

    useEffect(() => {
        void refresh();
    }, [refresh]);
    useInterval(() => void refresh(), enabled ? POLL_MS : null);

    const getDevUsdc = useCallback(async () => {
        if (!signer || !owner) return;
        setBusy(true);
        setErr(null);
        try {
            await requestFaucet(owner); // mint 1000 devUSDC + dust SOL (server-signed)
            const ixs = [];
            if (!exists) ixs.push(await buildInitCollateralIx(owner));
            ixs.push(await buildDepositIx(owner, GRANT));
            await sendInstructions(signer, ixs, () => undefined);
            await refresh();
        } catch (e) {
            setErr(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }, [signer, owner, exists, refresh]);

    if (!enabled) return null;

    return (
        <div className="flex items-center gap-2">
            <span className="hidden font-mono text-xs tabular-nums text-muted-foreground sm:inline">
                Free margin{" "}
                <span className="text-foreground">
                    ${free !== null ? formatUnits(free, DECIMALS) : "0.00"}
                </span>
            </span>
            <Button
                size="sm"
                variant="outline"
                className="h-7 text-xs"
                disabled={busy}
                onClick={() => void getDevUsdc()}
                title={err ?? "Mint 1000 devUSDC and deposit it as margin"}
            >
                {busy ? "Working…" : "Get free devUSDC"}
            </Button>
        </div>
    );
}
