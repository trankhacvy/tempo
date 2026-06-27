use solana_sdk::signature::{Keypair, Signer};

use crate::error::CommonError;

/// Marker for any signer a Tempo service can drive. A file-backed [`Keypair`] is
/// the devnet default; production swaps in a KMS/Vault-backed signer behind this
/// same trait with no service-code change.
pub trait TempoSigner: Signer + Send + Sync {}

impl<T: Signer + Send + Sync> TempoSigner for T {}

/// Load a signer from a Solana CLI keypair file (a JSON array of 64 bytes).
pub fn load_keypair_file(path: &str) -> Result<Keypair, CommonError> {
    let bytes = std::fs::read(path)?;
    let secret: Vec<u8> =
        serde_json::from_slice(&bytes).map_err(|e| CommonError::Signer(e.to_string()))?;
    Keypair::try_from(secret.as_slice()).map_err(|e| CommonError::Signer(e.to_string()))
}
