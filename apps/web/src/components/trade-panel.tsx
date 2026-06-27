"use client";

import { useWallet } from "@solana/wallet-adapter-react";
import { useWalletModal } from "@solana/wallet-adapter-react-ui";
import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { StatusMessage, type TxStatus } from "@/components/ui/status-message";
import { type MarketView } from "@/lib/data";
import { buildSubmitOrderIx } from "@/lib/instructions";
import { Phase } from "@/lib/tempo-client";
import { notionalToQty, tickToUsd, usdToTick } from "@/lib/tempo-math";
import { sendInstructions } from "@/lib/tx";
import { useWalletSigner } from "@/lib/use-wallet-signer";
import { cn, isValidBase58Address } from "@/lib/utils";

type Side = 0 | 1;

const MIN_LEVERAGE = 1;
const MAX_LEVERAGE = 10;
const CROSS_TICKS = 2n;

interface DerivedOrder {
    price: bigint;
    quantity: bigint;
    notionalUsd: number;
    priceUsd: number | null;
}

/** A USD collateral + leverage → a marketable order. Notional = collateral ·
 *  leverage; quantity = floor(notional / oracleUsd); price = the live oracle
 *  tick crossed a couple ticks into the trade direction (a taker that sweeps). */
function deriveOrder(
    collateralUsd: number,
    leverage: number,
    oracleUsd: number,
    side: Side,
    tickSize: bigint,
): DerivedOrder | null {
    if (collateralUsd <= 0 || oracleUsd <= 0) return null;
    const notionalUsd = collateralUsd * leverage;
    const quantity = notionalToQty(notionalUsd, oracleUsd);
    if (quantity <= 0n) return null;
    const oracleTick = usdToTick(oracleUsd, tickSize);
    const price =
        side === 0
            ? oracleTick + CROSS_TICKS
            : oracleTick > CROSS_TICKS
              ? oracleTick - CROSS_TICKS
              : 1n;
    return { price, quantity, notionalUsd, priceUsd: tickToUsd(price, tickSize) };
}

export function TradePanel({
    market,
    view,
    oracleUsd,
    countdown,
}: {
    market: string;
    view: MarketView | null;
    oracleUsd: number | null;
    countdown: string | null;
}) {
    const { connected, publicKey } = useWallet();
    const { setVisible: openWalletModal } = useWalletModal();
    const signer = useWalletSigner();

    const [side, setSide] = useState<Side>(0);
    const [collateral, setCollateral] = useState("");
    const [leverage, setLeverage] = useState(2);
    const [advanced, setAdvanced] = useState(false);
    const [price, setPrice] = useState("");
    const [quantity, setQuantity] = useState("");
    const [status, setStatus] = useState<TxStatus>({ kind: "idle" });

    const busy = status.kind === "pending";
    const owner = publicKey?.toBase58() ?? null;
    const marketReady = isValidBase58Address(market);
    const tickSize = view?.tickSize ?? 0n;
    const collecting = view !== null && view.phase === Phase.Collect;
    const gated = view !== null && !collecting;

    const derived = useMemo<DerivedOrder | null>(() => {
        const c = Number(collateral);
        if (advanced || !Number.isFinite(c) || c <= 0 || oracleUsd === null || tickSize <= 0n)
            return null;
        return deriveOrder(c, leverage, oracleUsd, side, tickSize);
    }, [advanced, collateral, leverage, oracleUsd, side, tickSize]);

    function resolveOrder(): { price: bigint; quantity: bigint } | null {
        if (advanced) {
            try {
                const p = BigInt(price.trim());
                const q = BigInt(quantity.trim());
                if (p <= 0n || q <= 0n) throw new Error("non-positive");
                return { price: p, quantity: q };
            } catch {
                setStatus({
                    kind: "error",
                    message: "Price and quantity must be positive whole numbers (ticks / base units).",
                });
                return null;
            }
        }
        if (!derived) {
            setStatus({
                kind: "error",
                message: "Enter a collateral amount and wait for the live oracle price.",
            });
            return null;
        }
        return { price: derived.price, quantity: derived.quantity };
    }

    async function submit() {
        if (!signer || !owner) {
            setStatus({ kind: "error", message: "Connect a wallet first." });
            return;
        }
        if (!marketReady) {
            setStatus({ kind: "error", message: "Load a valid market first." });
            return;
        }
        if (gated) {
            setStatus({
                kind: "error",
                message: "The auction is clearing. Submissions reopen when it returns to Collect.",
            });
            return;
        }
        const resolved = resolveOrder();
        if (!resolved) return;
        setStatus({ kind: "pending" });
        try {
            const ix = await buildSubmitOrderIx(owner, {
                market,
                side,
                price: resolved.price,
                quantity: resolved.quantity,
            });
            const { signature } = await sendInstructions(signer, [ix], (sig) =>
                setStatus({ kind: "pending", signature: sig }),
            );
            setStatus({ kind: "success", signature });
        } catch (e) {
            setStatus({
                kind: "error",
                message: e instanceof Error ? e.message : String(e),
            });
        }
    }

    const inputsEmpty = advanced
        ? price.trim() === "" || quantity.trim() === ""
        : derived === null;
    const submitDisabled = busy || !marketReady || gated || inputsEmpty;

    return (
        <div className="flex flex-col">
            {/* Panel header */}
            <div className="flex h-9 items-center justify-between border-b border-border px-4">
                <span className="text-xs font-medium text-foreground">Submit order</span>
                <button
                    type="button"
                    onClick={() => setAdvanced((v) => !v)}
                    className="font-mono text-[11px] uppercase tracking-wide text-muted-foreground transition-colors hover:text-foreground"
                >
                    {advanced ? "simple" : "advanced"}
                </button>
            </div>

            {/* Panel content */}
            <div className="space-y-4 p-4">
                {/* Side toggle */}
                <div className="space-y-2">
                    <Label>Side</Label>
                    <div className="grid grid-cols-2 gap-2">
                        <SideToggle active={side === 0} tone="up" onClick={() => setSide(0)} disabled={busy || !connected}>
                            Buy / Long
                        </SideToggle>
                        <SideToggle active={side === 1} tone="down" onClick={() => setSide(1)} disabled={busy || !connected}>
                            Sell / Short
                        </SideToggle>
                    </div>
                </div>

                {advanced ? (
                    <>
                        <p className="border border-border/60 bg-secondary/20 p-2 text-[11px] text-muted-foreground">
                            Orders submitted here are takers. Maker liquidity is posted through the
                            maker-quote book (a market-maker flow), not the trade panel.
                        </p>
                        <div className="grid grid-cols-2 gap-3">
                            <div className="space-y-2">
                                <Label htmlFor="price">Price (ticks)</Label>
                                <Input
                                    id="price"
                                    inputMode="numeric"
                                    placeholder="0"
                                    value={price}
                                    onChange={(e) => setPrice(e.target.value)}
                                    disabled={busy}
                                    className="h-11 font-mono text-base"
                                />
                            </div>
                            <div className="space-y-2">
                                <Label htmlFor="qty">Quantity</Label>
                                <Input
                                    id="qty"
                                    inputMode="numeric"
                                    placeholder="0"
                                    value={quantity}
                                    onChange={(e) => setQuantity(e.target.value)}
                                    disabled={busy}
                                    className="h-11 font-mono text-base"
                                />
                            </div>
                        </div>
                    </>
                ) : (
                    <>
                        <div className="space-y-2">
                            <Label htmlFor="collateral">Collateral (USD)</Label>
                            <Input
                                id="collateral"
                                inputMode="decimal"
                                placeholder="0.00"
                                value={collateral}
                                onChange={(e) => setCollateral(e.target.value)}
                                disabled={busy || !connected}
                                className="h-11 font-mono text-base"
                            />
                        </div>

                        <div className="space-y-2">
                            <div className="flex items-center justify-between">
                                <Label htmlFor="leverage">Leverage</Label>
                                <span className="font-mono text-sm font-semibold tnum text-foreground">
                                    {leverage}×
                                </span>
                            </div>
                            <input
                                id="leverage"
                                type="range"
                                min={MIN_LEVERAGE}
                                max={MAX_LEVERAGE}
                                step={1}
                                value={leverage}
                                onChange={(e) => setLeverage(Number(e.target.value))}
                                disabled={busy || !connected}
                                className="h-1.5 w-full cursor-pointer appearance-none bg-secondary accent-primary disabled:opacity-50"
                            />
                            <div className="flex justify-between text-[10px] text-muted-foreground/60">
                                <span>{MIN_LEVERAGE}×</span>
                                <span>{MAX_LEVERAGE}×</span>
                            </div>
                        </div>

                        <div className="space-y-1 border border-border/60 bg-secondary/20 p-3 text-[11px]">
                            <Estimate label="Oracle price">
                                {oracleUsd !== null ? `$${oracleUsd.toFixed(2)}` : "syncing…"}
                            </Estimate>
                            <Estimate label="Notional">
                                {derived ? `$${derived.notionalUsd.toFixed(2)}` : "—"}
                            </Estimate>
                            <Estimate label="Quantity">
                                {derived ? derived.quantity.toString() : "—"}
                            </Estimate>
                            <Estimate label="Limit price">
                                {derived
                                    ? `${derived.price.toString()} tick${derived.priceUsd !== null ? ` · $${derived.priceUsd.toFixed(2)}` : ""}`
                                    : "—"}
                            </Estimate>
                        </div>
                    </>
                )}

                {!connected ? (
                    <Button
                        size="lg"
                        className="w-full text-base font-semibold"
                        onClick={() => openWalletModal(true)}
                    >
                        Connect Wallet
                    </Button>
                ) : gated ? (
                    <Button size="lg" className="w-full text-base font-semibold" disabled>
                        Auction clearing — opens in {countdown ?? "soon"}
                    </Button>
                ) : (
                    <Button
                        size="lg"
                        className={cn(
                            "w-full text-base font-semibold text-white",
                            side === 0 ? "bg-up hover:bg-up/90" : "bg-down hover:bg-down/90",
                        )}
                        disabled={submitDisabled}
                        onClick={() => void submit()}
                    >
                        {busy ? "Submitting…" : side === 0 ? "Submit Buy" : "Submit Sell"}
                    </Button>
                )}

                {!marketReady && (
                    <p className="text-sm text-muted-foreground">
                        Load a market in the Market tab first.
                    </p>
                )}
                <StatusMessage status={status} />
            </div>
        </div>
    );
}

function Estimate({ label, children }: { label: string; children: React.ReactNode }) {
    return (
        <div className="flex items-center justify-between">
            <span className="text-muted-foreground">{label}</span>
            <span className="font-mono tnum text-foreground">{children}</span>
        </div>
    );
}

function SideToggle({
    active,
    tone,
    children,
    ...props
}: {
    active: boolean;
    tone: "up" | "down";
} & React.ButtonHTMLAttributes<HTMLButtonElement>) {
    return (
        <button
            type="button"
            className={cn(
                "border py-2.5 text-sm font-semibold transition-colors disabled:opacity-50",
                active
                    ? tone === "up"
                        ? "border-up bg-up/15 text-up"
                        : "border-down bg-down/15 text-down"
                    : "border-border bg-secondary/30 text-muted-foreground hover:text-foreground",
            )}
            {...props}
        >
            {children}
        </button>
    );
}
