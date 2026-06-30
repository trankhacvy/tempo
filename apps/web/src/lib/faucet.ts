export interface FaucetResult {
    signature: string;
    ata: string;
    amount: string;
}

/** Ask the serverless faucet to mint test collateral (and dust SOL) to a wallet.
 *  Throws with the server's error message on failure. */
export async function requestFaucet(owner: string): Promise<FaucetResult> {
    const res = await fetch("/api/faucet", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ owner }),
    });
    const body = (await res.json().catch(() => ({}))) as Partial<FaucetResult> & {
        error?: string;
    };
    if (!res.ok) {
        throw new Error(body.error ?? `Faucet request failed (${res.status}).`);
    }
    return {
        signature: body.signature ?? "",
        ata: body.ata ?? "",
        amount: body.amount ?? "0",
    };
}
