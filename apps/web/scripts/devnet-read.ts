// Read-only devnet smoke test: confirms the bundled generated client + RPC
// wiring resolve, fetches the program account, and (if a market is provided)
// decodes it. Run with: pnpm --filter @tempo/web devnet-read [marketAddress]
import { address, createSolanaRpc } from "@solana/kit";

// The generated TEMPO_PROGRAM_PROGRAM_ADDRESS is also re-exported from the
// bundle; importing it here confirms the bundle links under tsx/Node ESM.
import { TEMPO_PROGRAM_PROGRAM_ADDRESS } from "../src/vendor/tempo-client.mjs";

const RPC = process.env.NEXT_PUBLIC_SOLANA_RPC_URL ?? "https://api.devnet.solana.com";
const PROGRAM_ID = TEMPO_PROGRAM_PROGRAM_ADDRESS as string;

// 2-byte prefix (disc + version), then little-endian C-struct fields. See
// lib/data.ts and tests/integration-tests/src/lib.rs for the authoritative
// layout — the Codama decoders are off-by-one and must not be used to decode.
const PREFIX = 2;
function readU64le(b: Uint8Array, off: number): bigint {
    let v = 0n;
    for (let i = 7; i >= 0; i--) v = (v << 8n) | BigInt(b[PREFIX + off + i] ?? 0);
    return v;
}
function readU32le(b: Uint8Array, off: number): number {
    let v = 0;
    for (let i = 3; i >= 0; i--) v = v * 256 + (b[PREFIX + off + i] ?? 0);
    return v;
}

async function main(): Promise<void> {
    const rpc = createSolanaRpc(RPC);

    const program = await rpc.getAccountInfo(address(PROGRAM_ID), { encoding: "base64" }).send();
    if (program.value === null) throw new Error("Program account not found on devnet");
    if (!program.value.executable) throw new Error("Program account is not executable");
    console.log(`✓ Program ${PROGRAM_ID} is executable on devnet (owner ${program.value.owner})`);

    const marketArg = process.argv[2];
    if (marketArg) {
        const info = await rpc.getAccountInfo(address(marketArg), { encoding: "base64" }).send();
        if (info.value === null) {
            console.log(`• Market ${marketArg}: no account found`);
            return;
        }
        if (info.value.owner !== PROGRAM_ID) {
            console.log(`• Market ${marketArg}: not owned by Tempo program (owner ${info.value.owner})`);
            return;
        }
        const d = Uint8Array.from(Buffer.from(info.value.data[0], "base64"));
        const phase = d[PREFIX + 160] ?? 0;
        console.log(`✓ Decoded Market ${marketArg}:`);
        console.log(
            `    phase=${phase} auctionId=${readU64le(d, 0)} tickSize=${readU64le(d, 16)} numTicks=${readU32le(d, 60)}`,
        );
    } else {
        console.log("• No market address passed; skipping decode. Pass one as argv to decode.");
    }
}

main().catch((e) => {
    console.error("devnet-read failed:", e instanceof Error ? e.message : String(e));
    process.exit(1);
});
