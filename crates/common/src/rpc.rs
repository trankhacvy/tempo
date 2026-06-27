use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;

use crate::error::CommonError;

const MAX_BACKOFF: Duration = Duration::from_secs(4);

/// Classify an RPC error string into `(rate_limited, transient)`, mirroring the
/// regexes in `tx.ts`. A 429 means the request never reached the validator (safe
/// to switch endpoints); a transient network error may have landed.
pub fn classify_error(msg: &str) -> (bool, bool) {
    let m = msg.to_ascii_lowercase();
    let rate = m.contains("429") || m.contains("too many requests") || m.contains("rate");
    let transient = m.contains("fetch failed")
        || m.contains("econn")
        || m.contains("etimedout")
        || m.contains("timed out")
        || m.contains("502")
        || m.contains("503");
    (rate, transient)
}

/// A round-robin pool of RPC endpoints. Requests rotate across the pool and skip
/// to the next endpoint on a 429, so several free-tier keys behave like one
/// higher-limit endpoint (port of `tx.ts::connect`).
pub struct RpcPool {
    clients: Vec<RpcClient>,
    hosts: Vec<String>,
    cursor: AtomicUsize,
}

impl RpcPool {
    /// Parse a comma/whitespace-separated URL list (matches `TEMPO_RPC_URL`).
    pub fn from_urls(urls: &str, commitment: CommitmentConfig) -> Result<Self, CommonError> {
        let mut clients = Vec::new();
        let mut hosts = Vec::new();
        for u in urls
            .split(|c: char| c.is_whitespace() || c == ',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            clients.push(RpcClient::new_with_commitment(u.to_string(), commitment));
            hosts.push(host_of(u));
        }
        if clients.is_empty() {
            return Err(CommonError::NoRpcUrls);
        }
        Ok(Self {
            clients,
            hosts,
            cursor: AtomicUsize::new(0),
        })
    }

    pub fn len(&self) -> usize {
        self.clients.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor.load(Ordering::Relaxed)
    }

    /// The endpoint host + pool size, for logging.
    pub fn label(&self) -> String {
        let i = self.cursor() % self.hosts.len();
        format!("{} [{} rpc]", self.hosts[i], self.hosts.len())
    }

    /// The client at `idx` (wraps around the pool).
    pub fn client(&self, idx: usize) -> &RpcClient {
        &self.clients[idx % self.clients.len()]
    }

    fn rotate(&self) -> usize {
        self.cursor.fetch_add(1, Ordering::Relaxed)
    }

    /// Round-robin a call across the pool: rotate each attempt, switch endpoint on
    /// a 429, back off on transient errors (port of `tx.ts::rpcCall`).
    pub async fn call<T, E, F>(&self, attempts: usize, f: F) -> Result<T, CommonError>
    where
        F: AsyncFn(&RpcClient) -> Result<T, E>,
        E: std::fmt::Display,
    {
        let mut delay = Duration::from_millis(200);
        let mut last = String::new();
        for i in 0..attempts.max(1) {
            let idx = self.rotate();
            match f(self.client(idx)).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let msg = e.to_string();
                    let (rate, transient) = classify_error(&msg);
                    let retryable = rate || transient;
                    if i + 1 >= attempts || !retryable {
                        return Err(CommonError::Rpc(msg));
                    }
                    last = msg;
                    tokio::time::sleep(if rate {
                        Duration::from_millis(100)
                    } else {
                        delay
                    })
                    .await;
                    if !rate {
                        delay = (delay * 2).min(MAX_BACKOFF);
                    }
                }
            }
        }
        Err(CommonError::Rpc(last))
    }

    /// Run a call against ONE pinned endpoint (no rotation), so a transaction's
    /// send and its confirm poll hit the same node. Retries rate-limits always;
    /// retries transient errors only when `retry_transient` (port of `rpcOn`).
    pub async fn call_on<T, E, F>(
        &self,
        idx: usize,
        attempts: usize,
        retry_transient: bool,
        f: F,
    ) -> Result<T, CommonError>
    where
        F: AsyncFn(&RpcClient) -> Result<T, E>,
        E: std::fmt::Display,
    {
        let client = self.client(idx);
        let mut delay = Duration::from_millis(200);
        let mut last = String::new();
        for i in 0..attempts.max(1) {
            match f(client).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let msg = e.to_string();
                    let (rate, transient) = classify_error(&msg);
                    let retryable = rate || (retry_transient && transient);
                    if i + 1 >= attempts || !retryable {
                        return Err(CommonError::Rpc(msg));
                    }
                    last = msg;
                    tokio::time::sleep(if rate {
                        Duration::from_millis(100)
                    } else {
                        delay
                    })
                    .await;
                    if !rate {
                        delay = (delay * 2).min(MAX_BACKOFF);
                    }
                }
            }
        }
        Err(CommonError::Rpc(last))
    }
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_error() {
        assert_eq!(classify_error("429 Too Many Requests"), (true, false));
        assert_eq!(classify_error("connection timed out"), (false, true));
        assert_eq!(classify_error("invalid instruction data"), (false, false));
    }

    #[test]
    fn test_from_urls_parses_and_rejects_empty() {
        let pool = RpcPool::from_urls(
            "https://a.example/x, https://b.example/y",
            CommitmentConfig::confirmed(),
        )
        .unwrap();
        assert_eq!(pool.len(), 2);
        assert!(RpcPool::from_urls("  ", CommitmentConfig::confirmed()).is_err());
    }

    #[test]
    fn test_host_of() {
        assert_eq!(
            host_of("https://api.devnet.solana.com/abc"),
            "api.devnet.solana.com"
        );
    }
}
