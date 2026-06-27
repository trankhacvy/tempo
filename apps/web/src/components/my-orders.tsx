"use client";

import { useCallback, useEffect, useState } from "react";
import { useWallet } from "@solana/wallet-adapter-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { EmptyHint, SkeletonRows } from "@/components/ui/skeleton";
import { StatusMessage, type TxStatus } from "@/components/ui/status-message";
import { fetchMyOrders, type OrderView } from "@/lib/auction";
import { type MarketView } from "@/lib/data";
import { buildCancelOrderIx } from "@/lib/instructions";
import { sendInstructions } from "@/lib/tx";
import { useInterval } from "@/lib/use-interval";
import { useWalletSigner } from "@/lib/use-wallet-signer";
import { cn } from "@/lib/utils";

const POLL_MS = 3000;

interface MyOrdersProps {
    market: string | null;
    view: MarketView | null;
}

function statusLabel(status: 0 | 1 | 2 | 3): string {
    switch (status) {
        case 1: return "Resting";
        case 2: return "Accumulated";
        default: return "Unknown";
    }
}

function statusVariant(status: 0 | 1 | 2 | 3): "muted" | "default" {
    return status === 1 ? "muted" : "default";
}

function formatPrice(order: OrderView): string {
    if (order.priceUsd !== null) return `$${order.priceUsd.toFixed(2)}`;
    return order.price.toString();
}

export function MyOrders({ market, view }: MyOrdersProps) {
    const { publicKey } = useWallet();
    const signer = useWalletSigner();
    const walletAddress = publicKey?.toBase58() ?? null;

    const [orders, setOrders] = useState<OrderView[]>([]);
    const [loading, setLoading] = useState(false);
    const [cancelStatus, setCancelStatus] = useState<TxStatus>({ kind: "idle" });

    const tickSize = view?.tickSize ?? 0n;
    const active = !!market && !!walletAddress;

    const refresh = useCallback(async () => {
        if (!active) {
            setOrders([]);
            return;
        }
        try {
            const result = await fetchMyOrders(market!, walletAddress!, tickSize);
            setOrders(result);
        } catch {
            // silent
        } finally {
            setLoading(false);
        }
    }, [market, walletAddress, tickSize]);

    useEffect(() => {
        if (!active) {
            setOrders([]);
            return;
        }
        setLoading(true);
        void refresh();
    }, [active, refresh]);

    useInterval(() => void refresh(), active ? POLL_MS : null);

    async function cancel(order: OrderView) {
        if (!signer || !walletAddress || !market) {
            setCancelStatus({ kind: "error", message: "Wallet not connected." });
            return;
        }
        setCancelStatus({ kind: "pending" });
        try {
            const ix = await buildCancelOrderIx(
                walletAddress,
                market,
                order.orderId,
                order.slotIndex,
            );
            const { signature } = await sendInstructions(signer, [ix], (sig) =>
                setCancelStatus({ kind: "pending", signature: sig }),
            );
            setCancelStatus({ kind: "success", signature });
            await refresh();
        } catch (e) {
            setCancelStatus({
                kind: "error",
                message: e instanceof Error ? e.message : String(e),
            });
        }
    }

    if (!walletAddress) {
        return (
            <div className="p-4">
                <p className="text-sm text-muted-foreground">
                    Connect a wallet to see your pending orders.
                </p>
            </div>
        );
    }

    if (loading) {
        return (
            <div className="p-4">
                <SkeletonRows rows={3} />
            </div>
        );
    }

    if (orders.length === 0) {
        return (
            <div className="p-4">
                <EmptyHint title="No open orders in this auction.">
                    Submit an order during the Collect phase to see it here.
                </EmptyHint>
            </div>
        );
    }

    const busy = cancelStatus.kind === "pending";

    return (
        <div>
            {/* Column headers */}
            <div className="flex items-center border-b border-border px-4 py-2 text-xs font-medium text-muted-foreground">
                <span className="w-12">Side</span>
                <span className="w-24">Price</span>
                <span className="w-20">Qty</span>
                <span className="flex-1">Status</span>
                <span className="w-20 text-right">Action</span>
            </div>

            {orders.map((order) => (
                <div
                    key={`${order.orderId}-${order.slotIndex}`}
                    className="flex items-center border-b border-border/40 px-4 py-1.5 text-xs"
                >
                    {/* Side */}
                    <span
                        className={cn(
                            "w-12 font-mono font-medium tnum",
                            order.side === 0 ? "text-up" : "text-down",
                        )}
                    >
                        {order.side === 0 ? "BUY" : "SELL"}
                    </span>

                    {/* Price */}
                    <span className="w-24 font-mono tnum text-foreground">
                        {formatPrice(order)}
                    </span>

                    {/* Qty */}
                    <span className="w-20 font-mono tnum text-foreground">
                        {order.quantity.toString()}
                    </span>

                    {/* Status badge */}
                    <div className="flex-1">
                        <Badge variant={statusVariant(order.status)}>
                            {statusLabel(order.status)}
                        </Badge>
                    </div>

                    {/* Cancel action — only for Resting orders */}
                    <div className="w-20 text-right">
                        {order.status === 1 ? (
                            <Button
                                variant="outline"
                                size="sm"
                                className="h-6 px-2 text-xs"
                                disabled={busy}
                                onClick={() => void cancel(order)}
                            >
                                Cancel
                            </Button>
                        ) : (
                            <span className="text-muted-foreground/40">—</span>
                        )}
                    </div>
                </div>
            ))}

            {cancelStatus.kind !== "idle" && (
                <div className="px-4 py-2">
                    <StatusMessage status={cancelStatus} />
                </div>
            )}
        </div>
    );
}
