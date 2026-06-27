import { isSignerRole, isWritableRole, type Instruction, type AccountMeta } from "@solana/kit";
import {
    Connection,
    PublicKey,
    TransactionInstruction,
    TransactionMessage,
    VersionedTransaction,
    type Commitment,
} from "@solana/web3.js";

import { RPC_URL } from "./config";

const COMMITMENT: Commitment = "confirmed";

// A wallet-adapter-shaped signer: signs a VersionedTransaction.
export interface WalletSigner {
    publicKey: PublicKey;
    signTransaction: <T extends VersionedTransaction>(tx: T) => Promise<T>;
}

/** Convert a @solana/kit Instruction (from the generated client) into a
 *  web3.js TransactionInstruction so a Wallet-Standard wallet can sign it. */
function toWeb3Instruction(ix: Instruction): TransactionInstruction {
    const metas = (ix.accounts ?? []) as readonly AccountMeta[];
    return new TransactionInstruction({
        programId: new PublicKey(ix.programAddress),
        keys: metas.map((m) => ({
            pubkey: new PublicKey(m.address),
            isSigner: isSignerRole(m.role),
            isWritable: isWritableRole(m.role),
        })),
        data: Buffer.from((ix.data ?? new Uint8Array()) as Uint8Array),
    });
}

export class TxError extends Error {
    constructor(
        message: string,
        readonly cause?: unknown,
    ) {
        super(message);
        this.name = "TxError";
    }
}

// Readable copy for the Tempo program's custom error codes (mirrors the
// generated error map, but is independent of NODE_ENV — the generated
// getTempoProgramErrorMessage returns a placeholder in production bundles).
// Codes are the on-chain custom-error numbers from clients/.../errors.
const PROGRAM_ERROR_MESSAGES: Record<number, string> = {
    0x3: "The auction is not in its order-collection phase right now. Submissions reopen when the round returns to Collect.",
    0x4: "The order slab is full for this auction (orders-per-auction cap reached). Try the next round.",
    0x6: "Order price is invalid — it must be non-zero and aligned to the market tick size.",
    0x7: "Order quantity is invalid.",
    0x8: "Order quantity must be greater than zero.",
    0x18: "Not enough free collateral. Deposit more, or reduce your order size / leverage.",
    0x1a: "The token account or mint does not match the market vault. Check your collateral wiring.",
    0x1b: "This fill needs your position/collateral accounts. Initialize a position first.",
    0x1c: "The oracle confidence interval is too wide to trade against right now. Try again shortly.",
    0x1d: "Order quantity is below the market minimum order size.",
    0x1e: "You have reached the per-trader order cap for this auction.",
    0x20: "The collection window is still open; this action cannot run yet.",
};

function customErrorMessage(code: number): string {
    return PROGRAM_ERROR_MESSAGES[code] ?? `Program rejected the transaction (custom error 0x${code.toString(16)}).`;
}

/** Translate raw send/sign failures into actionable messages. */
function explain(err: unknown): TxError {
    const msg = err instanceof Error ? err.message : String(err);
    const lower = msg.toLowerCase();
    if (lower.includes("user rejected") || lower.includes("rejected the request")) {
        return new TxError("You rejected the transaction in your wallet.", err);
    }
    if (lower.includes("insufficient") && lower.includes("lamports")) {
        return new TxError("Insufficient SOL to pay transaction fees. Airdrop some devnet SOL.", err);
    }
    if (lower.includes("blockhash not found") || lower.includes("block height exceeded")) {
        return new TxError("Blockhash expired before the transaction landed. Please retry.", err);
    }
    const custom = /custom program error: (0x[0-9a-fA-F]+|\d+)/.exec(msg);
    if (custom) {
        const raw = custom[1] ?? "0";
        const code = raw.startsWith("0x") ? Number.parseInt(raw, 16) : Number.parseInt(raw, 10);
        return new TxError(customErrorMessage(code), err);
    }
    return new TxError(msg, err);
}

export interface SendResult {
    signature: string;
}

/** Build → sign (via wallet) → send → confirm. Returns the signature.
 *  Surfaces the signature even while confirmation is pending by accepting an
 *  onSignature callback. */
export async function sendInstructions(
    wallet: WalletSigner,
    instructions: readonly Instruction[],
    onSignature?: (signature: string) => void,
): Promise<SendResult> {
    const connection = new Connection(RPC_URL, COMMITMENT);
    try {
        const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash(COMMITMENT);
        const message = new TransactionMessage({
            payerKey: wallet.publicKey,
            recentBlockhash: blockhash,
            instructions: instructions.map(toWeb3Instruction),
        }).compileToV0Message();
        const tx = new VersionedTransaction(message);

        const signed = await wallet.signTransaction(tx);

        const signature = await connection.sendTransaction(signed, {
            skipPreflight: true,   // bypass stale simulation cache
            maxRetries: 3,
        });
        onSignature?.(signature);

        const confirmation = await connection.confirmTransaction(
            { signature, blockhash, lastValidBlockHeight },
            COMMITMENT,
        );
        if (confirmation.value.err) {
            throw new TxError(
                `Transaction failed on-chain: ${JSON.stringify(confirmation.value.err)}`,
            );
        }
        return { signature };
    } catch (err) {
        if (err instanceof TxError) throw err;
        throw explain(err);
    }
}

export function explorerTxUrl(signature: string): string {
    return `https://explorer.solana.com/tx/${signature}?cluster=devnet`;
}

export function explorerAddressUrl(address: string): string {
    return `https://explorer.solana.com/address/${address}?cluster=devnet`;
}
