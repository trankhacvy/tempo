import { Wordmark } from "@/components/brand/logo";
import { PROGRAM_ID } from "@/lib/config";
import { explorerAddressUrl } from "@/lib/tx";
import { shortenAddress } from "@/lib/utils";

export function Footer() {
    const year = new Date().getFullYear();

    return (
        <footer className="mt-20 border-t border-border/60">
            <div className="mx-auto max-w-7xl px-4 py-10 sm:px-6">
                <div className="flex flex-col gap-8 md:flex-row md:justify-between">
                    <div className="max-w-sm space-y-3">
                        <Wordmark />
                        <p className="text-sm leading-relaxed text-muted-foreground">
                            A dual-flow batch-auction perpetuals DEX on Solana. Orders clear at one uniform
                            price per auction — no speed advantage, no MEV.
                        </p>
                    </div>

                    <div className="flex flex-col gap-3 text-sm">
                        <span className="text-xs font-medium uppercase tracking-[0.18em] text-muted-foreground/70">
                            Program
                        </span>
                        <a
                            href={explorerAddressUrl(PROGRAM_ID)}
                            target="_blank"
                            rel="noreferrer"
                            className="font-mono text-muted-foreground transition-colors hover:text-foreground"
                        >
                            {shortenAddress(PROGRAM_ID, 6)}
                        </a>
                    </div>
                </div>

                <div className="mt-8 flex flex-col gap-3 border-t border-border/60 pt-6 text-xs text-muted-foreground sm:flex-row sm:items-center sm:justify-between">
                    <p>© {year} Tempo. Solana devnet.</p>
                    <p>Not financial advice.</p>
                </div>
            </div>
        </footer>
    );
}
