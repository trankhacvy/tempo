import {
    address,
    appendTransactionMessageInstructions,
    createKeyPairSignerFromBytes,
    createSolanaRpc,
    createTransactionMessage,
    getSignatureFromTransaction,
    lamports,
    pipe,
    sendTransactionWithoutConfirmingFactory,
    setTransactionMessageFeePayerSigner,
    setTransactionMessageLifetimeUsingBlockhash,
    signTransactionMessageWithSigners,
    type Address,
    type Signature,
} from "@solana/kit";
import {
    findAssociatedTokenPda,
    getCreateAssociatedTokenIdempotentInstructionAsync,
    getMintToInstruction,
    TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";

// Server-only — the master keypair (mint authority of the synthetic devnet
// collateral) lives in this route's env, NEVER in a NEXT_PUBLIC_* variable, so it
// is never shipped to the browser. Devnet only; the mint has no real value.
export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const RPC_URL =
    process.env.TEMPO_FAUCET_RPC_URL?.trim() ||
    process.env.NEXT_PUBLIC_SOLANA_RPC_URL?.trim() ||
    "https://api.devnet.solana.com";
const MINT = process.env.NEXT_PUBLIC_COLLATERAL_MINT?.trim() ?? "";
// Default grant: 100k tokens at 6 decimals.
const AMOUNT = BigInt(process.env.TEMPO_FAUCET_AMOUNT?.trim() || "100000000000");
// Best-effort SOL dust so the wallet can pay fees (devnet airdrop, may be throttled).
const SOL_DUST = BigInt(process.env.TEMPO_FAUCET_SOL_LAMPORTS?.trim() || "50000000");

const CONFIRM_TIMEOUT_MS = 30_000;
const CONFIRM_POLL_MS = 800;

function isValidBase58Address(s: string): boolean {
    return /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(s);
}

function loadMasterBytes(): Uint8Array | null {
    const raw = process.env.TEMPO_FAUCET_SECRET?.trim();
    if (!raw) return null;
    try {
        const arr = JSON.parse(raw) as number[];
        if (!Array.isArray(arr) || arr.length !== 64) return null;
        return Uint8Array.from(arr);
    } catch {
        return null;
    }
}

type Rpc = ReturnType<typeof createSolanaRpc>;

async function confirm(rpc: Rpc, sig: Signature): Promise<boolean> {
    const deadline = Date.now() + CONFIRM_TIMEOUT_MS;
    while (Date.now() < deadline) {
        const { value } = await rpc.getSignatureStatuses([sig]).send();
        const st = value[0];
        if (st) {
            if (st.err) return false;
            if (st.confirmationStatus === "confirmed" || st.confirmationStatus === "finalized") {
                return true;
            }
        }
        await new Promise((r) => setTimeout(r, CONFIRM_POLL_MS));
    }
    return false;
}

export async function POST(req: Request): Promise<Response> {
    if (!MINT) {
        return Response.json({ error: "Faucet not configured (collateral mint missing)." }, { status: 503 });
    }
    const masterBytes = loadMasterBytes();
    if (!masterBytes) {
        return Response.json(
            { error: "Faucet not configured (set TEMPO_FAUCET_SECRET to the master keypair byte array)." },
            { status: 503 },
        );
    }

    let owner: string;
    try {
        const body = (await req.json()) as { owner?: unknown };
        owner = typeof body.owner === "string" ? body.owner.trim() : "";
    } catch {
        return Response.json({ error: "Invalid JSON body." }, { status: 400 });
    }
    if (!isValidBase58Address(owner)) {
        return Response.json({ error: "Provide a valid wallet address." }, { status: 400 });
    }

    const rpc = createSolanaRpc(RPC_URL);
    const master = await createKeyPairSignerFromBytes(masterBytes);
    const mint = address(MINT);
    const ownerAddr = address(owner);

    const [ata] = await findAssociatedTokenPda({
        owner: ownerAddr,
        mint,
        tokenProgram: TOKEN_PROGRAM_ADDRESS,
    });

    // Best-effort SOL airdrop for fees — devnet only, ignore throttling.
    if (SOL_DUST > 0n) {
        try {
            await rpc.requestAirdrop(ownerAddr, lamports(SOL_DUST)).send();
        } catch {
            // ignore — the UI also surfaces a SOL faucet hint.
        }
    }

    const createAta = await getCreateAssociatedTokenIdempotentInstructionAsync({
        payer: master,
        owner: ownerAddr,
        mint,
    });
    const mintTo = getMintToInstruction({
        mint,
        token: ata as Address,
        mintAuthority: master,
        amount: AMOUNT,
    });

    try {
        const { value: blockhash } = await rpc.getLatestBlockhash().send();
        const message = pipe(
            createTransactionMessage({ version: 0 }),
            (m) => setTransactionMessageFeePayerSigner(master, m),
            (m) => setTransactionMessageLifetimeUsingBlockhash(blockhash, m),
            (m) => appendTransactionMessageInstructions([createAta, mintTo], m),
        );
        const signed = await signTransactionMessageWithSigners(message);
        const send = sendTransactionWithoutConfirmingFactory({ rpc });
        await send(signed, { commitment: "confirmed" });
        const signature = getSignatureFromTransaction(signed);
        const ok = await confirm(rpc, signature);
        if (!ok) {
            return Response.json(
                { error: "Mint transaction did not confirm in time.", signature },
                { status: 504 },
            );
        }
        return Response.json({ signature, ata, amount: AMOUNT.toString() });
    } catch (e) {
        return Response.json(
            { error: e instanceof Error ? e.message : String(e) },
            { status: 500 },
        );
    }
}
