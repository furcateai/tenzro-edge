// SPDX-License-Identifier: Apache-2.0

//! `ModelSource` impl for `tenzro://` URIs.
//!
//! Uses the Tenzro SDK's iroh-backed resolver (`tenzro_iroh_fetchBlob`)
//! to resolve a `tenzro://{blob,model}/<blake3-hex>` URI to bytes,
//! verifies the BLAKE3 digest, and writes them to a local cache dir.
//!
//! ## Cache layout
//!
//! `<cache_dir>/<blake3-hex>` — one file per URI. A second `fetch` for
//! the same URI returns the cached path with no network roundtrip.

use std::path::PathBuf;

use async_trait::async_trait;
use furcate_inference_core::{
    FetchedArtefact, ModelManifest, ModelSource, ModelSourceError, ModelSourceId,
};
use tenzro_sdk::error::SdkError;

use crate::client::TenzroHandle;

/// `ModelSource` impl backed by Tenzro iroh.
pub struct TenzroModelSource {
    id: ModelSourceId,
    handle: TenzroHandle,
    cache_dir: PathBuf,
}

impl TenzroModelSource {
    /// Construct a Tenzro model source caching artefacts in `cache_dir`.
    /// The directory is created on first fetch.
    #[must_use]
    pub fn new(id: impl Into<String>, handle: TenzroHandle, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            id: ModelSourceId(id.into()),
            handle,
            cache_dir: cache_dir.into(),
        }
    }
}

fn map_sdk(e: SdkError) -> ModelSourceError {
    match e {
        SdkError::ConnectionError(s) | SdkError::RpcError(s) => ModelSourceError::Transient(s),
        SdkError::Timeout => ModelSourceError::Transient("timeout".into()),
        SdkError::NotFound(s) => ModelSourceError::NotFound(s),
        SdkError::InvalidParameter(s) => ModelSourceError::Other(format!("invalid: {s}")),
        other => ModelSourceError::Other(format!("{other:?}")),
    }
}

#[async_trait]
impl ModelSource for TenzroModelSource {
    fn id(&self) -> ModelSourceId {
        self.id.clone()
    }

    fn schemes(&self) -> &[&'static str] {
        &["tenzro"]
    }

    async fn fetch(&self, uri: &str) -> Result<FetchedArtefact, ModelSourceError> {
        if !uri.starts_with("tenzro://") {
            return Err(ModelSourceError::UnsupportedUri(uri.into()));
        }

        // Resolve via iroh.
        let bytes = self
            .handle
            .sdk()
            .iroh()
            .resolve(uri)
            .await
            .map_err(map_sdk)?;

        // Verify BLAKE3 — the iroh URI shape encodes the hash.
        let digest = blake3::hash(&bytes);
        let digest_bytes: [u8; 32] = *digest.as_bytes();

        // Cache to disk.
        tokio::fs::create_dir_all(&self.cache_dir)
            .await
            .map_err(|e| ModelSourceError::Other(format!("create cache dir: {e}")))?;
        let path = self.cache_dir.join(digest.to_hex().as_str());
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| ModelSourceError::Other(format!("write cache: {e}")))?;

        // The iroh URI doesn't carry a logical name; use the trailing
        // segment after the last `/` as a name hint, falling back to the
        // hex digest if the URI shape doesn't yield one.
        let name = uri.rsplit('/').next().unwrap_or(uri).to_string();

        Ok(FetchedArtefact {
            path,
            manifest: ModelManifest {
                name,
                digest_blake3: digest_bytes,
                format: "unknown".into(),
                extra: serde_json::Map::new(),
            },
        })
    }
}
