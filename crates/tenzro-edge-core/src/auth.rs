// SPDX-License-Identifier: Apache-2.0

//! DPoP (RFC 9449) proof minting for privileged Tenzro RPCs.
//!
//! The Tenzro testnet enforces RFC 9449 §4 on every privileged JSON-RPC
//! call: each request must carry both `Authorization: DPoP <jwt>` and a
//! `DPoP: <proof>` header. The bearer JWT is minted once during
//! onboarding (via `tenzro_onboardHuman` etc.); the DPoP proof is a
//! short-lived JWT signed per-call by the holder's Ed25519 key and
//! attests that the bearer is presenting the token correctly.
//!
//! The SDK forwards the bearer + DPoP proof via the
//! `TENZRO_BEARER_JWT` / `TENZRO_DPOP_PROOF` environment variables — but
//! the env-var dance is only safe for sequential calls and requires
//! `unsafe { std::env::set_var }` on edition 2024. Privileged calls in
//! this crate that need DPoP go through `dpop_rpc::call` instead, which
//! makes a direct HTTP call with the bearer + freshly-minted proof in
//! per-request headers — no global mutable state.
//!
//! This module owns:
//!
//! 1. Generating + persisting the Ed25519 holder key
//! 2. Computing its RFC 7638 JWK thumbprint (`jkt`) for onboarding
//! 3. Minting a fresh DPoP proof JWT per call
//!
//! The DPoP proof JWT shape (RFC 9449 §4.2):
//!
//! ```text
//! header:  { "typ": "dpop+jwt", "alg": "EdDSA", "jwk": <holder JWK> }
//! payload: { "jti": <random>, "htm": <HTTP method>, "htu": <URL>, "iat": <now> }
//! ```
//!
//! Signed with the holder's Ed25519 private key, JWS-compact encoded
//! (three base64url segments joined by `.`).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};

use crate::client::TenzroEdgeError;

/// Holder keypair for DPoP proofs. Cheap to clone — the inner signing
/// key is small (32-byte secret + derived public).
#[derive(Clone)]
pub struct DpopSigner {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    /// Cached JWK thumbprint (`jkt` claim value, RFC 7638). Derived
    /// once and reused — onboarding sends it as `dpop_jkt`.
    jkt: String,
}

impl DpopSigner {
    /// Generate a fresh Ed25519 holder keypair. The thumbprint is
    /// derived eagerly so subsequent calls are pure compute.
    #[must_use]
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let jkt = jwk_thumbprint_ed25519(&verifying_key);
        Self {
            signing_key,
            verifying_key,
            jkt,
        }
    }

    /// Reconstruct a signer from a 32-byte Ed25519 seed.
    ///
    /// Useful when the seed has been persisted (e.g. to a keystore)
    /// across restarts so the bearer JWT remains valid.
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let jkt = jwk_thumbprint_ed25519(&verifying_key);
        Self {
            signing_key,
            verifying_key,
            jkt,
        }
    }

    /// RFC 7638 JWK thumbprint of the holder public key. Pass as
    /// `dpop_jkt` during onboarding so the issued JWT is DPoP-bound to
    /// this key.
    #[must_use]
    pub fn jwk_thumbprint(&self) -> &str {
        &self.jkt
    }

    /// Raw 32-byte seed for persistence. Treat as a secret.
    #[must_use]
    pub fn seed(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    /// Mint a fresh DPoP proof JWT for an HTTP method + URL.
    ///
    /// The proof's `iat` is "now" and `jti` is a random 128-bit nonce,
    /// so each call gets a distinct proof — required by RFC 9449 §4.2
    /// to prevent replay.
    ///
    /// # Errors
    /// Returns [`TenzroEdgeError::Config`] if the system clock is
    /// before the Unix epoch (effectively never on real hardware).
    pub fn mint_proof(&self, htm: &str, htu: &str) -> Result<String, TenzroEdgeError> {
        // Header: typ = "dpop+jwt", alg = "EdDSA", jwk = embedded public key
        let jwk = serde_json::json!({
            "kty": "OKP",
            "crv": "Ed25519",
            "x": URL_SAFE_NO_PAD.encode(self.verifying_key.to_bytes()),
        });
        let header = serde_json::json!({
            "typ": "dpop+jwt",
            "alg": "EdDSA",
            "jwk": jwk,
        });

        // Payload: jti, htm, htu, iat
        let iat = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| TenzroEdgeError::Config(format!("system clock pre-epoch: {e}")))?
            .as_secs();
        let mut jti_bytes = [0u8; 16];
        rand_core::RngCore::fill_bytes(&mut OsRng, &mut jti_bytes);
        let jti = hex::encode(jti_bytes);
        let payload = serde_json::json!({
            "jti": jti,
            "htm": htm,
            "htu": htu,
            "iat": iat,
        });

        // Encode + sign
        let header_b64 = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&header)
                .map_err(|e| TenzroEdgeError::Config(format!("dpop header serialise: {e}")))?,
        );
        let payload_b64 =
            URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&payload).map_err(|e| {
                    TenzroEdgeError::Config(format!("dpop payload serialise: {e}"))
                })?);
        let signing_input = format!("{header_b64}.{payload_b64}");
        let signature = self.signing_key.sign(signing_input.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());
        Ok(format!("{signing_input}.{sig_b64}"))
    }
}

/// RFC 7638 §3 JWK thumbprint for an Ed25519 public key. The canonical
/// form for OKP/Ed25519 is `{"crv":"Ed25519","kty":"OKP","x":<base64url>}`
/// with members in lexicographic order and no whitespace.
fn jwk_thumbprint_ed25519(key: &VerifyingKey) -> String {
    let canonical = format!(
        r#"{{"crv":"Ed25519","kty":"OKP","x":"{}"}}"#,
        URL_SAFE_NO_PAD.encode(key.to_bytes())
    );
    let hash = Sha256::digest(canonical.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbprint_is_stable_for_fixed_seed() {
        let seed = [42u8; 32];
        let a = DpopSigner::from_seed(seed);
        let b = DpopSigner::from_seed(seed);
        assert_eq!(a.jwk_thumbprint(), b.jwk_thumbprint());
        assert!(!a.jwk_thumbprint().is_empty());
    }

    #[test]
    fn proof_has_three_segments_and_is_self_consistent() {
        let signer = DpopSigner::generate();
        let proof = signer
            .mint_proof("POST", "https://rpc.tenzro.network")
            .expect("mint");
        let parts: Vec<&str> = proof.split('.').collect();
        assert_eq!(parts.len(), 3, "JWS compact form has 3 segments");
        // Verify the signature against the signing input
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let sig_bytes = URL_SAFE_NO_PAD.decode(parts[2]).expect("sig b64");
        let sig: [u8; 64] = sig_bytes.try_into().expect("64-byte sig");
        let signature = ed25519_dalek::Signature::from_bytes(&sig);
        ed25519_dalek::Verifier::verify(
            &signer.verifying_key,
            signing_input.as_bytes(),
            &signature,
        )
        .expect("self-verify");
    }

    #[test]
    fn proofs_have_distinct_jti() {
        let signer = DpopSigner::generate();
        let p1 = signer.mint_proof("POST", "https://x").expect("p1");
        let p2 = signer.mint_proof("POST", "https://x").expect("p2");
        // Different jti → different payload → different signature
        assert_ne!(p1, p2);
    }
}
