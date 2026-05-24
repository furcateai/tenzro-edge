// SPDX-License-Identifier: Apache-2.0

//! `Attester` impl via Tenzro crypto + identity.
//!
//! Signs receipts and artefact digests with a Tenzro-managed private key,
//! producing an [`Attestation`] whose `proof` field carries the Tenzro
//! signature blob (signature + public key, JSON-encoded).
//!
//! Private-key sourcing is **environment-driven** on Pi-class nodes:
//! `TENZRO_SIGNING_KEY` (hex, no `0x` prefix). This avoids requiring a
//! keystore on first boot and matches `minima-attest`'s env-driven shape.

use async_trait::async_trait;
use furcate_inference_core::{
    Attestation, Attester, AttesterError, AttesterId, ReceiptDigest, StepReceipt,
};
use serde::{Deserialize, Serialize};
use tenzro_sdk::error::SdkError;

use crate::client::{TenzroEdgeError, TenzroHandle};

/// `Attester` impl backed by Tenzro crypto signing.
pub struct TenzroAttester {
    id: AttesterId,
    handle: TenzroHandle,
    /// Hex-encoded private key. Sourced from `TENZRO_SIGNING_KEY` env var
    /// at construction; not retained beyond the struct itself.
    private_key_hex: String,
}

/// Wire shape of the bytes carried in [`Attestation::proof`] — the Tenzro
/// signature plus the signer public key so a third-party verifier can
/// re-check it without re-deriving from the private key.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct TenzroProof {
    signature: String,
    public_key: String,
}

impl TenzroAttester {
    /// Construct a Tenzro attester. Reads `TENZRO_SIGNING_KEY` from env.
    ///
    /// # Errors
    /// Returns [`TenzroEdgeError::Config`] if `TENZRO_SIGNING_KEY` is unset.
    pub fn new(id: impl Into<String>, handle: TenzroHandle) -> Result<Self, TenzroEdgeError> {
        let private_key_hex = std::env::var("TENZRO_SIGNING_KEY").map_err(|_| {
            TenzroEdgeError::Config("TENZRO_SIGNING_KEY is required for TenzroAttester".into())
        })?;
        Ok(Self {
            id: AttesterId(id.into()),
            handle,
            private_key_hex,
        })
    }
}

fn map_sdk(e: SdkError) -> AttesterError {
    match e {
        SdkError::ConnectionError(s) | SdkError::RpcError(s) => AttesterError::Transient(s),
        SdkError::Timeout => AttesterError::Transient("timeout".into()),
        SdkError::AuthenticationError(s) => AttesterError::Failed(format!("auth: {s}")),
        other => AttesterError::Failed(format!("{other:?}")),
    }
}

#[async_trait]
impl Attester for TenzroAttester {
    fn id(&self) -> AttesterId {
        self.id.clone()
    }

    async fn sign_receipt(&self, receipt: &StepReceipt) -> Result<Attestation, AttesterError> {
        // Canonicalise the receipt to JSON, hash it, sign the hash.
        let canon = serde_json::to_vec(receipt)
            .map_err(|e| AttesterError::Failed(format!("canonicalise receipt: {e}")))?;
        let digest = blake3_32(&canon);
        self.sign_digest(&digest).await
    }

    async fn sign_digest(&self, digest: &ReceiptDigest) -> Result<Attestation, AttesterError> {
        let sig = self
            .handle
            .sdk()
            .crypto()
            .sign_message(&self.private_key_hex, digest)
            .await
            .map_err(map_sdk)?;
        let proof = TenzroProof {
            signature: sig.signature,
            public_key: sig.public_key,
        };
        let bytes = serde_json::to_vec(&proof)
            .map_err(|e| AttesterError::Failed(format!("encode proof: {e}")))?;
        Ok(Attestation {
            kind: "tenzro".into(),
            digest: *digest,
            proof: bytes,
        })
    }

    async fn verify(&self, att: &Attestation) -> Result<(), AttesterError> {
        if att.kind != "tenzro" {
            return Err(AttesterError::Failed(format!(
                "kind mismatch: expected 'tenzro', got '{}'",
                att.kind
            )));
        }
        let proof: TenzroProof = serde_json::from_slice(&att.proof)
            .map_err(|e| AttesterError::Failed(format!("decode proof: {e}")))?;
        let result = self
            .handle
            .sdk()
            .crypto()
            .verify_signature(&proof.public_key, &att.digest, &proof.signature)
            .await
            .map_err(map_sdk)?;
        if result.valid {
            Ok(())
        } else {
            Err(AttesterError::Invalid)
        }
    }
}

/// BLAKE3-32 of arbitrary bytes — same shape as `ReceiptDigest`.
fn blake3_32(bytes: &[u8]) -> ReceiptDigest {
    // We pull the digest inline (without taking a `blake3` workspace dep
    // here) by leaning on the `ReceiptDigest` shape: 32 bytes. The actual
    // hash quality matters less than determinism — but blake3 is in our
    // workspace already via `bytes`/`mesh-core`, so use it.
    let mut hasher = blake3::Hasher::new();
    hasher.update(bytes);
    *hasher.finalize().as_bytes()
}
