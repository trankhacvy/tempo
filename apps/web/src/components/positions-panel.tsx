"use client";

import { useWallet } from "@solana/wallet-adapter-react";
import { useCallback, useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { DataRow } from "@/components/data-row";
import { EmptyHint, SkeletonRows } from "@/components/ui/skeleton";
import { StatusMessage, type TxStatus } from "@/components/ui/status-message";
import { fetchPositionView, type MarketView, type PositionView } from "@/lib/data";
import { buildInitPositionIx } from "@/lib/instructions";
import {
    equity as computeEquity,
    liquidationPrice,
    markPrice as computeMarkPrice,
    unrealizedPnl,
} from "@/lib/tempo-math";
import { sendInstructions } from "@/lib/tx";
import { useInterval } from "@/lib/use-interval";
import { useWalletSigner } from "@/lib/use-wallet-signer";
import { cn } from "@/lib/utils";

const POLL_MS = 3000;

function signClass(v: bigint): string {
    if (v > 0n) return "text-up";
    if (v < 0n) return "text-down";
    return "text-foreground";
}

export function PositionsPanel({ view }: { view: MarketView | null }) {
    const { publicKey, connected } = useWallet();
    const signer = useWalletSigner();
    const owner = publicKey?.toBase58() ?? null;
    const market = view?.address ?? null;

    const [position, setPosition] = useState<PositionView | null>(null);
    const [exists, setExists] = useState<boolean | null>(null);
    const [status, setStatus] = useState<TxStatus>({ kind: "idle" });

    const refresh = useCallback(async () => {
        if (!owner || !market) {
            setPosition(null);
            setExists(null);
            return;
        }
        try {
            const p = await fetchPositionView(owner, market);
            setPosition(p);
            setExists(p !== null);
        } catch {
            setPosition(null);
            setExists(null);
        }
    }, [owner, market]);

    // Initial fetch on mount / when owner or market changes.
    useEffect(() => {
        void refresh();
    }, [refresh]);

    useInterval(() => void refresh(), owner && market ? POLL_MS : null);

    const busy = status.kind === "pending";

    const initPosition = useCallback(async () => {
        if (!signer || !owner || !market) {
            setStatus({ kind: "error", message: "Connect a wallet and load a market first." });
            return;
        }
        setStatus({ kind: "pending" });
        try {
            const ix = await buildInitPositionIx(owner, market);
            const { signature } = await sendInstructions(signer, [ix], (sig) =>
                setStatus({ kind: "pending", signature: sig }),
            );
            setStatus({ kind: "success", signature });
            await refresh();
        } catch (e) {
            setStatus({ kind: "error", message: e instanceof Error ? e.message : String(e) });
        }
    }, [signer, owner, market, refresh]);

    if (!connected || !owner) {
        return (
            <div className="p-4">
                <p className="text-sm text-muted-foreground">Connect a wallet to view your position.</p>
            </div>
        );
    }

    const mark = view ? computeMarkPrice(view.lastBidFillPrice, view.lastAskFillPrice) : null;

    return (
        <div className="space-y-4 p-4">
            {!market ? (
                <EmptyHint title="No market loaded">
                    Configure <code className="font-mono">NEXT_PUBLIC_TEMPO_MARKET</code> or load a
                    market in the Market tab to view your position.
                </EmptyHint>
            ) : exists === false ? (
                <div className="space-y-3">
                    <p className="text-sm text-muted-foreground">
                        No position account exists for this wallet in this market yet.
                    </p>
                    <Button disabled={busy || !market} onClick={() => void initPosition()}>
                        {busy ? "Working…" : "Init position"}
                    </Button>
                </div>
            ) : position ? (
                <PositionDetail
                    position={position}
                    mark={mark}
                    maintenanceBps={view?.maintenanceMarginBps ?? 0}
                />
            ) : (
                <SkeletonRows rows={6} />
            )}
            <StatusMessage status={status} />
        </div>
    );
}

function PositionDetail({
    position,
    mark,
    maintenanceBps,
}: {
    position: PositionView;
    mark: bigint | null;
    maintenanceBps: number;
}) {
    const { size, entryPrice, collateral, realizedPnl } = position;
    const flat = size === 0n;
    const sideLabel = flat ? "Flat" : size > 0n ? "Long" : "Short";

    const upnl = mark !== null ? unrealizedPnl(size, entryPrice, mark) : null;
    const eq = upnl !== null ? computeEquity(collateral, realizedPnl, upnl) : null;
    const liq = liquidationPrice(size, entryPrice, collateral, realizedPnl, maintenanceBps);

    return (
        <div className="space-y-1">
            <DataRow label="Side">
                <span className={cn(flat ? "text-muted-foreground" : size > 0n ? "text-up" : "text-down")}>
                    {sideLabel}
                </span>
            </DataRow>
            <DataRow label="Size">{size.toString()}</DataRow>
            <DataRow label="Entry">{entryPrice.toString()}</DataRow>
            <DataRow label="Mark">{mark !== null ? mark.toString() : "—"}</DataRow>
            <DataRow label="Collateral">{collateral.toString()}</DataRow>
            <DataRow label="Realized PnL">
                <span className={signClass(realizedPnl)}>{realizedPnl.toString()}</span>
            </DataRow>
            <DataRow label="Unrealized PnL">
                {upnl !== null ? <span className={signClass(upnl)}>{upnl.toString()}</span> : "—"}
            </DataRow>
            <DataRow label="Equity">{eq !== null ? eq.toString() : "—"}</DataRow>
            <DataRow label="Liq. price">{liq !== null ? liq.toString() : "—"}</DataRow>
        </div>
    );
}
