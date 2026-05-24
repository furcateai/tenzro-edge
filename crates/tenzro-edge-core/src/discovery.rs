// SPDX-License-Identifier: Apache-2.0

//! `DiscoveryBackend` impl backed by Tenzro's provider directory.
//!
//! Polls `provider.list_providers()` and yields each `NetworkProvider`
//! as a [`PeerAnnouncement`]. Polling cadence is configurable via
//! [`TenzroDiscovery::with_poll_interval`] (default 30s) — Tenzro's
//! directory does not currently offer a streaming watch RPC, so this is
//! the best we can do without bloating the trait surface with backend-
//! specific event types.
//!
//! ## PeerId mapping
//!
//! Tenzro provider IDs are libp2p peer-id strings (variable-length
//! base58); mesh-core's [`PeerId`] is a fixed 32-byte array. We derive a
//! stable 32-byte id by hashing the libp2p peer-id string with BLAKE3,
//! which is collision-free for any practical mesh size. The original
//! peer-id string is preserved in `labels` for debugging.

use std::time::Duration;

use async_trait::async_trait;
use furcate_mesh_core::PeerId;
use furcate_mesh_core::extensions::{
    AnnouncementStream, DiscoveryBackend, DiscoveryError, PeerAnnouncement,
};
use futures::StreamExt;
use tenzro_sdk::error::SdkError;
use tenzro_sdk::provider::NetworkProvider;

use crate::client::TenzroHandle;

/// `DiscoveryBackend` impl backed by Tenzro.
pub struct TenzroDiscovery {
    handle: TenzroHandle,
    poll_interval: Duration,
}

impl TenzroDiscovery {
    /// Construct a Tenzro discovery backend with the default 30-second
    /// poll interval.
    #[must_use]
    pub const fn new(handle: TenzroHandle) -> Self {
        Self {
            handle,
            poll_interval: Duration::from_secs(30),
        }
    }

    /// Override the poll interval. Shorter intervals trade RPC load for
    /// freshness; longer intervals are fine when the peer set is stable.
    #[must_use]
    pub const fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

fn map_sdk(e: SdkError) -> DiscoveryError {
    match e {
        SdkError::ConnectionError(s) | SdkError::RpcError(s) => DiscoveryError::Terminated(s),
        SdkError::Timeout => DiscoveryError::Terminated("timeout".into()),
        other => DiscoveryError::Setup(format!("{other:?}")),
    }
}

fn peer_id_from_libp2p(peer_id_str: &str) -> PeerId {
    let h = blake3::hash(peer_id_str.as_bytes());
    PeerId(*h.as_bytes())
}

fn announcement_from_provider(p: &NetworkProvider) -> PeerAnnouncement {
    let mut labels = Vec::new();
    if !p.peer_id.is_empty() {
        labels.push(format!("libp2p={}", p.peer_id));
    }
    if !p.provider_type.is_empty() {
        labels.push(format!("type={}", p.provider_type));
    }
    if !p.status.is_empty() {
        labels.push(format!("status={}", p.status));
    }
    for cap in &p.capabilities {
        labels.push(format!("cap={cap}"));
    }
    for m in &p.served_models {
        labels.push(format!("model={m}"));
    }
    let endpoint = if p.rpc_endpoint.is_empty() {
        None
    } else {
        Some(p.rpc_endpoint.clone())
    };
    PeerAnnouncement {
        peer: peer_id_from_libp2p(&p.peer_id),
        endpoint,
        labels,
    }
}

#[async_trait]
impl DiscoveryBackend for TenzroDiscovery {
    async fn start(&self) -> Result<AnnouncementStream, DiscoveryError> {
        let handle = self.handle.clone();
        let interval = self.poll_interval;

        // Poll loop: every `interval` seconds, list providers and emit
        // one announcement per provider. Stream is infinite — caller
        // drops to stop.
        let stream = async_stream::try_stream! {
            loop {
                let providers = handle
                    .sdk()
                    .provider()
                    .list_providers()
                    .await
                    .map_err(map_sdk)?;
                for p in providers {
                    yield announcement_from_provider(&p);
                }
                tokio::time::sleep(interval).await;
            }
        };

        Ok(stream.boxed())
    }
}
