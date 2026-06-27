//! Shared infrastructure for every Tempo off-chain service: typed config, JSON
//! tracing + Prometheus metrics, a round-robin RPC pool with 429 failover, a
//! transaction sender with compute-budget sizing + conflict retry, and a
//! KMS-ready signer seam. A faithful Rust port of the production patterns in
//! `apps/bots/src/tx.ts`.

pub mod backoff;
pub mod config;
pub mod error;
pub mod rpc;
pub mod signer;
pub mod telemetry;
pub mod tx;

pub use backoff::Backoff;
pub use config::{env_parse, Config};
pub use error::CommonError;
pub use rpc::RpcPool;
pub use signer::{load_keypair_file, TempoSigner};
pub use tx::{TxSender, DEFAULT_CU_LIMIT};
