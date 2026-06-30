/**
 * Generates the TypeScript and Rust clients from the Codama IDL emitted by
 * `program/build.rs` (run `pnpm generate-idl` first).
 */

import { renderVisitor as renderJavaScriptVisitor } from '@codama/renderers-js';
import { renderVisitor as renderRustVisitor } from '@codama/renderers-rust';
import { createFromRoot, type RootNode } from 'codama';
import fs from 'node:fs';
import path from 'node:path';

const projectRoot = path.join(__dirname, '..');
const idlPath = path.join(projectRoot, 'idl', 'tempo_program.json');
const rustClientDir = path.join(projectRoot, 'crates', 'sdk');
const tsClientDir = path.join(projectRoot, 'clients', 'typescript');

const rawIdl = JSON.parse(fs.readFileSync(idlPath, 'utf-8')) as RootNode;

// Patch all instruction nodes to use "omitted" optional-account strategy.
// The default "programId" substitutes the program address (executable) as a
// writable placeholder for missing optional accounts, which the Solana runtime
// rejects with "Account is immutable". With "omitted", missing optional
// accounts are absent from the instruction — what the program actually expects.
function patchOmitted(node: unknown): unknown {
    if (node === null || typeof node !== 'object') return node;
    if (Array.isArray(node)) return node.map(patchOmitted);
    const obj = node as Record<string, unknown>;
    const patched = Object.fromEntries(
        Object.entries(obj).map(([k, v]) => [k, patchOmitted(v)])
    );
    if (patched['kind'] === 'instructionNode') {
        patched['optionalAccountStrategy'] = 'omitted';
    }
    return patched;
}

const idl = patchOmitted(rawIdl) as RootNode;
const codama = createFromRoot(idl);

// Refresh only src/generated — the crate manifest and hand-written modules are untouched.
fs.rmSync(path.join(rustClientDir, 'src', 'generated'), { recursive: true, force: true });
codama.accept(
    renderRustVisitor(rustClientDir, {
        deleteFolderBeforeRendering: false,
        formatCode: true,
    }),
);

codama.accept(
    renderJavaScriptVisitor(tsClientDir, {
        deleteFolderBeforeRendering: true,
        formatCode: true,
    }),
);

console.log('Generated Rust client → crates/sdk/src/generated');
console.log('Generated TypeScript client →', tsClientDir);
