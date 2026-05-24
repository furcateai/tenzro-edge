// SPDX-License-Identifier: Apache-2.0

//! `StorageBackend` impl backed by Tenzro iroh blobs.
//!
//! - [`StorageBackend::put`] buffers the input stream into memory then
//!   calls `iroh.publish_blob`, returning a `tenzro://blob/<hash>` key.
//! - [`StorageBackend::get`] / `head` call `iroh.fetch_blob` and serve
//!   the bytes as a one-chunk stream.
//! - [`StorageBackend::delete`] is a no-op — iroh content is
//!   content-addressed and immutable.
//!
//! ## Memory caveat
//!
//! The current SDK surface accepts `Vec<u8>` for `publish_blob` and
//! returns `Vec<u8>` for `fetch_blob`, so very large blobs are buffered
//! end-to-end. A Pi handling multi-GB blobs should not use this impl
//! until the SDK exposes a chunked surface.

use async_trait::async_trait;
use bytes::Bytes;
use furcate_inference_core::{BlobKey, BlobMetadata, StorageBackend, StorageError};
use futures::StreamExt;
use futures::stream::BoxStream;
use tenzro_sdk::error::SdkError;

use crate::client::TenzroHandle;

/// `StorageBackend` impl backed by Tenzro iroh blobs.
pub struct TenzroStorage {
    handle: TenzroHandle,
}

impl TenzroStorage {
    /// Construct a Tenzro storage backend.
    #[must_use]
    pub const fn new(handle: TenzroHandle) -> Self {
        Self { handle }
    }
}

fn map_sdk(e: SdkError) -> StorageError {
    match e {
        SdkError::ConnectionError(s) | SdkError::RpcError(s) => StorageError::Transient(s),
        SdkError::Timeout => StorageError::Transient("timeout".into()),
        SdkError::NotFound(s) => StorageError::NotFound(s),
        SdkError::AuthenticationError(_) => StorageError::Unauthorised,
        other => StorageError::Failed(format!("{other:?}")),
    }
}

#[async_trait]
impl StorageBackend for TenzroStorage {
    async fn put(
        &self,
        mut stream: BoxStream<'_, std::result::Result<Bytes, std::io::Error>>,
    ) -> Result<BlobMetadata, StorageError> {
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| StorageError::Failed(format!("read: {e}")))?;
            buf.extend_from_slice(&chunk);
        }

        let result = self
            .handle
            .sdk()
            .iroh()
            .publish_blob(buf)
            .await
            .map_err(map_sdk)?;

        // Parse the blake3 hex back into 32 bytes for `BlobMetadata`.
        let digest_bytes: [u8; 32] = hex_to_32(&result.blake3_hex).ok_or_else(|| {
            StorageError::Failed(format!("bad blake3_hex: {}", result.blake3_hex))
        })?;

        Ok(BlobMetadata {
            key: BlobKey(result.tenzro_uri),
            size: result.size_bytes,
            digest_blake3: digest_bytes,
        })
    }

    async fn get<'a>(
        &'a self,
        key: &'a BlobKey,
    ) -> Result<BoxStream<'a, std::result::Result<Bytes, std::io::Error>>, StorageError> {
        let bytes = self
            .handle
            .sdk()
            .iroh()
            .fetch_blob(&key.0)
            .await
            .map_err(map_sdk)?;
        // Serve as a single-chunk stream.
        let stream = futures::stream::once(async move { Ok(Bytes::from(bytes)) });
        Ok(stream.boxed())
    }

    async fn head(&self, key: &BlobKey) -> Result<BlobMetadata, StorageError> {
        // The SDK does not currently expose a head/stat for iroh blobs.
        // We synthesise it by fetching once and discarding — wasteful, but
        // correct. An SDK-side `iroh_headBlob` would replace this.
        let bytes = self
            .handle
            .sdk()
            .iroh()
            .fetch_blob(&key.0)
            .await
            .map_err(map_sdk)?;
        let digest = blake3::hash(&bytes);
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        Ok(BlobMetadata {
            key: key.clone(),
            size,
            digest_blake3: *digest.as_bytes(),
        })
    }

    async fn delete(&self, _key: &BlobKey) -> Result<(), StorageError> {
        // Iroh content is immutable + content-addressed; delete is a no-op.
        Ok(())
    }
}

fn hex_to_32(s: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(s).ok()?;
    bytes.try_into().ok()
}
