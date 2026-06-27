// Types for the esbuild-bundled generated client (tempo-client.mjs). The bundle
// is the runtime artifact (Next/webpack ESM can't link the generated client's
// chained `export *` const re-exports under all conditions); types come
// straight from the generated sources. Regenerate the bundle with
// `pnpm bundle-client` (runs automatically on dev/build).
export * from "../../../../clients/typescript/src/generated/index";
