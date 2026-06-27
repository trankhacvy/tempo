"use client";

import { AlertCircle, CheckCircle2, Loader2, ExternalLink } from "lucide-react";

import { explorerTxUrl } from "@/lib/tx";
import { cn } from "@/lib/utils";

export type TxStatus =
    | { kind: "idle" }
    | { kind: "pending"; signature?: string }
    | { kind: "success"; signature: string }
    | { kind: "error"; message: string };

export function StatusMessage({ status }: { status: TxStatus }) {
    if (status.kind === "idle") return null;

    const base = "flex items-start gap-2 rounded-[var(--radius)] border p-3 text-sm";

    if (status.kind === "pending") {
        return (
            <div className={cn(base, "border-border bg-muted text-muted-foreground")}>
                <Loader2 className="mt-0.5 size-4 shrink-0 animate-spin" />
                <div className="min-w-0">
                    <p className="font-medium text-foreground">Sending transaction…</p>
                    {status.signature && <SigLink signature={status.signature} />}
                </div>
            </div>
        );
    }

    if (status.kind === "success") {
        return (
            <div className={cn(base, "border-success/40 bg-success/10 text-success")}>
                <CheckCircle2 className="mt-0.5 size-4 shrink-0" />
                <div className="min-w-0">
                    <p className="font-medium">Confirmed on devnet.</p>
                    <SigLink signature={status.signature} />
                </div>
            </div>
        );
    }

    return (
        <div className={cn(base, "border-destructive/40 bg-destructive/10 text-destructive")}>
            <AlertCircle className="mt-0.5 size-4 shrink-0" />
            <p className="min-w-0 break-words">{status.message}</p>
        </div>
    );
}

function SigLink({ signature }: { signature: string }) {
    return (
        <a
            href={explorerTxUrl(signature)}
            target="_blank"
            rel="noreferrer"
            className="mt-0.5 inline-flex items-center gap-1 break-all font-mono text-xs underline underline-offset-2 hover:opacity-80"
        >
            {signature.slice(0, 12)}…{signature.slice(-12)}
            <ExternalLink className="size-3" />
        </a>
    );
}
