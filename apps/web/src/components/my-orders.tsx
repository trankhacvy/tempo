"use client";

import { useCallback, useEffect, useState } from "react";
import { useWallet } from "@solana/wallet-adapter-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { EmptyHint, SkeletonRows } from "@/components/ui/skeleton";
import { StatusMessage, type TxStatus } from "@/components/ui/status-message";
import { fetchOrderBook, type OrderView } from "@/lib/auction";
import { type MarketView } from "@/lib/data";
import { buildCancelOrderIx } from "@/lib/instructions";
import { sendInstructions } from "@/lib/tx";
import { useInterval } from "@/lib/use-interval";
import { useWalletSigner } from "@/lib/use-wallet-signer";
import { cn, shortenAddress } from "@/lib/utils";

const POLL_MS = 3000;

type BookOrder = OrderView & { trader: string };

interface MyOrdersProps {
    market: string | null;
    view: MarketView | null;
}

function statusLabel(status: 0 | 1 | 2 | 3): string {
    switch (status) {
        case 1: return "Resting";
        case 2: return "Accumulated";
        default: return "—";
    }
}

function formatPrice(order: OrderView): string {
    if (order.priceUsd !== null) return `$${order.priceUsd.toFixed(2)}`;
    return order.price.toString();
}

export function MyOrders({ market, view }: MyOrdersProps) {
    const { publicKey } = useWallet();
    const signer = useWalletSigner();
    const walletAddress = publicKey?.toBase58() ?? null;
    void view;

    const [orders, setOrders] = useState<BookOrder[]>([]);
    const [loading, setLoading] = useState(false);
    const [cancelStatus, setCancelStatus] = useState<TxStatus>({ kind: "idle" });

    const active = !!market;

    const refresh = useCallback(async () => {
        if (!market) {
            setOrders([]);
            return;
        }
        try {
            const book = await fetchOrderBook(market);
            // Sell asks high→low then buy bids high→low, so the book reads top-down.
            const sorted = [...book.orders].sort((a, b) =>
                a.side !== b.side ? a.side - b.side : Number(b.price - a.price),
            );
            setOrders(sorted);
        } catch {
            // silent
        } finally {
            setLoading(false);
        }
    }, [market]);

    useEffect(() => {
        if (!active) {
            setOrders([]);
            return;
        }
        setLoading(true);
        void refresh();
    }, [active, refresh]);

    useInterval(() => void refresh(), active ? POLL_MS : null);

    async function cancel(order: BookOrder) {
        if (!signer || !walletAddress || !market) {
            setCancelStatus({ kind: "error", message: "Wallet not connected." });
            return;
        }
        setCancelStatus({ kind: "pending" });
        try {
            const ix = await buildCancelOrderIx(walletAddress, market, order.orderId, order.slotIndex);
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
                <EmptyHint title="No resting orders">
                    Orders appear here during the Collect phase, then fold into the histogram.
                </EmptyHint>
            </div>
        );
    }

    const busy = cancelStatus.kind === "pending";

    return (
        <div>
            <div className="flex items-center border-b border-border px-4 py-2 text-xs font-medium text-muted-foreground">
                <span className="w-12">Side</span>
                <span className="w-24">Price</span>
                <span className="w-20">Qty</span>
                <span className="w-24">Trader</span>
                <span className="flex-1">Status</span>
                <span className="w-20 text-right">Action</span>
            </div>

            {orders.map((order) => {
                const mine = walletAddress !== null && order.trader === walletAddress;
                return (
                    <div
                        key={`${order.orderId}-${order.slotIndex}`}
                        className={cn(
                            "flex items-center border-b border-border/40 px-4 py-1.5 text-xs",
                            mine && "bg-primary/5",
                        )}
                    >
                        <span
                            className={cn(
                                "w-12 font-mono font-medium tnum",
                                order.side === 0 ? "text-up" : "text-down",
                            )}
                        >
                            {order.side === 0 ? "BUY" : "SELL"}
                        </span>
                        <span className="w-24 font-mono tnum text-foreground">{formatPrice(order)}</span>
                        <span className="w-20 font-mono tnum text-foreground">{order.quantity.toString()}</span>
                        <span className="w-24 font-mono tnum text-muted-foreground">
                            {mine ? <span className="text-primary">you</span> : shortenAddress(order.trader, 4)}
                        </span>
                        <div className="flex-1">
                            <Badge variant={order.status === 1 ? "muted" : "default"}>
                                {statusLabel(order.status)}
                            </Badge>
                        </div>
                        <div className="w-20 text-right">
                            {mine && order.status === 1 ? (
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
                );
            })}

            {cancelStatus.kind !== "idle" && (
                <div className="px-4 py-2">
                    <StatusMessage status={cancelStatus} />
                </div>
            )}
        </div>
    );
}
