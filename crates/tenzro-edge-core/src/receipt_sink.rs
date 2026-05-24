// SPDX-License-Identifier: Apache-2.0

//! `ReceiptSink` impl that anchors receipts via `tenzro_signMessage`.
//!
//! ## Why `tenzro_signMessage` and not `tenzro_settle`
//!
//! `tenzro_settle` is for escrow-mediated value transfer with
//! cryptographic proof requirements (Plonky3 STARKs + Ed25519
//! channel-state signatures per `tenzro.com/docs/settlement`). Tenzro
//! testnet enforces a 1000-base-unit minimum and rejects unsigned
//! settle calls with `-32000 Invalid proof: Cryptographic proof requires
//! signatures`. Recording-that-work-happened is not a value-transfer
//! event — wrong primitive.
//!
//! `tenzro_signMessage` signs an arbitrary hex blob with the holder's
//! MPC-managed wallet key (the server identifies the key from the
//! DPoP-bound JWT). The result is a wallet-bound signature attributable
//! to the holder's DID. No on-chain write, no minimum amount, no escrow
//! proofs. This is the right primitive for a DID-bound attestation
//! that this receipt digest existed at this time — see
//! `project_tenzro_receipt_sink_design.md`.
//!
//! ## Wire-shape note
//!
//! The live testnet (probed 2026-05-23) expects
//! `{ "message_hex": "<hex>" }` and rejects the SDK's legacy
//! `{ private_key, message }` payload with `-32001 Authentication
//! required`. We call the RPC directly via [`dpop_rpc::DpopRpc`] rather
//! than through `tenzro_sdk::CryptoClient::sign_message` for that
//! reason.
//!
//! ## Offline buffering
//!
//! On any error from the RPC, the impl returns
//! [`SinkError::Transient`]; the caller (or a thin redb-backed queue
//! layer above this) is responsible for replay. The trait surface
//! explicitly leaves retry policy to the caller — see
//! `furcate-inference-core::SinkError`.

use async_trait::async_trait;
use furcate_inference_core::{Attestation, ReceiptSink, SinkError, SinkId, StepReceipt};
use serde::Deserialize;

use crate::auth::DpopSigner;
use crate::client::{TenzroEdgeError, TenzroHandle};
use crate::dpop_rpc::DpopRpc;

/// Wire shape of the `tenzro_signMessage` result (2026-05-23 live
/// testnet). Decoded for structural validation; the result is consumed
/// inside `write` but not yet surfaced through the `ReceiptSink` trait
/// (whose `write` returns `()`). The server retains the signature in
/// its audit log keyed by message digest, so downstream verifiers can
/// query it independently — see receipt-sink design memo.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // fields decoded for shape-validation; see doc above.
struct SignMessageResult {
    /// Hex-encoded signature over the message bytes, signed by the
    /// wallet's MPC key.
    signature: String,
    /// Hex-encoded public key the signature can be verified against.
    /// Not all engine versions return this; defaulted to empty when
    /// absent.
    #[serde(default)]
    public_key: String,
    /// Algorithm identifier (e.g. `"ed25519"`). Defaulted when absent.
    #[serde(default)]
    algorithm: String,
}

/// `ReceiptSink` impl that records receipts via `tenzro_signMessage`.
///
/// Construct via [`TenzroReceiptSink::new`] after onboarding a DPoP
/// signer through the SDK's `auth` client. The sink holds the bearer
/// JWT + signer and mints a fresh DPoP proof per call.
pub struct TenzroReceiptSink {
    id: SinkId,
    rpc: DpopRpc,
}

impl TenzroReceiptSink {
    /// Construct a DPoP-authenticated receipt sink.
    ///
    /// - `id` — sink id surfaced to the inference layer.
    /// - `handle` — connected Tenzro handle (used here only to read the
    ///   endpoint; the SDK's `RpcClient` is bypassed because it
    ///   forwards DPoP via env vars rather than per-request headers).
    /// - `bearer_jwt` — access token from an `onboard_human` /
    ///   `onboard_delegated_agent` / `onboard_autonomous_agent` call
    ///   that supplied `signer.jwk_thumbprint()` as `dpop_jkt`.
    /// - `signer` — the holder's Ed25519 key whose JWK thumbprint was
    ///   bound to the JWT during onboarding. Mints per-call DPoP proofs.
    ///
    /// # Errors
    ///
    /// Returns [`TenzroEdgeError::Sdk`] if the internal `reqwest::Client`
    /// fails to build (rare — usually only on TLS root failure).
    pub fn new(
        id: impl Into<String>,
        handle: &TenzroHandle,
        bearer_jwt: String,
        signer: DpopSigner,
    ) -> Result<Self, TenzroEdgeError> {
        let endpoint = handle.endpoint().to_string();
        let rpc = DpopRpc::new(endpoint, bearer_jwt, signer)?;
        Ok(Self {
            id: SinkId(id.into()),
            rpc,
        })
    }
}

#[async_trait]
impl ReceiptSink for TenzroReceiptSink {
    fn id(&self) -> SinkId {
        self.id.clone()
    }

    async fn write(
        &self,
        receipt: &StepReceipt,
        _attestations: &[Attestation],
    ) -> Result<(), SinkError> {
        // Canonicalise the receipt and hash it. The same digest is what
        // `TenzroAttester::sign_receipt` would sign — so an off-chain
        // verifier can cross-check the Tenzro-signed attestation against
        // any other attester's signature over the same bytes.
        let canon = serde_json::to_vec(receipt)
            .map_err(|e| SinkError::Rejected(format!("canonicalise receipt: {e}")))?;
        let digest = blake3::hash(&canon);
        let message_hex = digest.to_hex().to_string();

        // Call `tenzro_signMessage` with the receipt digest. The server
        // identifies the signing key from the DPoP-bound JWT; we don't
        // (and cannot) supply the MPC wallet's private key directly.
        let _signed: SignMessageResult = self
            .rpc
            .call(
                "tenzro_signMessage",
                serde_json::json!([{ "message_hex": message_hex }]),
            )
            .await
            .map_err(|e| SinkError::Transient(format!("tenzro_signMessage: {e}")))?;

        // Future: surface the returned signature back to the caller so
        // it can be persisted alongside the receipt as an attestation.
        // The `ReceiptSink` trait's `write` signature currently returns
        // `()`, so the signature is captured by the server's audit log
        // (queryable by digest) and not retained locally. This matches
        // the Minima sink's behaviour — both anchor, neither return a
        // post-anchor receipt.

        Ok(())
    }

    async fn flush(&self) -> Result<(), SinkError> {
        // `tenzro_signMessage` is per-call synchronous; nothing to
        // flush at this layer.
        Ok(())
    }
}
