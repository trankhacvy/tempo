"use client";

import { useWallet } from "@solana/wallet-adapter-react";
import { useCallback, useEffect, useState } from "react";

import { ExternalLink } from "lucide-react";

import { Button } from "@/components/ui/button";
import { DataRow } from "@/components/data-row";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SkeletonRows } from "@/components/ui/skeleton";
import { StatusMessage, type TxStatus } from "@/components/ui/status-message";
import { COLLATERAL_MINT, VAULT_TOKEN_ACCOUNT } from "@/lib/config";
import { fetchCollateralView, type CollateralView, userCollateralAddress } from "@/lib/data";
import { buildDepositIx, buildInitCollateralIx, buildWithdrawIx } from "@/lib/instructions";
import { sendInstructions } from "@/lib/tx";
import { useWalletSigner } from "@/lib/use-wallet-signer";
import { formatUnits, parseUnits } from "@/lib/utils";

const DECIMALS = 6;

export function CollateralPanel() {
    const { publicKey, connected } = useWallet();
    const signer = useWalletSigner();
    const owner = publicKey?.toBase58() ?? null;

    const [collateral, setCollateral] = useState<CollateralView | null>(null);
    const [exists, setExists] = useState<boolean | null>(null);
    const [pda, setPda] = useState<string | null>(null);
    const [loading, setLoading] = useState(false);
    const [depositAmt, setDepositAmt] = useState("");
    const [withdrawAmt, setWithdrawAmt] = useState("");
    const [status, setStatus] = useState<TxStatus>({ kind: "idle" });

    const refresh = useCallback(async () => {
        if (!owner) {
            setCollateral(null);
            setExists(null);
            setPda(null);
            return;
        }
        setLoading(true);
        try {
            setPda(await userCollateralAddress(owner));
            const view = await fetchCollateralView(owner);
            setCollateral(view);
            setExists(view !== null);
        } catch {
            setCollateral(null);
            setExists(null);
        } finally {
            setLoading(false);
        }
    }, [owner]);

    useEffect(() => {
        void refresh();
    }, [refresh]);

    const busy = status.kind === "pending";

    const run = useCallback(
        async (build: () => Promise<Parameters<typeof sendInstructions>[1][number]>) => {
            if (!signer) {
                setStatus({ kind: "error", message: "Connect a wallet first." });
                return;
            }
            setStatus({ kind: "pending" });
            try {
                const ix = await build();
                const { signature } = await sendInstructions(signer, [ix], (sig) =>
                    setStatus({ kind: "pending", signature: sig }),
                );
                setStatus({ kind: "success", signature });
                await refresh();
            } catch (e) {
                setStatus({ kind: "error", message: e instanceof Error ? e.message : String(e) });
            }
        },
        [signer, refresh],
    );

    if (!connected || !owner) {
        return (
            <div className="p-4">
                <p className="text-sm text-muted-foreground">
                    Connect a wallet to view your collateral ledger.
                </p>
            </div>
        );
    }

    return (
        <div className="space-y-4 p-4">
            {exists === false ? (
                <div className="space-y-3">
                    <p className="text-sm text-muted-foreground">
                        No collateral ledger exists for this wallet yet. Initialize one to start
                        depositing.
                    </p>
                    <Button
                        disabled={busy}
                        onClick={() => void run(() => buildInitCollateralIx(owner))}
                    >
                        {busy ? "Working…" : "Init collateral"}
                    </Button>
                </div>
            ) : collateral ? (
                <div className="space-y-1">
                    <DataRow label="Balance">{formatUnits(collateral.balance, DECIMALS)}</DataRow>
                    <DataRow label="Locked">{formatUnits(collateral.locked, DECIMALS)}</DataRow>
                    <DataRow label="Free">{formatUnits(collateral.free, DECIMALS)}</DataRow>
                    {collateral.balance === 0n && <FaucetHint />}
                </div>
            ) : loading ? (
                <SkeletonRows rows={3} />
            ) : (
                <p className="text-sm text-muted-foreground">—</p>
            )}

            {exists && (
                <>
                    {!VAULT_TOKEN_ACCOUNT && (
                        <p className="text-sm text-destructive">
                            Set NEXT_PUBLIC_VAULT_TOKEN_ACCOUNT and NEXT_PUBLIC_USER_TOKEN_ACCOUNT in
                            apps/web/.env.local to enable deposit / withdraw.
                        </p>
                    )}
                    <div className="grid gap-3 sm:grid-cols-2">
                        <div className="space-y-2">
                            <Label htmlFor="deposit">Deposit amount</Label>
                            <Input
                                id="deposit"
                                inputMode="decimal"
                                placeholder="0.00"
                                value={depositAmt}
                                onChange={(e) => setDepositAmt(e.target.value)}
                                disabled={busy || !VAULT_TOKEN_ACCOUNT}
                            />
                            <Button
                                className="w-full"
                                disabled={busy || !VAULT_TOKEN_ACCOUNT || depositAmt.trim() === ""}
                                onClick={() =>
                                    void run(() => buildDepositIx(owner, parseUnits(depositAmt, DECIMALS)))
                                }
                            >
                                Deposit
                            </Button>
                        </div>
                        <div className="space-y-2">
                            <Label htmlFor="withdraw">Withdraw amount</Label>
                            <Input
                                id="withdraw"
                                inputMode="decimal"
                                placeholder="0.00"
                                value={withdrawAmt}
                                onChange={(e) => setWithdrawAmt(e.target.value)}
                                disabled={busy || !VAULT_TOKEN_ACCOUNT}
                            />
                            <Button
                                variant="outline"
                                className="w-full"
                                disabled={busy || !VAULT_TOKEN_ACCOUNT || withdrawAmt.trim() === ""}
                                onClick={() =>
                                    void run(() =>
                                        buildWithdrawIx(owner, parseUnits(withdrawAmt, DECIMALS)),
                                    )
                                }
                            >
                                Withdraw
                            </Button>
                        </div>
                    </div>
                </>
            )}

            {pda && (
                <p className="font-mono text-xs text-muted-foreground">Ledger PDA: {pda}</p>
            )}
            <StatusMessage status={status} />
        </div>
    );
}

function FaucetHint() {
    const url = COLLATERAL_MINT
        ? `https://spl-token-faucet.com/?token-name=${COLLATERAL_MINT}`
        : "https://spl-token-faucet.com/";
    return (
        <div className="mt-2 border border-dashed border-border/70 bg-secondary/10 px-3 py-2 text-xs text-muted-foreground">
            <span>Need devnet USDC? Mint the collateral token, then deposit it here. </span>
            <a
                href={url}
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-1 font-medium text-primary underline-offset-2 hover:underline"
            >
                Open devnet faucet
                <ExternalLink className="size-3" />
            </a>
        </div>
    );
}
