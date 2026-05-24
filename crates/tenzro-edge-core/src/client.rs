// SPDX-License-Identifier: Apache-2.0

//! Shared `TenzroClient` handle used by every trait impl, plus the common
//! error type the trait impls translate `SdkError` into.

use std::sync::Arc;

use thiserror::Error;

/// Top-level error common across every Tenzro Pi-class impl. Each trait
/// impl maps these into the trait-specific error variants.
#[derive(Debug, Error)]
pub enum TenzroEdgeError {
    /// Underlying SDK call failed (network, RPC, auth).
    #[error("tenzro sdk: {0}")]
    Sdk(String),
    /// Local persistence (offline buffer / token cache) error.
    #[error("local store: {0}")]
    Store(String),
    /// Misconfiguration — typically a missing required env var.
    #[error("config: {0}")]
    Config(String),
    /// The Tenzro SDK does not yet expose a surface needed for this trait
    /// method. The trait impl returns the appropriate trait-specific
    /// variant; this is the underlying reason.
    #[error("not yet supported by tenzro-sdk: {0}")]
    NotYetSupported(&'static str),
}

/// Pi-class configuration for the Tenzro handle.
///
/// Most users construct one via [`TenzroEdgeConfig::from_env`], which
/// reads:
///
/// - `TENZRO_RPC_URL` (required) — RPC endpoint, e.g. `https://rpc.tenzro.network`
/// - `TENZRO_API_KEY` (optional) — API key for unattended boot
/// - `TENZRO_CHAIN_ID` (optional, default 1) — chain id
/// - `TENZRO_TIMEOUT_MS` (optional, default 30000) — request timeout
#[derive(Clone, Debug)]
pub struct TenzroEdgeConfig {
    /// RPC endpoint.
    pub endpoint: String,
    /// Optional API key.
    pub api_key: Option<String>,
    /// Chain id.
    pub chain_id: u64,
    /// Per-request timeout in milliseconds.
    pub timeout_ms: u64,
    /// Max retries before falling back / surfacing Transient.
    pub max_retries: u32,
}

impl TenzroEdgeConfig {
    /// Read the Pi-class config from environment variables.
    ///
    /// # Errors
    /// Returns [`TenzroEdgeError::Config`] when `TENZRO_RPC_URL` is unset
    /// or any numeric env var fails to parse.
    pub fn from_env() -> Result<Self, TenzroEdgeError> {
        let endpoint = std::env::var("TENZRO_RPC_URL")
            .map_err(|_| TenzroEdgeError::Config("TENZRO_RPC_URL is required".into()))?;
        let api_key = std::env::var("TENZRO_API_KEY").ok();
        let chain_id = std::env::var("TENZRO_CHAIN_ID")
            .ok()
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|e| TenzroEdgeError::Config(format!("TENZRO_CHAIN_ID: {e}")))
            })
            .transpose()?
            .unwrap_or(1);
        let timeout_ms = std::env::var("TENZRO_TIMEOUT_MS")
            .ok()
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|e| TenzroEdgeError::Config(format!("TENZRO_TIMEOUT_MS: {e}")))
            })
            .transpose()?
            .unwrap_or(30_000);
        let max_retries = std::env::var("TENZRO_MAX_RETRIES")
            .ok()
            .map(|v| {
                v.parse::<u32>()
                    .map_err(|e| TenzroEdgeError::Config(format!("TENZRO_MAX_RETRIES: {e}")))
            })
            .transpose()?
            .unwrap_or(3);
        Ok(Self {
            endpoint,
            api_key,
            chain_id,
            timeout_ms,
            max_retries,
        })
    }

    /// Convert to the SDK's own config shape.
    #[must_use]
    pub fn into_sdk_config(self) -> tenzro_sdk::SdkConfig {
        tenzro_sdk::SdkConfig {
            endpoint: self.endpoint,
            timeout_ms: self.timeout_ms,
            max_retries: self.max_retries,
            api_key: self.api_key,
            chain_id: self.chain_id,
        }
    }
}

/// Shared handle wrapping a connected [`tenzro_sdk::TenzroClient`].
///
/// Cheap to clone — internally `Arc`-wrapped so every trait impl can hold
/// a copy without paying for re-connecting. Also retains the RPC
/// endpoint URL so the direct-HTTP DPoP path (see `dpop_rpc.rs`) can
/// reach the same node the SDK is talking to without re-reading env
/// vars.
#[derive(Clone)]
pub struct TenzroHandle {
    inner: Arc<tenzro_sdk::TenzroClient>,
    endpoint: String,
}

impl TenzroHandle {
    /// Connect to Tenzro using a fully-populated config.
    ///
    /// # Errors
    /// Returns [`TenzroEdgeError::Sdk`] when the SDK refuses to connect
    /// (bad endpoint, auth refused).
    pub async fn connect(config: TenzroEdgeConfig) -> Result<Self, TenzroEdgeError> {
        let endpoint = config.endpoint.clone();
        let client = tenzro_sdk::TenzroClient::connect(config.into_sdk_config())
            .await
            .map_err(|e| TenzroEdgeError::Sdk(format!("{e:?}")))?;
        Ok(Self {
            inner: Arc::new(client),
            endpoint,
        })
    }

    /// Connect using env-var defaults — see [`TenzroEdgeConfig::from_env`].
    ///
    /// # Errors
    /// Returns [`TenzroEdgeError::Config`] when required env vars are
    /// missing, or [`TenzroEdgeError::Sdk`] on connect failure.
    pub async fn connect_from_env() -> Result<Self, TenzroEdgeError> {
        let config = TenzroEdgeConfig::from_env()?;
        Self::connect(config).await
    }

    /// Borrow the inner SDK client. Trait impls go through this.
    #[must_use]
    pub fn sdk(&self) -> &tenzro_sdk::TenzroClient {
        &self.inner
    }

    /// RPC endpoint this handle is connected to. Used by the
    /// direct-HTTP DPoP path in `receipt_sink.rs`.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}
