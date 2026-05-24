// SPDX-License-Identifier: Apache-2.0

//! Direct-HTTP JSON-RPC client for DPoP-authenticated Tenzro calls.
//!
//! Why this exists alongside `tenzro_sdk::TenzroClient`:
//!
//! The SDK forwards `Authorization: DPoP <jwt>` + `DPoP: <proof>` from
//! the `TENZRO_BEARER_JWT` / `TENZRO_DPOP_PROOF` environment variables
//! (see `tenzro_sdk::rpc::RpcClient::call`). For receipt anchoring we
//! mint a **fresh** DPoP proof per call (RFC 9449 §4.2 requires a unique
//! `jti` + `iat` per request), which would mean mutating
//! `TENZRO_DPOP_PROOF` before every call. On edition 2024 that requires
//! `unsafe { std::env::set_var }` AND a process-wide lock to keep
//! concurrent calls from clobbering each other's proofs. Both are
//! avoidable: a direct HTTP call with per-request headers has none of
//! those constraints.
//!
//! This module also encodes the wire shape the live Tenzro testnet
//! expects today (probed 2026-05-23): `tenzro_signMessage` takes
//! `{ "message_hex": "<hex>" }` with no `private_key` argument — the
//! server identifies the signing key from the DPoP-bound JWT. The SDK's
//! `crypto.sign_message()` still sends the legacy `{ private_key,
//! message }` shape and is rejected by the live engine. Going through
//! this module instead keeps the workspace free of legacy/compat shims.

use serde::de::DeserializeOwned;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::auth::DpopSigner;
use crate::client::TenzroEdgeError;

/// A minimal JSON-RPC client that carries a bearer JWT + a per-call DPoP
/// proof signer. Cheap to clone — the inner `reqwest::Client` and
/// `DpopSigner` are both already cheap.
#[derive(Clone)]
pub(crate) struct DpopRpc {
    http: reqwest::Client,
    endpoint: String,
    bearer_jwt: String,
    signer: DpopSigner,
    request_id: std::sync::Arc<AtomicU64>,
}

impl DpopRpc {
    pub(crate) fn new(
        endpoint: String,
        bearer_jwt: String,
        signer: DpopSigner,
    ) -> Result<Self, TenzroEdgeError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| TenzroEdgeError::Sdk(format!("reqwest build: {e}")))?;
        Ok(Self {
            http,
            endpoint,
            bearer_jwt,
            signer,
            request_id: std::sync::Arc::new(AtomicU64::new(1)),
        })
    }

    /// Call a JSON-RPC method with `params`, sending a freshly-minted
    /// DPoP proof in the `DPoP:` header and the bearer JWT in
    /// `Authorization: DPoP <jwt>`.
    ///
    /// The proof's `htu` is the request URL (no query string per RFC
    /// 9449 §4.2) and `htm` is `"POST"`.
    ///
    /// ## Reverse-proxy `htu` workaround
    ///
    /// The Tenzro testnet (2026-05-23) terminates TLS at a reverse
    /// proxy and forwards requests to the inner service at
    /// `http://0.0.0.0:8545/`. The inner service compares the DPoP
    /// `htu` claim against its own bind address — not against the
    /// original client-visible URL nor the `X-Forwarded-*` headers —
    /// and rejects proofs that bind `htu` to the public URL with
    /// `-32001 DPoP htu mismatch: proof=<public>, request=<internal>`.
    ///
    /// As a client-side workaround, callers may set `TENZRO_DPOP_HTU`
    /// to the internal URL the server expects (e.g.
    /// `http://0.0.0.0:8545/`). When unset we use the public endpoint —
    /// which is what RFC 9449 §4.2 actually requires and what will
    /// work once the testnet is fixed upstream.
    pub(crate) async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, TenzroEdgeError> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let req_body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });
        let htu = std::env::var("TENZRO_DPOP_HTU").unwrap_or_else(|_| self.endpoint.clone());
        let proof = self.signer.mint_proof("POST", &htu)?;
        let resp = self
            .http
            .post(&self.endpoint)
            .header("Authorization", format!("DPoP {}", self.bearer_jwt))
            .header("DPoP", proof)
            .json(&req_body)
            .send()
            .await
            .map_err(|e| TenzroEdgeError::Sdk(format!("rpc transport: {e}")))?;
        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| TenzroEdgeError::Sdk(format!("rpc decode: {e}")))?;
        if !status.is_success() {
            return Err(TenzroEdgeError::Sdk(format!("rpc HTTP {status}: {body}")));
        }
        if let Some(err) = body.get("error") {
            return Err(TenzroEdgeError::Sdk(format!("rpc error: {err}")));
        }
        let result = body
            .get("result")
            .cloned()
            .ok_or_else(|| TenzroEdgeError::Sdk("rpc response missing result".into()))?;
        serde_json::from_value(result)
            .map_err(|e| TenzroEdgeError::Sdk(format!("rpc result decode: {e}")))
    }
}
